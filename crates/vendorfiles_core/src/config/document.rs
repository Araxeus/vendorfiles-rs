//! The editable config document.
//!
//! Write-back must keep everything the tool does not touch: keys it does not model
//! (`package.json` is a config file too), comments, quoting, blank lines. Each format is edited
//! through a lossless syntax tree, so an edit rewrites only the entry it names. The mutation
//! surface is four operations wide.

use std::path::Path;

use jsonc_parser::cst::{CstInputValue, CstRootNode};

use crate::config::format::{ConfigFormat, Indent, json5_options};
use crate::config::yaml_emit;
use crate::error::{Result, VendorError};
use crate::model::RawDependency;
use serde::Serialize;

const DEPENDENCIES_KEY: &str = "vendorDependencies";

/// A parsed config file, editable in place.
///
/// JSON and YAML keep their source text and parse it into a syntax tree for each edit. Both
/// trees share nodes through `Rc`, so holding text keeps the document `Send` and makes `clone`
/// a real copy.
#[derive(Debug, Clone)]
pub enum ConfigDocument {
    /// JSON5 source, edited through `jsonc-parser`'s CST.
    Json(String),
    /// YAML source, edited through `yaml-edit`.
    Yaml(String),
    /// TOML keeps its `toml_edit` document between edits.
    Toml(Box<toml_edit::DocumentMut>),
}

impl ConfigDocument {
    /// Wraps `text` for editing in the given format.
    ///
    /// JSON and YAML are parsed when first edited, so a file the reader accepts can still be
    /// listed and synced if its syntax tree cannot be built.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::ParseConfig`] when a TOML document is malformed.
    pub fn parse(format: ConfigFormat, text: &str, path: &Path) -> Result<Self> {
        match format {
            ConfigFormat::Toml => text
                .parse::<toml_edit::DocumentMut>()
                .map(|d| Self::Toml(Box::new(d)))
                .map_err(|e| VendorError::ParseConfig {
                    path: path.display().to_string(),
                    message: e.to_string(),
                }),
            ConfigFormat::Json => Ok(Self::Json(text.to_owned())),
            ConfigFormat::Yml => Ok(Self::Yaml(text.to_owned())),
        }
    }

    /// Sets `vendorDependencies.<name>.version`.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::SerializeConfig`] when the entry cannot be edited in place, such as
    /// a YAML entry written as an alias.
    pub fn set_dependency_version(&mut self, name: &str, version: &str) -> Result<()> {
        match self {
            Self::Json(text) => edit_json(text, name, |root| {
                let entry = root
                    .object_value_or_set()
                    .object_value_or_set(DEPENDENCIES_KEY)
                    .object_value_or_set(name);
                match entry.get("version") {
                    Some(prop) => match prop.value().and_then(|v| v.as_string_lit()) {
                        Some(old) if old.raw_value().starts_with('\'') => {
                            old.set_raw_value(json5_single_quoted(version));
                        }
                        _ => prop.set_value(version.into()),
                    },
                    None => {
                        entry.append("version", version.into());
                    }
                }
                Ok(())
            }),
            Self::Yaml(text) => edit_yaml(text, name, |doc| {
                let entry = dependencies(doc)
                    .and_then(|deps| deps.get_mapping(name))
                    .ok_or_else(|| unwritable(name, "the entry is not a plain mapping"))?;
                let style = yaml_version_style(entry.get("version").as_ref(), version);
                entry.set(
                    "version",
                    yaml_edit::ScalarValue::with_style(version, style),
                );
                Ok(())
            }),
            Self::Toml(doc) => {
                let slot = &mut doc[DEPENDENCIES_KEY][name]["version"];
                match slot.as_value_mut() {
                    // Reuse the old value's decor so surrounding spacing and any trailing
                    // comment survive the bump.
                    Some(existing) => {
                        let decor = existing.decor().clone();
                        *existing = toml_version_like(existing, version);
                        *existing.decor_mut() = decor;
                    }
                    None => *slot = toml_edit::value(version),
                }
                Ok(())
            }
        }
    }

