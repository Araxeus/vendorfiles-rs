//! The data model shared by the config file, the lockfile and the install pipeline.
//!
//! Config files are user-authored documents that the reference tool validates late and
//! leniently: a key of the wrong type is ignored until the command that needs it complains.
//! That is reproduced here by deserialising every field through [`de_lenient`], which turns a
//! type mismatch into `None` instead of a hard parse failure, and by keeping validation in
//! [`RawDependency::validate`] where the reference puts it.

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Result, VendorError};
use crate::template::{basename, is_github_url, owner_and_name_from_repo_url};

/// `owner`/`name` pair extracted from a GitHub repository URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repository {
    pub owner: String,
    pub name: String,
}

impl std::fmt::Display for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Global `vendorConfig` block.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VendorConfig {
    #[serde(rename = "vendorFolder")]
    pub vendor_folder: String,
}

impl Default for VendorConfig {
    fn default() -> Self {
        Self {
            vendor_folder: "./vendor".to_owned(),
        }
    }
}

/// Where a downloaded input lands.
///
/// A plain string renames the file; a list or map means the input is an archive and the
/// entries name paths *inside* it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FileTarget {
    /// `"LICENSE": "COPYING"` — save the input under a different name.
    Rename(String),
    /// `"archive.zip": ["a", "b"]` — extract these entries, keeping their names.
    ExtractList(Vec<String>),
    /// `"archive.zip": { "a": "b" }` — extract these entries under new names.
    ExtractMap(IndexMap<String, String>),
}

impl FileTarget {
    /// The archive members this target selects, as input→output pairs.
    ///
    /// Returns `None` for [`FileTarget::Rename`], which is not an extraction.
    #[must_use]
    pub fn extraction_pairs(&self) -> Option<Vec<(String, String)>> {
        match self {
            Self::Rename(_) => None,
            Self::ExtractList(list) => Some(list.iter().map(|e| (e.clone(), e.clone())).collect()),
            Self::ExtractMap(map) => {
                Some(map.iter().map(|(i, o)| (i.clone(), o.clone())).collect())
            }
        }
    }
}

/// One element of a dependency's `files` array.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FileEntry {
    /// `"dist/coloris.min.js"` — save under its basename.
    Simple(String),
    /// `{ "dist/coloris.min.js": "coloris.js" }` — one or more explicit input→output pairs.
    Mapped(IndexMap<String, FileTarget>),
}

/// A single input→output pair, after flattening the `files` array.
///
/// Mirrors the reference implementation's `flatFilesArray`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub input: String,
    pub output: FileTarget,
}

/// Flattens a `files` array into ordered input→output pairs.
#[must_use]
pub fn flatten_files(files: &[FileEntry]) -> Vec<FileSpec> {
    let mut out = Vec::with_capacity(files.len());
    for entry in files {
        match entry {
            FileEntry::Simple(input) => out.push(FileSpec {
                output: FileTarget::Rename(basename(input)),
                input: input.clone(),
            }),
            FileEntry::Mapped(map) => out.extend(map.iter().map(|(input, output)| FileSpec {
                input: input.clone(),
                output: output.clone(),
            })),
        }
    }
    out
}

/// `hashVersionFile`: either `true` (use the first declared file) or an explicit path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum HashVersionFile {
    Flag(bool),
    Path(String),
}

/// Deserialises a field, treating a type mismatch as "absent".
///
/// The input is always a `serde_json::Value` tree (every config format is normalised to one
/// at load time), so round-tripping through `Value` here is exact, not lossy.
fn de_lenient<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    Ok(serde_json::from_value(value).ok())
}

/// Like [`de_lenient`], but accepts a bare string as shorthand for a single-element array.
fn de_files<'de, D>(deserializer: D) -> std::result::Result<Option<Vec<FileEntry>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<FileEntry>),
    }

    let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    Ok(match serde_json::from_value::<OneOrMany>(value).ok() {
        None => None,
        Some(OneOrMany::One(single)) => Some(vec![FileEntry::Simple(single)]),
        Some(OneOrMany::Many(list)) => Some(list),
    })
}

/// Options shared by every dependency via the `default` / `defaultVendorOptions` block.
///
/// The reference merges *whatever keys are present*, so every dependency key is mirrored here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultOptions {
    #[serde(default, deserialize_with = "de_lenient")]
    pub repository: Option<String>,
    #[serde(default, deserialize_with = "de_files")]
    pub files: Option<Vec<FileEntry>>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub version: Option<String>,
    #[serde(rename = "hashVersionFile", default, deserialize_with = "de_lenient")]
    pub hash_version_file: Option<HashVersionFile>,
    #[serde(rename = "vendorFolder", default, deserialize_with = "de_lenient")]
    pub vendor_folder: Option<String>,
    #[serde(rename = "releaseRegex", default, deserialize_with = "de_lenient")]
    pub release_regex: Option<String>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub locked: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub name: Option<String>,
}

