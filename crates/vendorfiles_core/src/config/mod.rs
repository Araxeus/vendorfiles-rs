//! Config discovery, the [`Workspace`] that owns everything parsed out of it, and write-back.

pub mod document;
pub mod format;
pub mod yaml_emit;

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::error::{Result, VendorError};
use crate::fsx::{anchor, join_normalized, normalize, real_path};
use crate::model::{DefaultOptions, RawDependency, VendorConfig};
use crate::template::replace_vendor_folder;

pub use document::ConfigDocument;
pub use format::{ConfigFormat, Indent};

/// The file names searched for, in priority order.
pub const CONFIG_FILE_NAMES: [&str; 5] = [
    "vendor.toml",
    "vendor.yml",
    "vendor.yaml",
    "vendor.json",
    "package.json",
];

/// Everything needed to write the config back exactly as it was formatted.
#[derive(Debug, Clone)]
pub struct ConfigFileSettings {
    pub format: ConfigFormat,
    pub path: PathBuf,
    pub indent: Indent,
    pub final_newline: String,
}

/// The config file: how to write it, and its editable contents.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub settings: ConfigFileSettings,
    pub document: ConfigDocument,
}

impl ConfigFile {
    /// Renders and writes the document, restoring the original trailing newline.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::SerializeConfig`] if the document cannot be encoded, or
    /// [`VendorError::WriteFile`] if it cannot be written.
    pub async fn write(&self) -> Result<()> {
        let body = self
            .document
            .render(self.settings.format, &self.settings.indent)?;
        let data = format!("{body}{}", self.settings.final_newline);
        tokio::fs::write(&self.settings.path, data)
            .await
            .map_err(|source| VendorError::WriteFile {
                path: self.settings.path.clone(),
                source,
            })
    }

    /// The config file path as the reference prints it.
    #[must_use]
    pub fn display_path(&self) -> String {
        self.settings.path.display().to_string()
    }
}

/// The loaded project: config block, dependencies with defaults applied, and the raw file.
///
/// `Workspace` is the sole owner of config state for the lifetime of a command. Operations
/// borrow it mutably and clone the single dependency they act on, which keeps every signature
/// free of lifetime parameters at a cost that is invisible next to a network round-trip.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub config: VendorConfig,
    pub dependencies: IndexMap<String, RawDependency>,
    pub defaults: DefaultOptions,
    pub file: ConfigFile,
}

impl Workspace {
    /// Loads the config file, searching `config_location` (or the environment) for it.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::NoConfigFile`] when no config file is found, [`VendorError::ReadFile`]
    /// when the start path does not exist, [`VendorError::ParseConfig`] when the file is malformed,
    /// or one of the `Invalid*Key` variants when a modelled key has the wrong type.
    pub async fn load(config_location: Option<&str>) -> Result<Self> {
        let start = resolve_start_path(config_location);
        let folder = real_path(&start)?;
        let (path, text) = find_config_file(&folder)
            .await
            .ok_or(VendorError::NoConfigFile)?;

        let format = ConfigFormat::from_path(&path);
        let canonical = format.parse(&text, &path)?;
        let document = ConfigDocument::parse(format, &text, &path)?;
        let display_path = path.display().to_string();

        let config = read_vendor_config(&canonical, &display_path)?;
        let defaults = read_defaults(&canonical);
        let mut dependencies = read_dependencies(&canonical, &display_path)?;
        for dependency in dependencies.values_mut() {
            dependency.apply_defaults(&defaults);
        }

        Ok(Self {
            config,
            dependencies,
            defaults,
            file: ConfigFile {
                settings: ConfigFileSettings {
                    format,
                    path,
                    indent: Indent::detect(&text),
                    final_newline: format::final_newline(&text),
                },
                document,
            },
        })
    }

    /// The folder a dependency's files are written to.
    #[must_use]
    pub fn dependency_folder(&self, vendor_folder: Option<&str>, name: &str) -> PathBuf {
        dependency_folder(&self.config, &self.file.settings.path, vendor_folder, name)
    }
}

