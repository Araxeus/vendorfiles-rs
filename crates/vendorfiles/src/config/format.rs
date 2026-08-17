//! Config file formats: detection, parsing into a canonical value, and rendering.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Result, VendorError};

/// The three interchangeable config encodings. `.yaml` and `.yml` share one variant, matching
/// the reference's `format` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFormat {
    Toml,
    Yml,
    Json,
}

impl ConfigFormat {
    /// Picks the format from a config file name.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        let name = path.to_string_lossy();
        if name.ends_with(".toml") {
            Self::Toml
        } else if name.ends_with(".yml") || name.ends_with(".yaml") {
            Self::Yml
        } else {
            Self::Json
        }
    }

    /// Parses `text` into the canonical order-preserving value used for all typed reads.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::ParseConfig`] when the text is malformed, or holds values this
    /// format can express but the canonical model cannot (TOML datetimes, non-string keys).
    pub fn parse(self, text: &str, path: &Path) -> Result<serde_json::Value> {
        let fail = |e: &dyn std::fmt::Display| VendorError::ParseConfig {
            path: path.display().to_string(),
            message: e.to_string(),
        };
        match self {
            Self::Toml => toml_edit::de::from_str(text).map_err(|e| fail(&e)),
            Self::Yml => serde_yaml_ng::from_str(text).map_err(|e| fail(&e)),
            Self::Json => serde_json::from_str(text).map_err(|e| fail(&e)),
        }
    }
}

/// The whitespace unit used to indent a document.
///
/// Reproduces the `detect-indent` package, whose result the reference feeds straight into
/// `JSON.stringify` and `yaml.stringify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indent(String);

impl Indent {
    /// The default when a file gives no evidence: two spaces.
    #[must_use]
    pub fn default_two_spaces() -> Self {
        Self("  ".to_owned())
    }

    /// The indent string, e.g. `"    "` or `"\t"`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The width the YAML emitter would be configured with.
    #[must_use]
    pub fn width(&self) -> usize {
        self.0.chars().count().max(1)
    }

    /// Infers the indent from a document's text.
    #[must_use]
    pub fn detect(text: &str) -> Self {
        indents_map(text, true)
            .or_else(|| indents_map(text, false))
            .unwrap_or_else(Self::default_two_spaces)
    }
}

/// Leading-whitespace run of a line: `(is_space, length)`. `None` when the line has none.
fn leading_indent(line: &str) -> Option<(bool, usize)> {
    let mut chars = line.chars();
    match chars.next() {
        Some(' ') => Some((true, 1 + chars.take_while(|c| *c == ' ').count())),
        Some('\t') => Some((false, 1 + chars.take_while(|c| *c == '\t').count())),
        _ => None,
    }
}

/// The scoring loop of `detect-indent`: tally indentation *changes* between adjacent lines,
/// then take the most-used one, breaking ties by weight.
fn indents_map(text: &str, ignore_single_spaces: bool) -> Option<Indent> {
    let mut indents: HashMap<(bool, usize), (u32, u32)> = HashMap::new();
    let mut previous_size = 0usize;
    let mut previous_is_space: Option<bool> = None;
    let mut key: Option<(bool, usize)> = None;

    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        let Some((is_space, size)) = leading_indent(line) else {
            previous_size = 0;
            previous_is_space = None;
            continue;
        };
        if ignore_single_spaces && is_space && size == 1 {
            continue;
        }
        if previous_is_space != Some(is_space) {
            previous_size = 0;
        }
        previous_is_space = Some(is_space);

        let (use_count, weight) = if size == previous_size {
            (1, 1)
        } else {
            key = Some((is_space, size.abs_diff(previous_size)));
            (1, 0)
        };
        previous_size = size;

        if let Some(k) = key {
            let entry = indents.entry(k).or_insert((0, 0));
            entry.0 += use_count;
            entry.1 += weight;
        }
    }

    let (&(is_space, amount), _) =
        indents
            .iter()
            .max_by_key(|(&(is_space, amount), &(used, weight))| {
                // Deterministic tie-break so the result never depends on hash order.
                (used, weight, u32::from(is_space), amount)
            })?;
    if amount == 0 {
        return None;
    }
    Some(Indent(if is_space { " " } else { "\t" }.repeat(amount)))
}