    /// Sets `vendorDependencies.<name>.<key>`, leaving the rest of the entry as written.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::SerializeConfig`] when the value cannot be encoded or the entry
    /// cannot be edited in place.
    pub fn set_dependency_value(
        &mut self,
        name: &str,
        key: &str,
        value: &impl Serialize,
    ) -> Result<()> {
        let json = serde_json::to_value(value).map_err(|e| unwritable(name, e))?;
        match self {
            Self::Json(text) => edit_json(text, name, |root| {
                let entry = root
                    .object_value_or_set()
                    .object_value_or_set(DEPENDENCIES_KEY)
                    .object_value_or_set(name);
                match entry.get(key) {
                    Some(prop) => prop.set_value(cst_value(&json)),
                    None => {
                        entry.append(key, cst_value(&json));
                    }
                }
                Ok(())
            }),
            Self::Yaml(text) => set_yaml_block(text, name, |doc| {
                let entry = dependencies(doc)
                    .and_then(|deps| deps.get_mapping(name))
                    .ok_or_else(|| unwritable(name, "the entry is not a plain mapping"))?;
                Ok((entry, key, &json))
            }),
            Self::Toml(doc) => {
                let value = value
                    .serialize(toml_edit::ser::ValueSerializer::new())
                    .map_err(|e| unwritable(name, e))?;
                let slot = &mut doc[DEPENDENCIES_KEY][name][key];
                match slot.as_value_mut() {
                    Some(existing) => {
                        let decor = existing.decor().clone();
                        *existing = value;
                        *existing.decor_mut() = decor;
                    }
                    None => *slot = toml_edit::Item::Value(value),
                }
                Ok(())
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
        let serialized = serde_json::to_value(dependency).map_err(|e| unwritable(name, e))?;
        match self {
            Self::Json(text) => edit_json(text, name, |root| {
                let deps = root
                    .object_value_or_create()
                    .ok_or_else(|| unwritable(name, "config root is not a table"))?
                    .object_value_or_set(DEPENDENCIES_KEY);
                let value = cst_value(&serialized);
                match deps.get(name) {
                    Some(prop) => prop.set_value(value),
                    None => {
                        deps.append(name, value);
                    }
                }
                Ok(())
            }),
            Self::Yaml(text) => upsert_yaml(text, name, &serialized),
            Self::Toml(doc) => {
                let table = toml_edit::ser::to_document(dependency)
                    .map_err(|e| unwritable(name, e))?
                    .as_table()
                    .clone();
                let deps = doc.entry(DEPENDENCIES_KEY).or_insert_with(|| {
                    let mut t = toml_edit::Table::new();
                    t.set_implicit(true);
                    toml_edit::Item::Table(t)
                });
                if let Some(deps) = deps.as_table_mut() {
                    deps.insert(name, toml_edit::Item::Table(table));
                } else if let Some(deps) = deps.as_inline_table_mut() {
                    let mut entry = table.into_inline_table();
                    entry.fmt();
                    deps.insert(name, toml_edit::Value::InlineTable(entry));
                } else {
                    return Err(unwritable(name, "vendorDependencies is not a table"));
                }
                Ok(())
            }
        }
    }

    /// Removes a dependency entry; a no-op if it is not there.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::SerializeConfig`] when the JSON or YAML text cannot be parsed for
    /// editing.
    pub fn remove_dependency(&mut self, name: &str) -> Result<()> {
        match self {
            Self::Json(text) => edit_json(text, name, |root| {
                if let Some(prop) = root
                    .object_value()
                    .and_then(|o| o.object_value(DEPENDENCIES_KEY))
                    .and_then(|deps| deps.get(name))
                {
                    prop.remove();
                }
                Ok(())
            }),
            Self::Yaml(text) => edit_yaml(text, name, |doc| {
                if let Some(deps) = dependencies(doc) {
                    deps.remove(name);
                }
                Ok(())
            }),
            Self::Toml(doc) => {
                if let Some(deps) = doc
                    .get_mut(DEPENDENCIES_KEY)
                    .and_then(toml_edit::Item::as_table_like_mut)
                {
                    deps.remove(name);
                }
                Ok(())
            }
        }
    }

    /// Renders the document back to text, before the file's own trailing newline is restored.
    ///
    /// Removing an entry can leave its blank lines behind at the end of the file, which would
    /// otherwise accumulate one run per uninstall. The trailing run is trimmed here and the
    /// original ending is put back on write.
    #[must_use]
    pub fn render(&self) -> String {
        let text = match self {
            Self::Toml(doc) => &doc.to_string(),
            Self::Json(text) | Self::Yaml(text) => text,
        };
        text.trim_end_matches(['\r', '\n']).to_owned()
    }
}

/// Parses JSON5 text into a CST, applies `edit`, and prints the tree back into `text`.
fn edit_json(
    text: &mut String,
    name: &str,
    edit: impl FnOnce(&CstRootNode) -> Result<()>,
) -> Result<()> {
    let root = CstRootNode::parse(text, &json5_options()).map_err(|e| unwritable(name, e))?;
    edit(&root)?;
    *text = root.to_string();
    Ok(())
}

/// Parses YAML text into a lossless tree, applies `edit`, and prints the tree back into `text`.
fn edit_yaml(
    text: &mut String,
    name: &str,
    edit: impl FnOnce(&yaml_edit::Document) -> Result<()>,
) -> Result<()> {
    let doc = yaml_document(text, name)?;
    edit(&doc)?;
    *text = doc.to_string();
    Ok(())
}

/// Writes a dependency into YAML text, in the reference emitter's style and the file's indent.
fn upsert_yaml(text: &mut String, name: &str, serialized: &serde_json::Value) -> Result<()> {
    let wrapped = serde_json::json!({ name: serialized });
    if text.trim().is_empty() {
        let width = Indent::detect(text).width();
        *text = yaml_emit::to_string(&serde_json::json!({ DEPENDENCIES_KEY: wrapped }), width);
        return Ok(());
    }

    set_yaml_block(text, name, |doc| match dependencies(doc) {
        Some(deps) if !deps.is_empty() => Ok((deps, name, serialized)),
        // An empty `{}` would take the new entry in flow style; a block mapping replaces it.
        _ => {
            let root = doc
                .as_mapping()
                .ok_or_else(|| unwritable(name, "config root is not a table"))?;
            Ok((root, DEPENDENCIES_KEY, &wrapped))
        }
    })
}

/// The `vendorDependencies` mapping, when it is one.
fn dependencies(doc: &yaml_edit::Document) -> Option<yaml_edit::Mapping> {
    doc.as_mapping()?.get_mapping(DEPENDENCIES_KEY)
}

/// Sets a key on the mapping `place` picks out to the value it gives, in the reference
/// emitter's style and the file's indent.
fn set_yaml_block<'a>(
    text: &mut String,
    name: &str,
    place: impl FnOnce(
        &yaml_edit::Document,
    ) -> Result<(yaml_edit::Mapping, &'a str, &'a serde_json::Value)>,
) -> Result<()> {
    // `yaml-edit` indents an inserted block two columns past its key, so the block is written in
    // two-space steps and widened to the file's step once it is in place.
    let width = Indent::detect(text).width();
    let mut inserted = None;
    edit_yaml(text, name, |doc| {
        let (parent, key, value) = place(doc)?;
        let block = yaml_document(&yaml_emit::to_string(value, 2), name)?;
        parent.set(key, &block);
        inserted = parent
            .get(key)
            .map(|node| (parent.detect_indentation_level(), node.byte_range()));
        Ok(())
    })?;
    if let Some((column, range)) = inserted {
        widen(
            text,
            column,
            range.start as usize..range.end as usize,
            width,
        );
    }
    Ok(())
}

/// Rescales the indent of the lines touching `range` from two-space steps to `width`-space
/// steps, measured from `column`. Lines at or left of `column` are left alone.
fn widen(text: &mut String, column: usize, range: std::ops::Range<usize>, width: usize) {
    if width == 2 {
        return;
    }
    let first_line = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let mut widened = String::new();
    for line in text[first_line..range.end].split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent > column {
            let steps = (indent - column) / 2;
            widened.push_str(&" ".repeat(column + steps * width));
            widened.push_str(&line[indent..]);
        } else {
            widened.push_str(line);
        }
    }
    text.replace_range(first_line..range.end, &widened);
}