/// A dependency exactly as written in the config file.
///
/// Every field is optional because the reference tool only validates on `sync`; other code
/// paths tolerate — and sometimes rely on — missing keys.
///
/// Field order is the order a newly written dependency appears in the config file, and is
/// chosen to match the reference project's own examples (`version`, `repository`, `files`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RawDependency {
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub version: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub repository: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_files",
        skip_serializing_if = "Option::is_none"
    )]
    pub files: Option<Vec<FileEntry>>,
    #[serde(
        rename = "hashVersionFile",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub hash_version_file: Option<HashVersionFile>,
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        rename = "vendorFolder",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub vendor_folder: Option<String>,
    #[serde(
        rename = "releaseRegex",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_regex: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub locked: Option<bool>,
}

impl RawDependency {
    /// Fills unset fields from the config's `default` block, matching the reference's `??=`.
    pub fn apply_defaults(&mut self, defaults: &DefaultOptions) {
        macro_rules! fill {
            ($($field:ident),+ $(,)?) => {$(
                if self.$field.is_none() {
                    self.$field.clone_from(&defaults.$field);
                }
            )+};
        }
        fill!(
            repository,
            files,
            version,
            hash_version_file,
            vendor_folder,
            release_regex,
            locked,
            name,
        );
    }

