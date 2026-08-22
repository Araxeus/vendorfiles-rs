//! Turns one example's JSON into the three collapsible blocks the README shows.

use std::collections::BTreeMap;

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
    check_comments(&body, &yaml_text, &toml_text)?;

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
    let removed = props.remove(at).1;
    // The key turns into a directive comment, so anything written about it moves to the top of
    // the document rather than leaving with the key.
    let mut carried = removed.leading;
    carried.extend(removed.trailing);
    carried.extend(removed.inner);
    carried.extend(std::mem::take(&mut node.leading));
    node.leading = carried;
    (node, Some(url))
}

/// Every comment the source carried has to survive into both generated formats.
///
/// The round-trip check compares data, and a dropped comment changes no data — so without this a
/// note could vanish from a tab while the build stayed green, which is exactly the drift this
/// command exists to prevent. A comment a format has no syntax for (inside a TOML inline table,
/// say) fails here instead of disappearing; the fix is to move it in the example.
fn check_comments(body: &Node, yaml_text: &str, toml_text: &str) -> Result<()> {
    let mut comments = Vec::new();
    collect_comments(body, &mut comments);
    let wanted = tally(comments);

    for (format, rendered) in [("YAML", yaml_text), ("TOML", toml_text)] {
        let written = tally(comments_in(rendered));
        for (comment, count) in &wanted {
            if written.get(comment).copied().unwrap_or_default() < *count {
                bail!(
                    "the comment `//{comment}` has no place in the generated {format}; \
                     move it in the example, above the property it describes"
                );
            }
        }
    }
    Ok(())
}

fn tally(comments: Vec<String>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for comment in comments {
        *counts.entry(comment).or_default() += 1;
    }
    counts
}

/// The comment text of every comment in an emitted document, one entry per comment.
///
/// Searching the text for `#{comment}` instead would be fooled by one comment being a prefix of
/// another — `# a` is a substring of `# above a`, so a dropped `// a` would look present. Both
/// formats run a comment to the end of its line, start one only at the beginning of a line or
/// after a space, and quote strings the same two ways, so skipping quoted spans is enough to
/// read the real ones back exactly.
fn comments_in(rendered: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in rendered.lines() {
        let mut quote: Option<char> = None;
        let mut previous = ' ';
        let mut chars = line.char_indices();
        while let Some((at, ch)) = chars.next() {
            match (quote, ch) {
                (None, '\'' | '"') => quote = Some(ch),
                // An escape inside a basic string cannot end it.
                (Some('"'), '\\') => {
                    chars.next();
                }
                (Some(open), ch) if ch == open => quote = None,
                (None, '#') if previous.is_whitespace() => {
                    found.push(line[at + 1..].to_owned());
                    break;
                }
                _ => {}
            }
            previous = ch;
        }
    }
    found
}

fn collect_comments(node: &Node, into: &mut Vec<String>) {
    into.extend(node.leading.iter().cloned());
    into.extend(node.trailing.iter().cloned());
    into.extend(node.inner.iter().cloned());
    match &node.value {
        Value::Object(props) => {
            for (_, child) in props {
                collect_comments(child, into);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_comments(child, into);
            }
        }
        _ => {}
    }
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
    use super::super::jsonc;
    use super::{FILES, NAMES, check_comments, group, verify};

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
    fn a_comment_no_format_can_place_fails_instead_of_vanishing() {
        // TOML has no syntax for a comment inside an inline table, and an object inside an array
        // has to be one. Better a build failure naming the comment than a tab that quietly
        // loses it.
        let source = r#"{
    "files": [
        {
            "a": "b" // note
        }
    ]
}"#;
        let error = format!("{:#}", group(source, "jsonc", NAMES).unwrap_err());
        assert!(
            error.contains("has no place in the generated TOML"),
            "{error}"
        );
    }

    #[test]
    fn a_comment_on_the_schema_key_moves_to_the_top() {
        let source = r#"{
    // editors pick this up
    "$schema": "https://example.com/s.json",
    "a": 1
}"#;
        let out = group(source, "jsonc", NAMES).unwrap();
        assert!(
            out.contains("```yml\n# yaml-language-server: $schema=https://example.com/s.json\n# editors pick this up\n"),
            "{out}"
        );
        assert!(
            out.contains(
                "```toml\n#:schema https://example.com/s.json\n\n# editors pick this up\n"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_comment_that_is_a_prefix_of_another_is_still_missed_when_dropped() {
        let body = jsonc::parse(
            r#"{
    // above a
    "x": 1, // a
    "y": 2
}"#,
        )
        .unwrap();
        // A rendering that kept the long comment and lost the short one. `# a` is a substring of
        // `# above a`, so a plain text search would call the lost one present.
        let dropped = "# above a
x: 1
y: 2
";
        let error = format!("{:#}", check_comments(&body, dropped, dropped).unwrap_err());
        assert!(error.contains("`// a`"), "{error}");
    }

    #[test]
    fn a_hash_inside_a_value_is_not_mistaken_for_a_comment() {
        let body = jsonc::parse(r#"{ "a": "https://example.com/x#y" }"#).unwrap();
        let yaml = "a: https://example.com/x#y
";
        let toml = "a = 'https://example.com/x#y'
";
        assert!(check_comments(&body, yaml, toml).is_ok());
    }

    #[test]
    fn a_wrong_translation_is_rejected() {
        let out = group(r#"{ "a": 1 }"#, "json", NAMES).unwrap();
        assert!(out.contains("a = 1"), "{out}");
        assert!(verify(r#"{ "a": 1 }"#, "a: 2\n", "a = 1\n").is_err());
    }
}