fn yaml_document(text: &str, name: &str) -> Result<yaml_edit::Document> {
    text.parse::<yaml_edit::Document>()
        .map_err(|e| unwritable(name, e))
}

/// A JSON5 single-quoted string literal holding `text`.
fn json5_single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The quote style for a new version: the old value's, so a bump changes only the text.
/// A plain old value stays plain unless YAML would read the new text as something other than a
/// string, and then it is double-quoted, as the reference emitter does.
fn yaml_version_style(old: Option<&yaml_edit::YamlNode>, version: &str) -> yaml_edit::ScalarStyle {
    let quote = old
        .and_then(yaml_edit::YamlNode::as_scalar)
        .and_then(|scalar| scalar.value().chars().next());
    match quote {
        Some('\'') => yaml_edit::ScalarStyle::SingleQuoted,
        Some('"') => yaml_edit::ScalarStyle::DoubleQuoted,
        _ if yaml_emit::plain_or_quoted(version) == version => yaml_edit::ScalarStyle::Plain,
        _ => yaml_edit::ScalarStyle::DoubleQuoted,
    }
}

/// `version` as a TOML string, written as a literal `'...'` when the old value was one.
fn toml_version_like(old: &toml_edit::Value, version: &str) -> toml_edit::Value {
    let literal = match old {
        toml_edit::Value::String(formatted) => formatted
            .as_repr()
            .and_then(|repr| repr.as_raw().as_str())
            .is_some_and(|raw| raw.starts_with('\'')),
        _ => false,
    };
    if literal
        && !version.contains(['\'', '\n', '\r'])
        && let Ok(value) = format!("'{version}'").parse()
    {
        return value;
    }
    toml_edit::Value::from(version)
}