    /// Reproduces `validateVendorDependency`, including its exact messages.
    ///
    /// # Errors
    ///
    /// Returns the `Invalid*Key` variant naming the offending config key.
    pub fn validate(&self, name: &str) -> Result<()> {
        if !self.repository.as_deref().is_some_and(is_github_url) {
            return Err(VendorError::InvalidRepositoryKey {
                name: name.to_owned(),
            });
        }
        if self.files.as_ref().is_none_or(Vec::is_empty) {
            return Err(VendorError::InvalidFilesKey {
                name: name.to_owned(),
            });
        }
        if let Some(regex) = self.release_regex.as_deref() {
            // `fancy_regex`, not `regex`: users write JavaScript patterns, which may use
            // lookaround (the reference README suggests `^v(?!.*-(?:alpha|beta)).*`).
            if !regex.is_empty() && fancy_regex::Regex::new(regex).is_err() {
                return Err(VendorError::InvalidReleaseRegexKey {
                    name: name.to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Turns a config entry into an installable [`Dependency`].
    ///
    /// `fallback_name` is the config key; the reference prefers it over the repository name
    /// and only consults `name` when the entry did not come from the dependency map.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::InvalidGitHubUrl`] when `repository` is missing or unparseable.
    pub fn resolve(&self, fallback_name: &str) -> Result<Dependency> {
        let repository = self
            .repository
            .clone()
            .ok_or_else(|| VendorError::InvalidGitHubUrl("undefined".to_owned()))?;
        let repo = owner_and_name_from_repo_url(&repository)?;
        let name = if fallback_name.is_empty() {
            self.name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| repo.name.clone())
        } else {
            fallback_name.to_owned()
        };

        Ok(Dependency {
            name,
            repo,
            repository,
            files: self.files.clone().unwrap_or_default(),
            version: self.version.clone().filter(|v| !v.is_empty()),
            hash_version_file: self.hash_version_file.clone(),
            vendor_folder: self.vendor_folder.clone(),
            release_regex: self.release_regex.clone().filter(|r| !r.is_empty()),
            locked: self.locked.unwrap_or(false),
        })
    }
}

/// A dependency with everything the install pipeline needs, owned outright.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub repo: Repository,
    pub repository: String,
    pub files: Vec<FileEntry>,
    pub version: Option<String>,
    pub hash_version_file: Option<HashVersionFile>,
    pub vendor_folder: Option<String>,
    pub release_regex: Option<String>,
    pub locked: bool,
}

impl Dependency {
    /// The `hashVersionFile` path to track, resolving `true` to the first declared file.
    ///
    /// `None` means "track releases instead".
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::InvalidHashVersionFile`] or
    /// [`VendorError::InvalidHashVersionFileTarget`] when `true` cannot be resolved to a file.
    pub fn hash_version_target(&self) -> Result<Option<String>> {
        match &self.hash_version_file {
            None | Some(HashVersionFile::Flag(false)) => Ok(None),
            Some(HashVersionFile::Path(path)) if path.is_empty() => Ok(None),
            Some(HashVersionFile::Path(path)) => Ok(Some(path.clone())),
            Some(HashVersionFile::Flag(true)) => match self.files.first() {
                Some(FileEntry::Simple(first)) => Ok(Some(first.clone())),
                Some(FileEntry::Mapped(map)) => map
                    .keys()
                    .next()
                    .cloned()
                    .map(Some)
                    .ok_or(VendorError::InvalidHashVersionFile),
                None => Err(VendorError::InvalidHashVersionFileTarget("undefined")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{flatten_files, FileEntry, FileTarget, HashVersionFile, RawDependency};

    fn dep(json: &str) -> RawDependency {
        serde_json::from_str(json).expect("valid dependency")
    }

    #[test]
    fn files_accepts_a_bare_string() {
        let d = dep(r#"{"files": "a/b.txt"}"#);
        assert_eq!(
            d.files.as_deref(),
            Some(&[FileEntry::Simple("a/b.txt".to_owned())][..])
        );
    }

    #[test]
    fn file_targets_discriminate_by_shape() {
        let d = dep(r#"{"files": [{"x": "y", "a.zip": ["one"], "b.zip": {"in": "out"}}]}"#);
        let FileEntry::Mapped(map) = &d.files.as_ref().unwrap()[0] else {
            panic!("expected a mapped entry");
        };
        assert_eq!(map["x"], FileTarget::Rename("y".to_owned()));
        assert_eq!(
            map["a.zip"],
            FileTarget::ExtractList(vec!["one".to_owned()])
        );
        let FileTarget::ExtractMap(inner) = &map["b.zip"] else {
            panic!("expected an extract map");
        };
        assert_eq!(inner["in"], "out");
    }

    #[test]
    fn flatten_preserves_declaration_order() {
        let d = dep(r#"{"files": ["a/one.txt", {"b": "c", "d": "e"}]}"#);
        let flat = flatten_files(d.files.as_ref().unwrap());
        let inputs: Vec<_> = flat.iter().map(|f| f.input.as_str()).collect();
        assert_eq!(inputs, ["a/one.txt", "b", "d"]);
        assert_eq!(flat[0].output, FileTarget::Rename("one.txt".to_owned()));
    }

    #[test]
    fn wrong_types_degrade_to_absent_rather_than_failing_to_parse() {
        let d = dep(r#"{"repository": 42, "files": {"not": "an array"}, "locked": "yes"}"#);
        assert!(d.repository.is_none());
        assert!(d.files.is_none());
        assert!(d.locked.is_none());
    }

    #[test]
    fn validate_reports_the_reference_messages() {
        let d = dep(r#"{"files": ["a"]}"#);
        assert_eq!(
            d.validate("Foo").unwrap_err().to_string(),
            "config key 'vendorDependencies.Foo.repository' is not a valid github url"
        );

        let d = dep(r#"{"repository": "https://github.com/a/b", "files": []}"#);
        assert_eq!(
            d.validate("Foo").unwrap_err().to_string(),
            "config key 'vendorDependencies.Foo.files' is not a valid array"
        );

        let d =
            dep(r#"{"repository": "https://github.com/a/b", "files": ["x"], "releaseRegex": "("}"#);
        assert_eq!(
            d.validate("Foo").unwrap_err().to_string(),
            "config key 'vendorDependencies.Foo.releaseRegex' must be a valid regex string"
        );
    }

    #[test]
    fn hash_version_target_resolves_true_to_the_first_file() {
        let d = dep(
            r#"{"repository":"https://github.com/a/b","files":["dist/x.js","y"],"hashVersionFile":true}"#,
        );
        let resolved = d.resolve("b").unwrap();
        assert_eq!(
            resolved.hash_version_target().unwrap().as_deref(),
            Some("dist/x.js")
        );

        let d = dep(
            r#"{"repository":"https://github.com/a/b","files":[{"k":"v"}],"hashVersionFile":true}"#,
        );
        let resolved = d.resolve("b").unwrap();
        assert_eq!(
            resolved.hash_version_target().unwrap().as_deref(),
            Some("k")
        );
    }

    #[test]
    fn hash_version_flag_false_means_track_releases() {
        let d =
            dep(r#"{"repository":"https://github.com/a/b","files":["x"],"hashVersionFile":false}"#);
        assert_eq!(d.hash_version_file, Some(HashVersionFile::Flag(false)));
        assert!(d
            .resolve("b")
            .unwrap()
            .hash_version_target()
            .unwrap()
            .is_none());
    }

    #[test]
    fn defaults_only_fill_absent_fields() {
        let mut d = dep(r#"{"version": "v1"}"#);
        let defaults: super::DefaultOptions = serde_json::from_str(
            r#"{"repository": "https://github.com/a/b", "version": "v9", "hashVersionFile": true}"#,
        )
        .unwrap();
        d.apply_defaults(&defaults);
        assert_eq!(d.version.as_deref(), Some("v1"));
        assert_eq!(d.repository.as_deref(), Some("https://github.com/a/b"));
        assert_eq!(d.hash_version_file, Some(HashVersionFile::Flag(true)));
    }
}
