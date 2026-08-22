//! Renders a README example as TOML, comments included.
//!
//! `toml_edit::ser::to_document` is what `config/document.rs` uses, but only ever for a single
//! dependency that is then inserted as a table. Handed a whole config it renders one inline
//! line — `vendorDependencies = { fzf = { … } }` — so documentation is emitted directly instead.

use std::fmt::Write as _;

use super::jsonc::{Node, Value};

/// Past this width an array is broken across lines, as `examples/vendor.toml` does.
const WIDTH: usize = 90;

pub fn to_string(node: &Node) -> String {
    let mut out = String::new();
    if let Value::Object(props) = &node.value {
        write_table(&mut out, props, &[], node);
    } else {
        let _ = writeln!(out, "{}", inline(&node.value));
    }
    out
}

/// Writes one table: its header if it needs one, its scalar members, then its sub-tables.
///
/// TOML reads every key after a header as part of that table, so scalars have to come first.
/// That is the one place the output's order differs from the source's, and the round-trip check
/// in `render` is what proves the reordering changed nothing.
fn write_table(out: &mut String, props: &[(String, Node)], path: &[&str], owner: &Node) {
    let (values, tables): (Vec<_>, Vec<_>) = props.iter().partition(|(_, node)| !is_table(node));

    // The root has no header, and neither does a table that only exists to hold other tables:
    // `[vendorDependencies.fzf]` already implies `vendorDependencies`.
    if !path.is_empty() && (!values.is_empty() || !owner.inner.is_empty()) {
        if !out.is_empty() {
            out.push('\n');
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
        for comment in &owner.inner {
            let _ = writeln!(out, "#{comment}");
        }
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

    for (name, node) in tables {
        let Value::Object(inner) = &node.value else {
            unreachable!("partitioned on `is_table`");
        };
        let mut nested: Vec<&str> = path.to_vec();
        nested.push(name);
        write_table(out, inner, &nested, node);
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
    for comment in &node.inner {
        let _ = writeln!(out, "  #{comment}");
    }
    for item in items {
        for comment in &item.leading {
            let _ = writeln!(out, "  #{comment}");
        }
        let _ = writeln!(out, "  {},{}", inline(&item.value), trailing(item));
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
fn string(text: &str) -> String {
    if text.contains('\'') || text.chars().any(char::is_control) {
        let escaped = text
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t");
        return format!("\"{escaped}\"");
    }
    format!("'{text}'")
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
