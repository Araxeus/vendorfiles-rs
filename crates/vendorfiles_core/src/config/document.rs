//! The editable config document.
//!
//! Write-back must round-trip keys the tool does not model - `package.json` is a config file
//! too - so the whole document is kept, not a projection of it. The mutation surface is
//! deliberately three operations wide, which is what keeps the two backing representations
//! (structural value for JSON/YAML, `toml_edit` for TOML) honest.

use std::path::Path;

use crate::config::format::{ConfigFormat, Indent, to_json_string, to_yaml_string};
use crate::error::{Result, VendorError};
use crate::model::RawDependency;

const DEPENDENCIES_KEY: &str = "vendorDependencies";

/// A parsed config file, editable in place.
#[derive(Debug, Clone)]
pub enum ConfigDocument {
    /// JSON and YAML round-trip through an order-preserving JSON value.
    Structural(serde_json::Value),
    /// TOML keeps its `toml_edit` document so comments and layout survive a version bump.
    Toml(Box<toml_edit::DocumentMut>),
}

impl ConfigDocument {
    /// Parses `text` in the given format.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::ParseConfig`] when the text is not valid in `format`.
    pub fn parse(format: ConfigFormat, text: &str, path: &Path) -> Result<Self> {
        match format {
            ConfigFormat::Toml => text
                .parse::<toml_edit::DocumentMut>()
                .map(|d| Self::Toml(Box::new(d)))
                .map_err(|e| VendorError::ParseConfig {
                    path: path.display().to_string(),
                    message: e.to_string(),
                }),
            ConfigFormat::Yml | ConfigFormat::Json => {
                format.parse(text, path).map(Self::Structural)
            }
        }
    }

    /// Sets `vendorDependencies.<name>.version`, creating the table if it is missing.
    pub fn set_dependency_version(&mut self, name: &str, version: &str) {
        match self {
            Self::Structural(value) => {
                let entry = structural_dependency_mut(value, name);
                entry["version"] = serde_json::Value::String(version.to_owned());
            }
            Self::Toml(doc) => {
                let slot = &mut doc[DEPENDENCIES_KEY][name]["version"];
                match slot.as_value_mut() {
                    // Reuse the old value's decor so surrounding spacing and any trailing
                    // comment survive the bump.
                    Some(existing) => {
                        let decor = existing.decor().clone();
                        *existing = toml_edit::Value::from(version);
                        *existing.decor_mut() = decor;
                    }
                    None => *slot = toml_edit::value(version),
                }
            }
        }
    }

    /// Inserts or replaces a whole dependency entry.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::SerializeConfig`] if the entry cannot be encoded, or if the
    /// document's root is not a table.
    pub fn upsert_dependency(&mut self, name: &str, dependency: &RawDependency) -> Result<()> {
        match self {
            Self::Structural(value) => {
                let serialized =
                    serde_json::to_value(dependency).map_err(|e| VendorError::SerializeConfig {
                        path: name.to_owned(),
                        message: e.to_string(),
                    })?;
                let deps = value
                    .as_object_mut()
                    .and_then(|o| {
                        o.entry(DEPENDENCIES_KEY)
                            .or_insert_with(|| serde_json::json!({}))
                            .as_object_mut()
                    })
                    .ok_or_else(|| VendorError::SerializeConfig {
                        path: name.to_owned(),
                        message: "config root is not a table".to_owned(),
                    })?;
                deps.insert(name.to_owned(), serialized);
            }
            Self::Toml(doc) => {
                let table = toml_edit::ser::to_document(dependency)
                    .map_err(|e| VendorError::SerializeConfig {
                        path: name.to_owned(),
                        message: e.to_string(),
                    })?
                    .as_table()
                    .clone();
                let deps = doc
                    .entry(DEPENDENCIES_KEY)
                    .or_insert_with(|| {
                        let mut t = toml_edit::Table::new();
                        t.set_implicit(true);
                        toml_edit::Item::Table(t)
                    })
                    .as_table_mut()
                    .ok_or_else(|| VendorError::SerializeConfig {
                        path: name.to_owned(),
                        message: "vendorDependencies is not a table".to_owned(),
                    })?;
                deps.insert(name, toml_edit::Item::Table(table));
            }
        }
        Ok(())
    }

    /// Removes a dependency entry; a no-op if it is not there.
    pub fn remove_dependency(&mut self, name: &str) {
        match self {
            Self::Structural(value) => {
                if let Some(deps) = value
                    .as_object_mut()
                    .and_then(|o| o.get_mut(DEPENDENCIES_KEY))
                    .and_then(serde_json::Value::as_object_mut)
                {
                    deps.shift_remove(name);
                }
            }
            Self::Toml(doc) => {
                if let Some(deps) = doc
                    .get_mut(DEPENDENCIES_KEY)
                    .and_then(toml_edit::Item::as_table_mut)
                {
                    deps.remove(name);
                }
            }
        }
    }

