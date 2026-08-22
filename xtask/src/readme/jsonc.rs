//! Reads a README example's JSONC into a tree that keeps key order and comments.
//!
//! `serde_json` would lose the comments and `jsonc-parser`'s value API would too, so the CST is
//! walked directly: it keeps comments as sibling nodes in document order, which is exactly the
//! information needed to tell "a note after this value" from "a note above the next one".

use anyhow::{Result, anyhow, bail};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstComment, CstLeafNode, CstNode, CstRootNode};

/// A value together with the comments written around it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    /// Whole-line comments above the value, in source order.
    pub leading: Vec<String>,
    /// A comment on the same line as the value's last token.
    pub trailing: Option<String>,
    /// Comments inside a container that has no members of its own.
    pub inner: Vec<String>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    /// Kept verbatim: `0.37.0` is a string, but `2` must not come back out as `2.0`.
    Number(String),
    String(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

/// Parses one example. Errors name the problem in terms an author can act on.
pub fn parse(source: &str) -> Result<Node> {
    let root = CstRootNode::parse(source, &ParseOptions::default())
        .map_err(|e| anyhow!("parsing the example as JSONC: {e}"))?;
    let (entries, leftover) = collect(&root.children())?;
    let [entry] = entries.as_slice() else {
        bail!(
            "an example must hold exactly one JSON value, found {}",
            entries.len()
        );
    };
    let mut node = node_from(entry)?;
    node.inner.extend(leftover);
    Ok(node)
}

/// A child of a container together with the comments that turned out to be its own.
struct Entry {
    node: CstNode,
    leading: Vec<String>,
    trailing: Option<String>,
}

/// One pass over a container's children, attaching each comment to the value it belongs to.
///
/// Returns the container's members and the comments left over - those sit inside a container
/// with no members of its own, which is how `{ //... }` is written.
fn collect(children: &[CstNode]) -> Result<(Vec<Entry>, Vec<String>)> {
    let mut out: Vec<Entry> = Vec::new();
    let mut leading: Vec<String> = Vec::new();
    // A comment reached while this holds is on the same line as the value just finished.
    let mut same_line = false;

    for child in children {
        match child {
            CstNode::Leaf(CstLeafNode::Comment(comment)) => {
                let text = comment_text(comment)?;
                match out.last_mut() {
                    Some(last) if same_line && last.trailing.is_none() => {
                        last.trailing = Some(text);
                    }
                    _ => leading.push(text),
                }
            }
            CstNode::Leaf(CstLeafNode::Newline(_)) => same_line = false,
            CstNode::Leaf(CstLeafNode::Whitespace(_) | CstLeafNode::Token(_)) => {}
            value => {
                out.push(Entry {
                    node: value.clone(),
                    leading: std::mem::take(&mut leading),
                    trailing: None,
                });
                same_line = true;
            }
        }
    }

    Ok((out, leading))
}

/// Converts one member into a node, carrying over the comments `collect` gave it.
fn node_from(entry: &Entry) -> Result<Node> {
    let mut node = value_of(&entry.node)?;
    node.leading.clone_from(&entry.leading);
    // A property's own trailing comment wins over one the value already picked up: they can
    // only both be set when the value ends on the line the property does.
    if entry.trailing.is_some() {
        node.trailing.clone_from(&entry.trailing);
    }
    Ok(node)
}

fn value_of(cst: &CstNode) -> Result<Node> {
    if let CstNode::Container(container) = cst {
        if let Some(object) = container.as_object() {
            let (entries, inner) = collect(&object.children())?;
            let mut props = Vec::with_capacity(entries.len());
            for entry in &entries {
                let CstNode::Container(prop) = &entry.node else {
                    bail!("an object may only hold properties");
                };
                let prop = prop
                    .as_object_prop()
                    .ok_or_else(|| anyhow!("an object may only hold properties"))?;
                let name = prop
                    .name()
                    .ok_or_else(|| anyhow!("a property is missing its name"))?
                    .decoded_value()
                    .map_err(|e| anyhow!("reading a property name: {e:?}"))?;
                props.push((name, node_from(entry)?));
            }
            return Ok(Node {
                inner,
                value: Value::Object(props),
                ..Node::default()
            });
        }
        if let Some(array) = container.as_array() {
            let (entries, inner) = collect(&array.children())?;
            let items = entries.iter().map(node_from).collect::<Result<Vec<_>>>()?;
            return Ok(Node {
                inner,
                value: Value::Array(items),
                ..Node::default()
            });
        }
        if let Some(prop) = container.as_object_prop() {
            let value = prop
                .value()
                .ok_or_else(|| anyhow!("a property is missing its value"))?;
            return value_of(&value);
        }
        bail!("unsupported JSON construct");
    }

    let CstNode::Leaf(leaf) = cst else {
        bail!("unsupported JSON construct");
    };
    let value = match leaf {
        CstLeafNode::StringLit(text) => Value::String(
            text.decoded_value()
                .map_err(|e| anyhow!("reading a string: {e:?}"))?,
        ),
        other => {
            let text = other.to_string();
            match text.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" => Value::Null,
                number => Value::Number(number.to_owned()),
            }
        }
    };
    Ok(Node {
        value,
        ..Node::default()
    })
}

/// Strips the marker, keeping the rest verbatim so `//...` stays `#...` and `// → x` stays
/// `# → x`, and rejects the form the emitters have nowhere to put.
fn comment_text(comment: &CstComment) -> Result<String> {
    let raw = comment.raw_value();
    let trimmed = raw.trim_end();
    let Some(body) = trimmed.strip_prefix("//") else {
        bail!(
            "block comments cannot be placed in the generated YAML and TOML; use `//`: {trimmed}"
        );
    };
    Ok(body.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Value, parse};

    #[test]
    fn keeps_key_order_and_scalar_shapes() {
        let node = parse(r#"{ "b": "1.0", "a": 2, "c": true, "d": null }"#).unwrap();
        let Value::Object(props) = &node.value else {
            panic!("expected an object");
        };
        let keys: Vec<&str> = props.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["b", "a", "c", "d"]);
        assert_eq!(props[0].1.value, Value::String("1.0".to_owned()));
        assert_eq!(props[1].1.value, Value::Number("2".to_owned()));
        assert_eq!(props[2].1.value, Value::Bool(true));
        assert_eq!(props[3].1.value, Value::Null);
    }

    #[test]
    fn attaches_trailing_leading_and_inner_comments() {
        let source = r#"{
    "a": "x", // → after
    // above
    "b": "y",
    "c": { //...
    }
}"#;
        let node = parse(source).unwrap();
        let Value::Object(props) = &node.value else {
            panic!("expected an object");
        };
        assert_eq!(props[0].1.trailing.as_deref(), Some(" → after"));
        assert_eq!(props[1].1.leading, [" above"]);
        assert_eq!(props[2].1.inner, ["..."]);
    }

    #[test]
    fn rejects_block_comments() {
        let error = parse(r#"{ "a": 1 /* no */ }"#).unwrap_err().to_string();
        assert!(error.contains("block comment"), "{error}");
    }
}