/// Serialises a canonical value as JSON with the given indent, like `JSON.stringify`.
///
/// # Errors
///
/// Returns [`VendorError::SerializeConfig`] if the value cannot be encoded as JSON.
pub fn to_json_string(value: &serde_json::Value, indent: &Indent) -> Result<String> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_str().as_bytes());
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(value, &mut ser).map_err(|e| VendorError::SerializeConfig {
        path: "config".to_owned(),
        message: e.to_string(),
    })?;
    String::from_utf8(buf).map_err(|e| VendorError::SerializeConfig {
        path: "config".to_owned(),
        message: e.to_string(),
    })
}

/// Serialises a canonical value as YAML, in the reference emitter's block style.
///
/// # Errors
///
/// Infallible in practice; the `Result` mirrors [`to_json_string`] so callers can treat both
/// emitters alike.
pub fn to_yaml_string(value: &serde_json::Value, indent: &Indent) -> Result<String> {
    Ok(crate::config::yaml_emit::to_string(value, indent.width()))
}

/// The document's trailing newline, so it can be restored verbatim on write.
#[must_use]
pub fn final_newline(text: &str) -> String {
    if text.ends_with("\r\n") {
        "\r\n".to_owned()
    } else if text.ends_with('\n') {
        "\n".to_owned()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{final_newline, to_json_string, ConfigFormat, Indent};
    use std::path::Path;

    #[test]
    fn format_is_taken_from_the_extension() {
        assert_eq!(
            ConfigFormat::from_path(Path::new("a/vendor.toml")),
            ConfigFormat::Toml
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("a/vendor.yml")),
            ConfigFormat::Yml
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("a/vendor.yaml")),
            ConfigFormat::Yml
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("a/vendor.json")),
            ConfigFormat::Json
        );
        assert_eq!(
            ConfigFormat::from_path(Path::new("a/package.json")),
            ConfigFormat::Json
        );
    }

    #[test]
    fn indent_detection_handles_the_common_cases() {
        assert_eq!(Indent::detect("{\n    \"a\": 1\n}").as_str(), "    ");
        assert_eq!(Indent::detect("{\n\t\"a\": 1\n}").as_str(), "\t");
        assert_eq!(
            Indent::detect("{\n  \"a\": {\n    \"b\": 1\n  }\n}").as_str(),
            "  "
        );
        // No indentation at all falls back to two spaces.
        assert_eq!(Indent::detect("{}").as_str(), "  ");
    }

    #[test]
    fn json_rendering_honours_the_detected_indent() {
        let value: serde_json::Value = serde_json::from_str(r#"{"a":{"b":1}}"#).unwrap();
        assert_eq!(
            to_json_string(&value, &Indent::detect("{\n\t\"x\": 1\n}")).unwrap(),
            "{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}"
        );
    }

    #[test]
    fn final_newline_is_captured_verbatim() {
        assert_eq!(final_newline("a\n"), "\n");
        assert_eq!(final_newline("a\r\n"), "\r\n");
        assert_eq!(final_newline("a"), "");
    }

    #[test]
    fn toml_and_yaml_parse_into_the_canonical_value() {
        let toml = ConfigFormat::Toml
            .parse(
                "[vendorConfig]\nvendorFolder = './v'\n",
                Path::new("vendor.toml"),
            )
            .unwrap();
        assert_eq!(toml["vendorConfig"]["vendorFolder"], "./v");

        let yaml = ConfigFormat::Yml
            .parse(
                "vendorConfig:\n  vendorFolder: ./v\n",
                Path::new("vendor.yml"),
            )
            .unwrap();
        assert_eq!(yaml["vendorConfig"]["vendorFolder"], "./v");
    }
}
