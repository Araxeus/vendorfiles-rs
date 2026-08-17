//! A YAML emitter that reproduces the reference tool's output byte for byte.
//!
//! The reference serialises with the `yaml` npm package, whose block style differs from every
//! Rust YAML crate in two visible ways: block sequences are indented under their key, and a
//! scalar that cannot be written plainly is double-quoted. Config files are rewritten on every
//! version bump, so getting this wrong would reformat a user's file on first use.

use std::fmt::Write as _;

use serde_json::Value;

/// Renders a canonical config value as YAML, ending with a newline.
#[must_use]
pub fn to_string(value: &Value, indent: usize) -> String {
    let indent = indent.max(1);
    let mut out = String::new();
    match value {
        Value::Object(map) if !map.is_empty() => write_mapping(&mut out, map, 0, indent),
        Value::Array(items) if !items.is_empty() => write_sequence(&mut out, items, 0, indent),
        scalar => {
            let _ = writeln!(out, "{}", scalar_text(scalar));
        }
    }
    out
}

fn write_mapping(out: &mut String, map: &serde_json::Map<String, Value>, at: usize, step: usize) {
    for (key, value) in map {
        let pad = " ".repeat(at);
        let key = plain_or_quoted(key);
        match value {
            Value::Object(inner) if !inner.is_empty() => {
                let _ = writeln!(out, "{pad}{key}:");
                write_mapping(out, inner, at + step, step);
            }
            Value::Array(items) if !items.is_empty() => {
                let _ = writeln!(out, "{pad}{key}:");
                write_sequence(out, items, at + step, step);
            }
            scalar => {
                let _ = writeln!(out, "{pad}{key}: {}", scalar_text(scalar));
            }
        }
    }
}

fn write_sequence(out: &mut String, items: &[Value], at: usize, step: usize) {
    let pad = " ".repeat(at);
    for item in items {
        match item {
            Value::Object(map) if !map.is_empty() => {
                // The first key shares the dash's line; the rest align past it.
                let mut nested = String::new();
                write_mapping(&mut nested, map, at + 2, step);
                let _ = write!(out, "{pad}- {}", nested.trim_start_matches(' '));
            }
            Value::Array(inner) if !inner.is_empty() => {
                let mut nested = String::new();
                write_sequence(&mut nested, inner, at + 2, step);
                let _ = write!(out, "{pad}- {}", nested.trim_start_matches(' '));
            }
            scalar => {
                let _ = writeln!(out, "{pad}- {}", scalar_text(scalar));
            }
        }
    }
}

/// Renders a leaf value, including the empty collections that stay in flow style.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => plain_or_quoted(s),
        Value::Array(_) => "[]".to_owned(),
        Value::Object(_) => "{}".to_owned(),
    }
}

/// Writes a string plainly when YAML would read it back unchanged, else double-quoted.
#[must_use]
pub fn plain_or_quoted(text: &str) -> String {
    if is_plain_safe(text) {
        text.to_owned()
    } else {
        quote(text)
    }
}

fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether a string survives a plain-style round trip.
fn is_plain_safe(text: &str) -> bool {
    if text.is_empty() || resolves_to_non_string(text) {
        return false;
    }
    if text.starts_with(' ') || text.ends_with(' ') {
        return false;
    }
    if text.chars().any(char::is_control) {
        return false;
    }

    let mut chars = text.chars();
    let first = chars.next().expect("checked non-empty");
    // Leading indicators always force quoting…
    if matches!(
        first,
        ',' | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    ) {
        return false;
    }
    // …and these only when they would start a token.
    if matches!(first, '-' | '?' | ':') {
        match chars.next() {
            None | Some(' ') => return false,
            Some(_) => {}
        }
    }

    !(text.contains(": ") || text.contains(" #") || text.ends_with(':'))
}

/// Whether YAML 1.2 would read the text back as something other than a string.
fn resolves_to_non_string(text: &str) -> bool {
    matches!(
        text,
        "null" | "Null" | "NULL" | "~" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    ) || text.parse::<i64>().is_ok()
        || text.parse::<f64>().is_ok()
        || matches!(text, ".inf" | "-.inf" | ".nan" | ".NaN" | ".NAN")
        || text
            .strip_prefix("0x")
            .is_some_and(|h| i64::from_str_radix(h, 16).is_ok())
        || text
            .strip_prefix("0o")
            .is_some_and(|o| i64::from_str_radix(o, 8).is_ok())
}

#[cfg(test)]
mod tests {
    use super::{plain_or_quoted, to_string};

    fn yaml(json: &str) -> String {
        to_string(&serde_json::from_str(json).unwrap(), 2)
    }

    #[test]
    fn block_sequences_are_indented_under_their_key() {
        assert_eq!(
            yaml(r#"{"files": ["LICENSE", "README.md"]}"#),
            "files:\n  - LICENSE\n  - README.md\n"
        );
    }

    #[test]
    fn a_map_inside_a_sequence_starts_on_the_dash_line() {
        assert_eq!(
            yaml(r#"{"files": [{"a": "1", "b": "2"}]}"#),
            "files:\n  - a: '1'\n    b: '2'\n".replace('\'', "\"")
        );
    }

    #[test]
    fn flow_indicators_force_double_quotes() {
        assert_eq!(
            plain_or_quoted("{vendorFolder}/ct"),
            "\"{vendorFolder}/ct\""
        );
        assert_eq!(
            plain_or_quoted("https://github.com/a/b"),
            "https://github.com/a/b"
        );
        assert_eq!(plain_or_quoted("v2.5.1"), "v2.5.1");
        assert_eq!(plain_or_quoted("0.37.0"), "0.37.0");
        assert_eq!(plain_or_quoted("1.0"), "\"1.0\"");
        assert_eq!(plain_or_quoted("true"), "\"true\"");
        assert_eq!(plain_or_quoted("dist/x.js"), "dist/x.js");
    }

    #[test]
    fn empty_collections_stay_in_flow_style() {
        assert_eq!(
            yaml(r#"{"vendorDependencies": {}}"#),
            "vendorDependencies: {}\n"
        );
        assert_eq!(yaml(r#"{"files": []}"#), "files: []\n");
    }

    #[test]
    fn nested_mappings_indent_by_the_configured_step() {
        assert_eq!(
            yaml(r#"{"vendorConfig": {"vendorFolder": "./deps"}}"#),
            "vendorConfig:\n  vendorFolder: ./deps\n"
        );
    }
}