    /// Renders the document back to text, before the file's own trailing newline is restored.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::SerializeConfig`] if the document cannot be encoded.
    pub fn render(&self, format: ConfigFormat, indent: &Indent) -> Result<String> {
        match (self, format) {
            // Removing a table leaves its leading blank line behind, which would otherwise
            // accumulate one per uninstall. Collapse the trailing run to a single newline,
            // which is also what the reference's TOML emitter produces.
            (Self::Toml(doc), _) => Ok(format!("{}\n", doc.to_string().trim_end_matches('\n'))),
            (Self::Structural(value), ConfigFormat::Json) => to_json_string(value, indent),
            (Self::Structural(value), ConfigFormat::Yml | ConfigFormat::Toml) => {
                to_yaml_string(value, indent)
            }
        }
    }
}

/// Returns `vendorDependencies.<name>` as a mutable object, creating anything missing.
///
/// Mirrors `toml_edit`'s auto-vivifying `Index` impl so both branches behave alike.
fn structural_dependency_mut<'a>(
    value: &'a mut serde_json::Value,
    name: &str,
) -> &'a mut serde_json::Value {
    fn object_at<'a>(parent: &'a mut serde_json::Value, key: &str) -> &'a mut serde_json::Value {
        if !parent.is_object() {
            *parent = serde_json::json!({});
        }
        let slot = parent
            .as_object_mut()
            .expect("just ensured object")
            .entry(key)
            .or_insert_with(|| serde_json::json!({}));
        if !slot.is_object() {
            *slot = serde_json::json!({});
        }
        slot
    }

    let deps = object_at(value, DEPENDENCIES_KEY);
    object_at(deps, name)
}

#[cfg(test)]
mod tests {
    use super::ConfigDocument;
    use crate::config::format::{ConfigFormat, Indent};
    use crate::model::RawDependency;
    use std::path::Path;

    fn doc(format: ConfigFormat, text: &str) -> ConfigDocument {
        ConfigDocument::parse(format, text, Path::new("vendor")).unwrap()
    }

    #[test]
    fn json_version_bump_preserves_unrelated_keys_and_order() {
        let mut d = doc(
            ConfigFormat::Json,
            r#"{"name":"pkg","vendorDependencies":{"a":{"version":"v1","repository":"r"}}}"#,
        );
        d.set_dependency_version("a", "v2");
        let out = d
            .render(ConfigFormat::Json, &Indent::default_two_spaces())
            .unwrap();
        assert_eq!(
            out,
            "{\n  \"name\": \"pkg\",\n  \"vendorDependencies\": {\n    \"a\": {\n      \
             \"version\": \"v2\",\n      \"repository\": \"r\"\n    }\n  }\n}"
        );
    }

    #[test]
    fn toml_version_bump_keeps_comments() {
        let mut d = doc(
            ConfigFormat::Toml,
            "# keep me\n[vendorDependencies.a]\nversion = \"v1\" # and me\nrepository = \"r\"\n",
        );
        d.set_dependency_version("a", "v2");
        let out = d
            .render(ConfigFormat::Toml, &Indent::default_two_spaces())
            .unwrap();
        assert_eq!(
            out,
            "# keep me\n[vendorDependencies.a]\nversion = \"v2\" # and me\nrepository = \"r\"\n"
        );
    }

    #[test]
    fn yaml_removal_drops_only_the_named_dependency() {
        let mut d = doc(
            ConfigFormat::Yml,
            "vendorConfig:\n  vendorFolder: ./v\nvendorDependencies:\n  a:\n    version: v1\n  b:\n    version: v2\n",
        );
        d.remove_dependency("a");
        let out = d
            .render(ConfigFormat::Yml, &Indent::default_two_spaces())
            .unwrap();
        assert_eq!(
            out,
            "vendorConfig:\n  vendorFolder: ./v\nvendorDependencies:\n  b:\n    version: v2\n"
        );
    }

    #[test]
    fn toml_removal_does_not_leave_a_growing_run_of_blank_lines() {
        let source = "[vendorDependencies.a]\nversion = \"v1\"\n\n[vendorDependencies.b]\nversion = \"v2\"\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.remove_dependency("a");
        d.remove_dependency("b");
        assert_eq!(
            d.render(ConfigFormat::Toml, &Indent::default_two_spaces())
                .unwrap(),
            "\n"
        );
    }

    #[test]
    fn upsert_writes_a_new_dependency_in_every_format() {
        let dep = RawDependency {
            version: Some("v1".to_owned()),
            repository: Some("https://github.com/a/b".to_owned()),
            files: serde_json::from_str(r#"["LICENSE"]"#).unwrap(),
            ..RawDependency::default()
        };

        let mut json = doc(ConfigFormat::Json, "{}");
        json.upsert_dependency("b", &dep).unwrap();
        assert_eq!(
            json.render(ConfigFormat::Json, &Indent::default_two_spaces())
                .unwrap(),
            "{\n  \"vendorDependencies\": {\n    \"b\": {\n      \"version\": \"v1\",\n      \
             \"repository\": \"https://github.com/a/b\",\n      \"files\": [\n        \
             \"LICENSE\"\n      ]\n    }\n  }\n}"
        );

        let mut toml = doc(ConfigFormat::Toml, "");
        toml.upsert_dependency("b", &dep).unwrap();
        let rendered = toml
            .render(ConfigFormat::Toml, &Indent::default_two_spaces())
            .unwrap();
        assert!(rendered.contains("[vendorDependencies.b]"), "{rendered}");
        assert!(rendered.contains("version = \"v1\""), "{rendered}");
    }
}
