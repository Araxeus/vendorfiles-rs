//! Turns one example's JSON into the three collapsible blocks the README shows.

use anyhow::{Context, Result, bail};
use jsonc_parser::ParseOptions;
use serde_json::Value as Json;

use super::jsonc::{self, Node, Value};
use super::{toml, yaml};

/// What the three `<summary>` lines say.
#[derive(Clone, Copy)]
pub struct Labels {
    pub json: &'static str,
    pub yaml: &'static str,
    pub toml: &'static str,
}

pub const NAMES: Labels = Labels {
    json: "JSON",
    yaml: "YAML",
    toml: "TOML",
};

/// For an example that is about creating the file, where the name is the useful part.
pub const FILES: Labels = Labels {
    json: "vendor.json",
    yaml: "vendor.yml",
    toml: "vendor.toml",
};

/// The key that is a language-server directive rather than config in YAML and TOML.
const SCHEMA: &str = "$schema";

/// Renders the region body: three `<details>`, JSON first and open.
///
/// `fence` is the info string of the source block, reused verbatim so a `jsonc` example keeps
/// its comment-aware highlighting.
pub fn group(json_source: &str, fence: &str, labels: Labels) -> Result<String> {
    let node = jsonc::parse(json_source)?;
    let (body, schema) = split_schema(node);

    let mut yaml_text = yaml::to_string(&body);
    let mut toml_text = toml::to_string(&body);
    if let Some(url) = &schema {
        yaml_text = format!("# yaml-language-server: $schema={url}\n{yaml_text}");
        toml_text = format!("#:schema {url}\n\n{toml_text}");
    }

    verify(json_source, &yaml_text, &toml_text)
        .context("the generated YAML and TOML do not round-trip to the source example")?;

    Ok([
        block(labels.json, fence, json_source.trim_end(), true),
        block(labels.yaml, "yml", yaml_text.trim_end(), false),
        block(labels.toml, "toml", toml_text.trim_end(), false),
    ]
    .join("\n"))
}

/// One `<details>`. The blank line after `</summary>` is required: without it GitHub renders the
/// fence as literal text instead of a highlighted block.
fn block(summary: &str, fence: &str, body: &str, open: bool) -> String {
    let tag = if open { "<details open>" } else { "<details>" };
    format!("{tag}\n<summary>{summary}</summary>\n\n```{fence}\n{body}\n```\n\n</details>\n")
}

/// Lifts a top-level `$schema` out of the tree, because YAML and TOML say it in a comment.
fn split_schema(mut node: Node) -> (Node, Option<String>) {
    let Value::Object(props) = &mut node.value else {
        return (node, None);
    };
    let Some(at) = props.iter().position(|(name, _)| name == SCHEMA) else {
        return (node, None);
    };
    let Value::String(url) = props[at].1.value.clone() else {
        return (node, None);
    };
    props.remove(at);
    (node, Some(url))
}

/// Parses the generated text back with the same crates `vendorfiles_core` reads configs with,
/// and refuses output that does not mean what the source said.
fn verify(json_source: &str, yaml_text: &str, toml_text: &str) -> Result<()> {
    let source = jsonc_parser::parse_to_serde_value::<Json>(json_source, &ParseOptions::default())
        .context("re-reading the example")?;
    let expected = normalize(without_schema(source));

    let from_yaml = serde_yaml_ng::from_str::<Json>(yaml_text).context("re-reading the YAML")?;
    let from_yaml = normalize(from_yaml);
    if from_yaml != expected {
        bail!("YAML differs:\n  expected {expected}\n  got      {from_yaml}");
    }

    let from_toml = toml_edit::de::from_str::<Json>(toml_text).context("re-reading the TOML")?;
    let from_toml = normalize(from_toml);
    if from_toml != expected {
        bail!("TOML differs:\n  expected {expected}\n  got      {from_toml}");
    }
    Ok(())
}

fn without_schema(value: Json) -> Json {
    let Json::Object(mut map) = value else {
        return value;
    };
    map.remove(SCHEMA);
    Json::Object(map)
}

/// An empty map, an empty sequence and null all compare equal. A container whose only content is
/// a comment has no other form in YAML — it reads back as null.
fn normalize(value: Json) -> Json {
    match value {
        Json::Object(map) if map.is_empty() => Json::Null,
        Json::Array(items) if items.is_empty() => Json::Null,
        Json::Object(map) => Json::Object(
            map.into_iter()
                .map(|(key, value)| (key, normalize(value)))
                .collect(),
        ),
        Json::Array(items) => Json::Array(items.into_iter().map(normalize).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{FILES, NAMES, group, verify};

    #[test]
    fn renders_three_details_with_json_open() {
        let out = group(r#"{ "a": "x" }"#, "json", NAMES).unwrap();
        assert!(
            out.starts_with("<details open>\n<summary>JSON</summary>\n\n```json\n"),
            "{out}"
        );
        assert_eq!(out.matches("<details").count(), 3);
        assert_eq!(out.matches("</details>").count(), 3);
        // A blank line after </summary> is what keeps GitHub's syntax highlighting working.
        assert_eq!(out.matches("</summary>\n\n```").count(), 3);
        assert!(
            out.contains("<summary>YAML</summary>\n\n```yml\na: x\n```"),
            "{out}"
        );
        assert!(
            out.contains("<summary>TOML</summary>\n\n```toml\na = 'x'\n```"),
            "{out}"
        );
    }

    #[test]
    fn file_labels_are_selectable() {
        let out = group(r#"{ "a": "x" }"#, "json", FILES).unwrap();
        assert!(out.contains("<summary>vendor.json</summary>"), "{out}");
        assert!(out.contains("<summary>vendor.toml</summary>"), "{out}");
    }

    #[test]
    fn schema_becomes_a_directive_in_the_generated_formats() {
        let source = r#"{
    "$schema": "https://example.com/s.json",
    "vendorDependencies": { //...
    }
}"#;
        let out = group(source, "jsonc", NAMES).unwrap();
        assert!(
            out.contains("# yaml-language-server: $schema=https://example.com/s.json"),
            "{out}"
        );
        assert!(out.contains("#:schema https://example.com/s.json"), "{out}");
        // The key itself is gone from the two generated tabs.
        assert_eq!(out.matches("\"$schema\"").count(), 1, "{out}");
    }

    #[test]
    fn the_source_fence_tag_is_preserved() {
        let out = group("{ \"a\": 1 // n\n}", "jsonc", NAMES).unwrap();
        assert!(out.contains("```jsonc\n"), "{out}");
    }

    #[test]
    fn a_wrong_translation_is_rejected() {
        let out = group(r#"{ "a": 1 }"#, "json", NAMES).unwrap();
        assert!(out.contains("a = 1"), "{out}");
        assert!(verify(r#"{ "a": 1 }"#, "a: 2\n", "a = 1\n").is_err());
    }
}
