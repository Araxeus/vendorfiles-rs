//! Renders a README example as TOML, comments included.
//!
//! `toml_edit::ser::to_document` is what `config/document.rs` uses, but only ever for a single
//! dependency that is then inserted as a table. Handed a whole config it renders one inline
//! line - `vendorDependencies = { fzf = { … } }` - so documentation is emitted directly instead.

use std::fmt::Write as _;

use super::jsonc::{Node, Value};

/// Past this width an array is broken across lines, as `examples/vendor.toml` does.
const WIDTH: usize = 90;

pub fn to_string(node: &Node) -> String {
    let mut out = String::new();
    // A note above the example's opening brace belongs to the document, and the root never gets
    // a header to hang it on.
    for comment in &node.leading {
        let _ = writeln!(out, "#{comment}");
    }
    if let Value::Object(props) = &node.value {
        write_table(&mut out, props, &[], node, &mut Vec::new());
        if let Some(text) = &node.trailing {
            let _ = writeln!(out, "#{text}");
        }
    } else {
        let _ = writeln!(out, "{}{}", inline(&node.value), trailing(node));
    }
    // Anything written after the last member closes the document.
    for comment in &node.inner {
        let _ = writeln!(out, "#{comment}");
    }
    out
}

/// Writes one table: its header if it needs one, its scalar members, then its sub-tables.
///
/// TOML reads every key after a header as part of that table, so scalars have to come first.
/// That is the one place the output's order differs from the source's, and the round-trip check
/// in `render` is what proves the reordering changed nothing.
///
/// `pending` carries the comments of tables that print no header of their own. A note above
/// `"vendorDependencies"` has nowhere to sit when that key only implies `[vendorDependencies.x]`,
/// so it waits here and is written above the first header that does appear.
fn write_table(
    out: &mut String,
    props: &[(String, Node)],
    path: &[&str],
    owner: &Node,
    pending: &mut Vec<String>,
) {
    let (values, tables): (Vec<_>, Vec<_>) = props.iter().partition(|(_, node)| !is_table(node));

    // The root has no header, and neither does a table that only exists to hold other tables:
    // `[vendorDependencies.fzf]` already implies `vendorDependencies`.
    let prints_header = !path.is_empty() && (!values.is_empty() || !owner.inner.is_empty());
    if prints_header {
        if !out.is_empty() {
            out.push('\n');
        }
        for comment in pending.drain(..) {
            let _ = writeln!(out, "#{comment}");
        }
        for comment in &owner.leading {
            let _ = writeln!(out, "#{comment}");
        }
        let header = path
            .iter()
            .map(|part| key(part))
            .collect::<Vec<_>>()
            .join(".");
        let _ = writeln!(out, "[{header}]{}", trailing(owner));
    } else if !path.is_empty() {
        pending.extend(owner.leading.iter().cloned());
        pending.extend(owner.trailing.clone());
    }

    for (name, node) in values {
        for comment in &node.leading {
            let _ = writeln!(out, "#{comment}");
        }
        let assigned = format!("{} = ", key(name));
        let _ = writeln!(
            out,
            "{assigned}{}{}",
            value_text(node, assigned.len()),
            trailing(node)
        );
    }

    // Written after the members, where the source had them: just before the closing brace. The
    // root's own are left to `to_string`, which puts them at the end of the document.
    if !path.is_empty() {
        for comment in &owner.inner {
            let _ = writeln!(out, "#{comment}");
        }
    }

    for (name, node) in tables {
        let Value::Object(inner) = &node.value else {
            unreachable!("partitioned on `is_table`");
        };
        let mut nested: Vec<&str> = path.to_vec();
        nested.push(name);
        write_table(out, inner, &nested, node, pending);
    }
}

/// Whether a member gets its own `[header]` rather than sitting on the right of an `=`.
///
/// An object with nothing but a comment in it still becomes a table, so the comment has a home.
const fn is_table(node: &Node) -> bool {
    match &node.value {
        Value::Object(props) => !props.is_empty() || !node.inner.is_empty(),
        _ => false,
    }
}

/// The right-hand side of an assignment, broken across lines when one line will not do.
fn value_text(node: &Node, column: usize) -> String {
    let Value::Array(items) = &node.value else {
        return inline(&node.value);
    };
    let annotated = items
        .iter()
        .any(|item| !item.leading.is_empty() || item.trailing.is_some());
    let flat = inline(&node.value);
    if !annotated && node.inner.is_empty() && column + flat.len() <= WIDTH {
        return flat;
    }

    let mut out = String::from("[\n");
    for item in items {
        for comment in &item.leading {
            let _ = writeln!(out, "  #{comment}");
        }
        let _ = writeln!(out, "  {},{}", inline(&item.value), trailing(item));
    }
    // Written after the last element, where the source had them, as the YAML emitter does.
    for comment in &node.inner {
        let _ = writeln!(out, "  #{comment}");
    }
    out.push(']');
    out
}

