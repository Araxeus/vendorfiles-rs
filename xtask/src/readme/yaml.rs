//! Renders a README example as YAML, comments included.
//!
//! `serde_yaml_ng` cannot emit comments at all, and the shipped `yaml_emit` takes a plain
//! `serde_json::Value`, which has nowhere to put them. Only the part worth not duplicating is
//! borrowed from it: the decision about which scalars YAML reads back unchanged.

use std::fmt::Write as _;

use vendorfiles_core::config::yaml_emit;

use super::jsonc::{Node, Value};

const STEP: usize = 2;

pub fn to_string(node: &Node) -> String {
    let mut out = String::new();
    match &node.value {
        Value::Object(props) if !props.is_empty() => write_mapping(&mut out, props, 0),
        Value::Array(items) if !items.is_empty() => write_sequence(&mut out, items, 0),
        other => {
            let _ = writeln!(out, "{}", scalar(other));
        }
    }
    out
}

fn write_mapping(out: &mut String, props: &[(String, Node)], at: usize) {
    let pad = " ".repeat(at);
    for (key, node) in props {
        write_leading(out, node, &pad);
        let key = quoted(key);
        match &node.value {
            Value::Object(inner) if !inner.is_empty() => {
                let _ = writeln!(out, "{pad}{key}:{}", trailing(node));
                write_mapping(out, inner, at + STEP);
            }
            Value::Array(items) if !items.is_empty() => {
                let _ = writeln!(out, "{pad}{key}:{}", trailing(node));
                write_sequence(out, items, at + STEP);
            }
            Value::Object(_) | Value::Array(_) if !node.inner.is_empty() => {
                // A container whose only content is a comment: the comment is the body.
                let _ = writeln!(out, "{pad}{key}:{}", trailing(node));
                write_inner(out, node, at + STEP);
            }
            value => {
                let _ = writeln!(out, "{pad}{key}: {}{}", scalar(value), trailing(node));
            }
        }
    }
}

fn write_sequence(out: &mut String, items: &[Node], at: usize) {
    let pad = " ".repeat(at);
    for node in items {
        write_leading(out, node, &pad);
        match &node.value {
            Value::Object(props) if !props.is_empty() => {
                // The first key shares the dash's line; the rest align past it.
                let mut nested = String::new();
                write_mapping(&mut nested, props, at + STEP);
                let _ = write!(out, "{pad}- {}", nested.trim_start_matches(' '));
            }
            Value::Array(inner) if !inner.is_empty() => {
                let mut nested = String::new();
                write_sequence(&mut nested, inner, at + STEP);
                let _ = write!(out, "{pad}- {}", nested.trim_start_matches(' '));
            }
            value => {
                let _ = writeln!(out, "{pad}- {}{}", scalar(value), trailing(node));
            }
        }
    }
}

fn write_leading(out: &mut String, node: &Node, pad: &str) {
    for comment in &node.leading {
        let _ = writeln!(out, "{pad}#{comment}");
    }
}

fn write_inner(out: &mut String, node: &Node, at: usize) {
    let pad = " ".repeat(at);
    for comment in &node.inner {
        let _ = writeln!(out, "{pad}#{comment}");
    }
}

fn trailing(node: &Node) -> String {
    node.trailing
        .as_ref()
        .map_or_else(String::new, |text| format!(" #{text}"))
}

/// A scalar, or the flow form of a container with nothing in it.
fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.clone(),
        Value::String(s) => quoted(s),
        Value::Array(_) => "[]".to_owned(),
        Value::Object(_) => "{}".to_owned(),
    }
}

/// Bare when YAML reads it back unchanged, single-quoted otherwise.
///
/// Which strings are safe bare is the tool's own rule, borrowed rather than restated; only the
/// quoting style differs, because the README and `examples/` use single quotes.
fn quoted(text: &str) -> String {
    let escaped = yaml_emit::plain_or_quoted(text);
    if escaped == text || text.chars().any(char::is_control) {
        // Either nothing to do, or a control character, which only the double-quoted form can
        // escape.
        return escaped;
    }
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::super::jsonc::parse;
    use super::to_string;

    #[test]
    fn renders_nested_maps_and_sequences() {
        let node = parse(
            r#"{
    "vendorDependencies": {
        "Coloris": {
            "version": "v0.17.1",
            "files": ["dist/coloris.min.js", "dist/coloris.min.css"]
        }
    }
}"#,
        )
        .unwrap();
        assert_eq!(
            to_string(&node),
            "vendorDependencies:\n  Coloris:\n    version: v0.17.1\n    files:\n      \
             - dist/coloris.min.js\n      - dist/coloris.min.css\n"
        );
    }

    #[test]
    fn quotes_only_what_yaml_would_misread() {
        let node =
            parse(r#"{ "a": "{vendorFolder}/x", "b": "./my-vendors", "c": "v2.2.0" }"#).unwrap();
        assert_eq!(
            to_string(&node),
            "a: '{vendorFolder}/x'\nb: ./my-vendors\nc: v2.2.0\n"
        );
    }

    #[test]
    fn writes_comments_where_they_were() {
        let node = parse(
            r#"{
    // above
    "a": "x", // → after
    "b": { //...
    }
}"#,
        )
        .unwrap();
        assert_eq!(to_string(&node), "# above\na: x # → after\nb:\n  #...\n");
    }

    #[test]
    fn maps_inside_sequences_share_the_dash_line() {
        let node = parse(r#"{ "files": [{ "{release}/f.zip": ["f.exe"] }] }"#).unwrap();
        assert_eq!(
            to_string(&node),
            "files:\n  - '{release}/f.zip':\n      - f.exe\n"
        );
    }

    #[test]
    fn comment_free_output_matches_the_shipped_emitter() {
        let source = r#"{
    "vendorConfig": { "vendorFolder": "./my-vendors" },
    "vendorDependencies": {
        "Cooltipz": { "version": "v2.2.0", "files": ["cooltipz.min.css", "LICENSE"] }
    }
}"#;
        let value: serde_json::Value = serde_json::from_str(source).unwrap();
        let shipped = vendorfiles_core::config::yaml_emit::to_string(&value, 2);
        let ours = to_string(&parse(source).unwrap());
        // Same structure and indentation; the only intended difference is the quoting style,
        // which this example deliberately avoids needing.
        assert_eq!(ours, shipped);
    }
}
