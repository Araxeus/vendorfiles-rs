//! Config file formats: detection, parsing into a canonical value, and indent detection.

use std::collections::HashMap;
use std::path::Path;

use crate::error::{Result, VendorError};

/// The three interchangeable config encodings. `.yaml` and `.yml` share one variant, matching
/// the reference's `format` field. `.json` and `.json5` share `Json`, because every JSON config
/// is read as JSON5.
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
            Self::Json => {
                jsonc_parser::parse_to_serde_value(text, &json5_options()).map_err(|e| fail(&e))
            }
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
            .max_by_key(|&(&(is_space, amount), &(used, weight))| {
                // Deterministic tie-break so the result never depends on hash order.
                (used, weight, u32::from(is_space), amount)
            })?;
    if amount == 0 {
        return None;
    }
    Some(Indent(if is_space { " " } else { "\t" }.repeat(amount)))
}

/// Parser options for JSON configs. The defaults accept all of JSON5.
#[must_use]
pub fn json5_options() -> jsonc_parser::ParseOptions {
    jsonc_parser::ParseOptions::default()
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
    use super::{ConfigFormat, Indent, final_newline};
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
            ConfigFormat::from_path(Path::new("a/vendor.json5")),
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

    #[test]
    fn json_is_read_as_json5() {
        let value = ConfigFormat::Json
            .parse(
                "// note
{vendorConfig: {vendorFolder: './v',}, /* c */ n: 0x10,}",
                Path::new("vendor.json"),
            )
            .unwrap();
        assert_eq!(value["vendorConfig"]["vendorFolder"], "./v");
        assert_eq!(value["n"], 16);
    }

    #[test]
    fn json_keeps_key_order() {
        let value = ConfigFormat::Json
            .parse(r#"{"b": 1, "a": 2}"#, Path::new("vendor.json"))
            .unwrap();
        let keys: Vec<_> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["b", "a"]);
    }
}