/// Resolves the folder (or file) the config search starts from.
///
/// `-c` beats `VENDOR_CONFIG` beats `INIT_CWD` beats `PWD` beats the process cwd - and, as in
/// the reference, an empty value falls through to the next candidate.
fn resolve_start_path(config_location: Option<&str>) -> PathBuf {
    let from_env = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    config_location
        .filter(|v| !v.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| from_env("VENDOR_CONFIG"))
        .or_else(|| from_env("INIT_CWD"))
        .or_else(|| from_env("PWD"))
        .map_or_else(
            || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            PathBuf::from,
        )
}

/// Finds the first config file at `folder_or_file`, returning its path and contents.
async fn find_config_file(folder_or_file: &Path) -> Option<(PathBuf, String)> {
    let as_str = folder_or_file.to_string_lossy();
    if CONFIG_FILE_NAMES.iter().any(|name| as_str.ends_with(name))
        && let Ok(text) = tokio::fs::read_to_string(folder_or_file).await
    {
        return Some((folder_or_file.to_path_buf(), text));
    }
    for name in CONFIG_FILE_NAMES {
        let candidate = folder_or_file.join(name);
        if let Ok(text) = tokio::fs::read_to_string(&candidate).await {
            return Some((normalize(&candidate), text));
        }
    }
    None
}

/// Reads `vendorConfig`, falling back to the default folder when absent or falsy.
fn read_vendor_config(canonical: &serde_json::Value, path: &str) -> Result<VendorConfig> {
    let invalid = || VendorError::InvalidConfigKey(path.to_owned());
    match canonical.get("vendorConfig") {
        None => Ok(VendorConfig::default()),
        Some(value) if is_falsy(value) => Ok(VendorConfig::default()),
        Some(serde_json::Value::Object(map)) => match map.get("vendorFolder") {
            None => Ok(VendorConfig::default()),
            Some(v) if is_falsy(v) => Ok(VendorConfig::default()),
            Some(serde_json::Value::String(folder)) => Ok(VendorConfig {
                vendor_folder: folder.clone(),
            }),
            Some(_) => Err(invalid()),
        },
        Some(_) => Err(invalid()),
    }
}

/// Reads `vendorDependencies`, tolerating a missing or falsy key.
fn read_dependencies(
    canonical: &serde_json::Value,
    path: &str,
) -> Result<IndexMap<String, RawDependency>> {
    let invalid = || VendorError::InvalidDependenciesKey(path.to_owned());
    match canonical.get("vendorDependencies") {
        None => Ok(IndexMap::new()),
        Some(value) if is_falsy(value) => Ok(IndexMap::new()),
        Some(value @ serde_json::Value::Object(_)) => {
            serde_json::from_value(value.clone()).map_err(|_| invalid())
        }
        Some(_) => Err(invalid()),
    }
}

/// Reads `defaultVendorOptions`, then `default`.
fn read_defaults(canonical: &serde_json::Value) -> DefaultOptions {
    ["defaultVendorOptions", "default"]
        .into_iter()
        .find_map(|key| canonical.get(key))
        .filter(|v| v.is_object())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// JavaScript truthiness for the values a config can hold.
fn is_falsy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::Number(n) => n.as_f64() == Some(0.0),
        serde_json::Value::String(s) => s.is_empty(),
        _ => false,
    }
}