/// A value as it appears on the right of `=`.
fn inline(value: &Value) -> String {
    match value {
        Value::Null => "''".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.clone(),
        Value::String(s) => string(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(|item| inline(&item.value)).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(props) => {
            let parts: Vec<String> = props
                .iter()
                .map(|(name, node)| format!("{} = {}", key(name), inline(&node.value)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

fn trailing(node: &Node) -> String {
    node.trailing
        .as_ref()
        .map_or_else(String::new, |text| format!(" #{text}"))
}

/// Literal strings, which need no escaping, unless the content cannot survive them.
///
/// A basic string has to escape every control character, not just the three with familiar names:
/// TOML forbids a raw one outright, and leaving it in produces a document no parser will read.
fn string(text: &str) -> String {
    if !text.contains('\'') && !text.chars().any(char::is_control) {
        return format!("'{text}'");
    }

    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            // Every control character TOML has no name for. All of them are below U+00FF, so
            // the four-digit form always fits.
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn key(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return text.to_owned();
    }
    string(text)
}

#[cfg(test)]
mod tests {
    use super::super::jsonc::parse;
    use super::to_string;

    #[test]
    fn nested_objects_become_tables() {
        let node = parse(
            r#"{
    "vendorConfig": { "vendorFolder": "./my-vendors" },
    "vendorDependencies": {
        "Cooltipz": { "version": "v2.2.0", "files": ["cooltipz.min.css", "LICENSE"] }
    }
}"#,
        )
        .unwrap();
        assert_eq!(
            to_string(&node),
            "[vendorConfig]\nvendorFolder = './my-vendors'\n\n\
             [vendorDependencies.Cooltipz]\nversion = 'v2.2.0'\n\
             files = ['cooltipz.min.css', 'LICENSE']\n"
        );
    }

    #[test]
    fn scalars_come_before_sub_tables() {
        let node = parse(r#"{ "a": { "sub": { "x": 1 }, "k": "v" } }"#).unwrap();
        assert_eq!(to_string(&node), "[a]\nk = 'v'\n\n[a.sub]\nx = 1\n");
    }

    #[test]
    fn objects_inside_arrays_stay_inline() {
        let node = parse(r#"{ "files": ["LICENSE", { "{release}/f.zip": ["f.exe"] }] }"#).unwrap();
        assert_eq!(
            to_string(&node),
            "files = ['LICENSE', { '{release}/f.zip' = ['f.exe'] }]\n"
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
        assert_eq!(
            to_string(&node),
            "# above\na = 'x' # → after\n\n[b]\n#...\n"
        );
    }

    #[test]
    fn keeps_comments_written_outside_the_members() {
        let node = parse(
            r#"// header
{
    "a": 1
    // dangling
}
// footer"#,
        )
        .unwrap();
        assert_eq!(to_string(&node), "# header\na = 1\n# dangling\n# footer\n");
    }

    #[test]
    fn a_comment_above_an_implied_table_waits_for_the_first_header() {
        let node = parse(
            r#"{
    // all the deps
    "vendorDependencies": {
        "x": { "version": "v1" }
    }
}"#,
        )
        .unwrap();
        assert_eq!(
            to_string(&node),
            "# all the deps\n[vendorDependencies.x]\nversion = 'v1'\n"
        );
    }

    #[test]
    fn every_control_character_is_escaped_rather_than_written_raw() {
        // A raw control character makes a basic string invalid TOML, and `\b` and `\u0001` are
        // reachable from a JSON example: `"C:\tools\bin"` decodes to a tab and a backspace.
        let node = parse(r#"{ "a": "x\by", "b": "y\u0001z", "c": "p\fq" }"#).unwrap();
        assert_eq!(
            to_string(&node),
            "a = \"x\\by\"\nb = \"y\\u0001z\"\nc = \"p\\fq\"\n"
        );
    }

    #[test]
    fn a_dangling_array_comment_goes_after_the_last_element() {
        let node = parse(
            r#"{
    "files": [
        "a"
        // dangling
    ]
}"#,
        )
        .unwrap();
        assert_eq!(to_string(&node), "files = [\n  'a',\n  # dangling\n]\n");
    }

    #[test]
    fn a_long_array_breaks_across_lines() {
        let node = parse(
            r#"{ "files": ["{release}/fzf-{version}-windows_amd64.zip", "{release}/fzf-{version}-linux_amd64.tar.gz"] }"#,
        )
        .unwrap();
        assert_eq!(
            to_string(&node),
            "files = [\n  '{release}/fzf-{version}-windows_amd64.zip',\n  \
             '{release}/fzf-{version}-linux_amd64.tar.gz',\n]\n"
        );
    }
}