fn cst_value(value: &serde_json::Value) -> CstInputValue {
    match value {
        serde_json::Value::Null => CstInputValue::Null,
        serde_json::Value::Bool(b) => CstInputValue::Bool(*b),
        serde_json::Value::Number(n) => CstInputValue::Number(n.to_string()),
        serde_json::Value::String(s) => CstInputValue::String(s.clone()),
        serde_json::Value::Array(items) => {
            CstInputValue::Array(items.iter().map(cst_value).collect())
        }
        serde_json::Value::Object(map) => CstInputValue::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), cst_value(value)))
                .collect(),
        ),
    }
}

fn unwritable(name: &str, message: impl std::fmt::Display) -> VendorError {
    VendorError::SerializeConfig {
        path: name.to_owned(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigDocument;
    use crate::config::format::ConfigFormat;
    use crate::model::{FileEntry, RawDependency};
    use std::path::Path;

    fn doc(format: ConfigFormat, text: &str) -> ConfigDocument {
        ConfigDocument::parse(format, text, Path::new("vendor")).unwrap()
    }

    fn dependency() -> RawDependency {
        RawDependency {
            version: Some("v1".to_owned()),
            repository: Some("https://github.com/a/b".to_owned()),
            files: serde_json::from_str(r#"["LICENSE"]"#).unwrap(),
            ..RawDependency::default()
        }
    }

    const JSON5: &str = r#"// top
{
    name: 'pkg', // unmodelled
    "vendorDependencies": {
        /* first */
        "a": {
            "version": "v1", // pinned
            repository: 'https://github.com/a/a',
            files: ["LICENSE",],
        },
        "b": { "version": "v2" },
    },
}
"#;

    #[test]
    fn json_version_bump_touches_only_the_version() {
        let mut d = doc(ConfigFormat::Json, JSON5);
        d.set_dependency_version("a", "v1.5").unwrap();
        assert_eq!(
            d.render(),
            JSON5
                .replace(r#""v1", // pinned"#, r#""v1.5", // pinned"#)
                .trim_end()
        );
    }

    #[test]
    fn json_version_bump_keeps_the_quote_style() {
        let mut d = doc(
            ConfigFormat::Json,
            "{vendorDependencies: {a: {version: 'v1'}, b: {version: \"v1\"}}}",
        );
        d.set_dependency_version("a", "it's").unwrap();
        d.set_dependency_version("b", "v2").unwrap();
        assert_eq!(
            d.render(),
            r#"{vendorDependencies: {a: {version: 'it\'s'}, b: {version: "v2"}}}"#
        );
    }

    #[test]
    fn json_removal_keeps_comments_and_siblings() {
        let mut d = doc(ConfigFormat::Json, JSON5);
        d.remove_dependency("b").unwrap();
        assert_eq!(
            d.render(),
            JSON5
                .replace("        \"b\": { \"version\": \"v2\" },\n", "")
                .trim_end()
        );
    }

    #[test]
    fn json_upsert_follows_the_file_indent_and_trailing_commas() {
        let mut d = doc(ConfigFormat::Json, JSON5);
        d.remove_dependency("b").unwrap();
        d.upsert_dependency("c", &dependency()).unwrap();
        let out = d.render();
        let entry = r#"
        "c": {
            "version": "v1",
            "repository": "https://github.com/a/b",
            "files": ["LICENSE"],
        },
"#;
        assert!(out.contains(entry), "{out}");
        assert!(
            out.starts_with("// top\n{\n    name: 'pkg', // unmodelled\n"),
            "{out}"
        );
    }

    #[test]
    fn json_upsert_creates_the_dependencies_table() {
        let mut d = doc(ConfigFormat::Json, "{\n\t\"name\": \"pkg\"\n}\n");
        d.upsert_dependency("b", &dependency()).unwrap();
        assert_eq!(
            d.render(),
            "{\n\t\"name\": \"pkg\",\n\t\"vendorDependencies\": {\n\t\t\"b\": {\n\t\t\t\
             \"version\": \"v1\",\n\t\t\t\"repository\": \"https://github.com/a/b\",\n\t\t\t\
             \"files\": [\"LICENSE\"]\n\t\t}\n\t}\n}"
        );
    }

    #[test]
    fn json_upsert_refuses_a_root_that_is_not_an_object() {
        let mut d = doc(ConfigFormat::Json, "[1]");
        assert!(d.upsert_dependency("b", &dependency()).is_err());
        assert_eq!(d.render(), "[1]");
    }

    const YAML: &str = "# top
vendorConfig:
    vendorFolder: ./v   # folder
vendorDependencies:
    a:
        version: v1   # pinned
        files:
            - LICENSE
            - '{vendorFolder}/x'

    b:
        version: v2
        files: [README.md]
";

    /// `YAML` up to the end of dependency `a`.
    fn yaml_without_b() -> &'static str {
        YAML.split("\n\n    b:").next().unwrap()
    }

    #[test]
    fn yaml_version_bump_touches_only_the_version() {
        let mut d = doc(ConfigFormat::Yml, YAML);
        d.set_dependency_version("a", "v1.5").unwrap();
        assert_eq!(
            d.render(),
            YAML.replace("v1   # pinned", "v1.5   # pinned").trim_end()
        );
    }

    #[test]
    fn yaml_version_bump_keeps_the_quote_style() {
        let source = "vendorDependencies:\n  a:\n    version: v1\n  b:\n    version: 'v1'\n  \
                      c:\n    version: \"v1\" # pinned\n";
        let mut d = doc(ConfigFormat::Yml, source);
        for name in ["a", "b", "c"] {
            d.set_dependency_version(name, "0.7.8").unwrap();
        }
        assert_eq!(d.render(), source.replace("v1", "0.7.8").trim_end());
    }

    #[test]
    fn yaml_version_bump_quotes_a_version_that_would_not_stay_a_string() {
        let mut d = doc(
            ConfigFormat::Yml,
            "vendorDependencies:\n  a:\n    version: v1\n",
        );
        d.set_dependency_version("a", "1.0").unwrap();
        assert_eq!(
            d.render(),
            "vendorDependencies:\n  a:\n    version: \"1.0\""
        );
    }

    #[test]
    fn yaml_version_bump_refuses_an_alias() {
        let mut d = doc(
            ConfigFormat::Yml,
            "base: &b\n  version: v1\nvendorDependencies:\n  a: *b\n",
        );
        assert!(d.set_dependency_version("a", "v2").is_err());
    }

    #[test]
    fn yaml_removal_drops_only_the_named_dependency() {
        let mut d = doc(ConfigFormat::Yml, YAML);
        d.remove_dependency("b").unwrap();
        assert_eq!(d.render(), yaml_without_b());
    }

    #[test]
    fn yaml_upsert_uses_the_file_indent() {
        let mut d = doc(ConfigFormat::Yml, YAML);
        d.remove_dependency("b").unwrap();
        d.upsert_dependency("c", &dependency()).unwrap();
        let entry = "
    c:
        version: v1
        repository: https://github.com/a/b
        files:
            - LICENSE";
        assert_eq!(d.render(), format!("{}{entry}", yaml_without_b()));
    }

    #[test]
    fn yaml_upsert_replaces_an_empty_flow_mapping_with_a_block() {
        let mut d = doc(ConfigFormat::Yml, "# keep\nvendorDependencies: {}\n");
        d.upsert_dependency("b", &dependency()).unwrap();
        assert_eq!(
            d.render(),
            "# keep\nvendorDependencies:\n  b:\n    version: v1\n    \
             repository: https://github.com/a/b\n    files:\n      - LICENSE"
        );
    }

    #[test]
    fn yaml_upsert_creates_the_dependencies_table_in_the_file_indent() {
        let mut d = doc(ConfigFormat::Yml, "vendorConfig:\n    vendorFolder: x\n");
        d.upsert_dependency("b", &dependency()).unwrap();
        assert_eq!(
            d.render(),
            "vendorConfig:\n    vendorFolder: x\nvendorDependencies:\n    b:\n        \
             version: v1\n        repository: https://github.com/a/b\n        files:\n            \
             - LICENSE"
        );
    }

    #[test]
    fn yaml_upsert_into_an_empty_file() {
        let mut d = doc(ConfigFormat::Yml, "");
        d.upsert_dependency("b", &dependency()).unwrap();
        assert!(
            d.render()
                .starts_with("vendorDependencies:\n  b:\n    version: v1\n")
        );
    }

    #[test]
    fn toml_version_bump_keeps_comments() {
        let mut d = doc(
            ConfigFormat::Toml,
            "# keep me\n[vendorDependencies.a]\nversion = \"v1\" # and me\nrepository = \"r\"\n",
        );
        d.set_dependency_version("a", "v2").unwrap();
        assert_eq!(
            d.render(),
            "# keep me\n[vendorDependencies.a]\nversion = \"v2\" # and me\nrepository = \"r\""
        );
    }

    #[test]
    fn toml_version_bump_keeps_the_string_kind() {
        let source =
            "[vendorDependencies.a]\nversion = 'v1'\n[vendorDependencies.b]\nversion = \"v1\"\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.set_dependency_version("a", "v2").unwrap();
        d.set_dependency_version("b", "v2").unwrap();
        assert_eq!(d.render(), source.replace("v1", "v2").trim_end());
    }

    #[test]
    fn toml_removal_does_not_leave_a_growing_run_of_blank_lines() {
        let source = "[vendorDependencies.a]\nversion = \"v1\"\n\n[vendorDependencies.b]\nversion = \"v2\"\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.remove_dependency("a").unwrap();
        d.remove_dependency("b").unwrap();
        assert_eq!(d.render(), "");
    }

    fn readme() -> Vec<FileEntry> {
        serde_json::from_str(r#"["README.md"]"#).unwrap()
    }

    #[test]
    fn json_files_change_touches_only_the_files() {
        let mut d = doc(ConfigFormat::Json, JSON5);
        d.set_dependency_value("a", "files", &readme()).unwrap();
        assert_eq!(
            d.render(),
            JSON5
                .replace(r#"files: ["LICENSE",]"#, r#"files: ["README.md"]"#)
                .trim_end()
        );
    }

    #[test]
    fn json_files_are_added_to_an_entry_without_them() {
        let mut d = doc(ConfigFormat::Json, JSON5);
        d.set_dependency_value("b", "files", &readme()).unwrap();
        let out = d.render();
        assert!(out.contains(r#""files": ["README.md"]"#), "{out}");
        assert!(out.contains(r#""version": "v1", // pinned"#), "{out}");
    }

    #[test]
    fn yaml_files_change_touches_only_the_files() {
        let mut d = doc(ConfigFormat::Yml, YAML);
        d.set_dependency_value("a", "files", &readme()).unwrap();
        assert_eq!(
            d.render(),
            YAML.replace(
                "            - LICENSE\n            - '{vendorFolder}/x'\n",
                "            - README.md\n"
            )
            .trim_end()
        );
    }

    #[test]
    fn toml_files_change_keeps_unmodelled_keys_and_comments() {
        let source = "[vendorDependencies.a]\nversion = \"v1\" # pinned\nfiles = [\"LICENSE\"]\n\
                      custom = 1\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.set_dependency_value("a", "files", &readme()).unwrap();
        assert_eq!(
            d.render(),
            source.replace("LICENSE", "README.md").trim_end()
        );
    }

    #[test]
    fn toml_files_change_in_an_inline_table() {
        let source = "vendorDependencies = { a = { version = \"v1\", files = [\"LICENSE\"] } }\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.set_dependency_value("a", "files", &readme()).unwrap();
        assert_eq!(
            d.render(),
            source.replace("LICENSE", "README.md").trim_end()
        );
    }

    #[test]
    fn yaml_value_is_added_to_an_entry() {
        let mut d = doc(ConfigFormat::Yml, YAML);
        d.set_dependency_value("b", "repository", &"https://github.com/a/b")
            .unwrap();
        let out = d.render();
        assert!(
            out.ends_with("files: [README.md]\n        repository: https://github.com/a/b"),
            "{out}"
        );
    }

    #[test]
    fn toml_removal_from_an_inline_table() {
        let source =
            "vendorDependencies = { a = { version = \"v1\" }, b = { version = \"v2\" } }\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.remove_dependency("a").unwrap();
        assert_eq!(
            d.render(),
            "vendorDependencies = { b = { version = \"v2\" } }"
        );
    }

    #[test]
    fn toml_upsert_into_an_inline_table() {
        let source = "vendorDependencies = { a = { version = \"v1\" } }\n";
        let mut d = doc(ConfigFormat::Toml, source);
        d.upsert_dependency("b", &dependency()).unwrap();
        let out = d.render();
        assert!(out.contains("a = { version = \"v1\" }"), "{out}");
        assert!(out.contains("b = { version = \"v1\""), "{out}");
        let parsed: toml_edit::DocumentMut = out.parse().unwrap();
        assert_eq!(
            parsed["vendorDependencies"]["b"]["files"][0].as_str(),
            Some("LICENSE")
        );
    }

    #[test]
    fn toml_upsert_writes_a_new_table() {
        let mut d = doc(ConfigFormat::Toml, "");
        d.upsert_dependency("b", &dependency()).unwrap();
        let rendered = d.render();
        assert!(rendered.contains("[vendorDependencies.b]"), "{rendered}");
        assert!(rendered.contains("version = \"v1\""), "{rendered}");
    }
}