/// The folder a dependency's files land in.
///
/// A dependency's own `vendorFolder` (with `{vendorFolder}` expanded) is used verbatim;
/// otherwise the global folder gets the dependency name appended.
#[must_use]
pub fn dependency_folder(
    config: &VendorConfig,
    config_path: &Path,
    vendor_folder: Option<&str>,
    name: &str,
) -> PathBuf {
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    vendor_folder.map_or_else(
        || join_normalized(&anchor(base, &config.vendor_folder), &[name]),
        |folder| anchor(base, &replace_vendor_folder(folder, &config.vendor_folder)),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        dependency_folder, is_falsy, read_defaults, read_dependencies, read_vendor_config,
    };
    use crate::model::VendorConfig;
    use std::path::Path;

    fn value(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn vendor_config_defaults_when_absent_or_falsy() {
        assert_eq!(
            read_vendor_config(&value("{}"), "p").unwrap().vendor_folder,
            "./vendor"
        );
        assert_eq!(
            read_vendor_config(&value(r#"{"vendorConfig": null}"#), "p")
                .unwrap()
                .vendor_folder,
            "./vendor"
        );
        assert_eq!(
            read_vendor_config(&value(r#"{"vendorConfig": {}}"#), "p")
                .unwrap()
                .vendor_folder,
            "./vendor"
        );
        assert_eq!(
            read_vendor_config(&value(r#"{"vendorConfig": {"vendorFolder": "x"}}"#), "p")
                .unwrap()
                .vendor_folder,
            "x"
        );
    }

    #[test]
    fn vendor_config_of_the_wrong_type_is_reported_with_the_reference_message() {
        assert_eq!(
            read_vendor_config(&value(r#"{"vendorConfig": "nope"}"#), "vendor.json")
                .unwrap_err()
                .to_string(),
            "Invalid vendorConfig key in vendor.json"
        );
        assert_eq!(
            read_dependencies(&value(r#"{"vendorDependencies": 7}"#), "vendor.json")
                .unwrap_err()
                .to_string(),
            "Invalid vendorDependencies key in vendor.json"
        );
    }

    #[test]
    fn defaults_prefer_the_explicit_key() {
        let d = read_defaults(&value(
            r#"{"default": {"version": "a"}, "defaultVendorOptions": {"version": "b"}}"#,
        ));
        assert_eq!(d.version.as_deref(), Some("b"));
        let d = read_defaults(&value(r#"{"default": {"version": "a"}}"#));
        assert_eq!(d.version.as_deref(), Some("a"));
    }

    #[test]
    fn falsiness_follows_javascript() {
        assert!(is_falsy(&value("null")));
        assert!(is_falsy(&value("false")));
        assert!(is_falsy(&value("0")));
        assert!(is_falsy(&value(r#""""#)));
        assert!(!is_falsy(&value("{}")));
        assert!(!is_falsy(&value("[]")));
    }

    #[test]
    fn dependency_folder_appends_the_name_only_without_an_override() {
        let config = VendorConfig {
            vendor_folder: "./vendor".to_owned(),
        };
        let config_path = Path::new("/proj/vendor.json");
        assert_eq!(
            dependency_folder(&config, config_path, None, "fzf"),
            Path::new("/proj").join("vendor").join("fzf")
        );
        assert_eq!(
            dependency_folder(&config, config_path, Some("{vendorFolder}"), "fzf"),
            Path::new("/proj").join("vendor")
        );
        assert_eq!(
            dependency_folder(&config, config_path, Some("{vendorFolder}/sub"), "fzf"),
            Path::new("/proj").join("vendor").join("sub")
        );
    }

    #[test]
    fn a_rooted_vendor_folder_is_used_as_given() {
        // Joining this onto the config's directory would give /root/root/.local/bin.
        let config = VendorConfig {
            vendor_folder: "/root/.local/bin".to_owned(),
        };
        let config_path = Path::new("/root/vendor.yml");
        let expected = Path::new("/root").join(".local").join("bin");

        // Inherited through `default.vendorFolder: '{vendorFolder}'`...
        assert_eq!(
            dependency_folder(
                &config,
                config_path,
                Some("{vendorFolder}"),
                "ls-interactive"
            ),
            expected
        );
        // ...set directly on the dependency...
        assert_eq!(
            dependency_folder(
                &config,
                config_path,
                Some("/root/.local/bin"),
                "ls-interactive"
            ),
            expected
        );
        // ...and from the global folder, which still gets the dependency name appended.
        assert_eq!(
            dependency_folder(&config, config_path, None, "ls-interactive"),
            expected.join("ls-interactive")
        );
    }

    #[test]
    fn a_rooted_folder_is_still_normalised() {
        let config = VendorConfig {
            vendor_folder: "/opt/./tools/../bin".to_owned(),
        };
        assert_eq!(
            dependency_folder(
                &config,
                Path::new("/proj/vendor.yml"),
                Some("{vendorFolder}"),
                "x"
            ),
            Path::new("/opt").join("bin")
        );
    }

    #[test]
    fn a_relative_vendor_folder_still_follows_the_config_file() {
        // The common case must not move: everything relative stays relative to the config.
        let config = VendorConfig {
            vendor_folder: "./vendor".to_owned(),
        };
        for folder in ["./vendor", "vendor", "../shared", "sub/dir"] {
            let resolved = dependency_folder(
                &config,
                Path::new("/proj/nested/vendor.yml"),
                Some(folder),
                "x",
            );
            assert!(
                resolved.starts_with("/proj"),
                "{folder} should resolve under the config directory, got {}",
                resolved.display()
            );
        }
    }
}
