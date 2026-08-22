# README Format Tabs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `cargo xtask readme` generates the YAML and TOML tabs of every README config example from that example's JSON, and CI fails when they drift.

**Architecture:** A new `xtask/src/readme/` module. It finds `<!-- formats -->` … `<!-- /formats -->` regions in `README.md`, reads the JSONC block inside each into a comment-carrying tree (`jsonc-parser`'s CST), emits YAML and TOML from that tree, proves each emission by parsing it back and comparing against the source, and rewrites the region as three stacked `<details>` blocks.

**Tech Stack:** Rust 2024, `jsonc-parser` 0.33 (`cst` + `serde` features), `serde_json` (`preserve_order`), `serde_yaml_ng`, `toml_edit` (`serde`), `vendorfiles_core` (for its YAML plain-safety rule), `anyhow`, `clap`.

**Spec:** `docs/superpowers/specs/2026-08-22-readme-format-tabs-design.md`

## Global Constraints

- **Do not commit.** The user asked for everything to stay in the working tree. Every task ends with a verification step, not a `git commit`.
- Rust edition 2024, MSRV 1.97.1 (`[workspace.package]` in `Cargo.toml`).
- New dependencies go in `[workspace.dependencies]` in the root `Cargo.toml` with a comment explaining why, matching the style of every other entry there, and are referenced from `xtask/Cargo.toml` as `name.workspace = true`.
- Clippy runs with `-W clippy::pedantic -W clippy::cargo -W clippy::nursery -D warnings`. Code must be clean under that, not just `cargo build`.
- `cargo fmt --all --check` must pass.
- Module layout follows `crates/vendorfiles_core/src/config/`: a directory with `mod.rs`.
- Tests live in `#[cfg(test)] mod tests` at the bottom of the file they test, as everywhere else in this repo.
- Doc comments explain *why*, matching the existing house style (see `xtask/src/ci.rs`, `crates/vendorfiles_core/src/config/yaml_emit.rs`).

---

### Task 1: The JSONC reader

Reads an example's JSON source into a tree that keeps key order and the comments written around each value.

**Files:**
- Create: `xtask/src/readme/jsonc.rs`
- Create: `xtask/src/readme/mod.rs` (just `mod jsonc;` for now)
- Modify: `Cargo.toml` (`[workspace.dependencies]`)
- Modify: `xtask/Cargo.toml`
- Modify: `xtask/src/main.rs` (add `mod readme;`)

**Interfaces:**
- Produces:
  - `pub struct Node { pub leading: Vec<String>, pub trailing: Option<String>, pub inner: Vec<String>, pub value: Value }`
  - `pub enum Value { Null, Bool(bool), Number(String), String(String), Array(Vec<Node>), Object(Vec<(String, Node)>) }`
  - `pub fn parse(source: &str) -> anyhow::Result<Node>`
  - Comment strings are stored with `//` stripped and **nothing else trimmed**, so `// → x` stores ` → x` and `//...` stores `...`. Emitters write `#` + the stored text.

- [ ] **Step 1: Add the dependencies**

In the root `Cargo.toml`, inside `[workspace.dependencies]`, keeping the list alphabetical:

```toml
# Reads the README's config examples for `cargo xtask readme`. The `cst` feature is the reason
# for the crate: it keeps comments as ordinary sibling nodes, so a `//` note on an example
# survives into the generated YAML and TOML instead of being dropped on the way through a value
# tree. `serde` gives the same text as a `serde_json::Value` for the round-trip check.
jsonc-parser = { version = "0.33", default-features = false, features = ["cst", "serde"] }
```

In `xtask/Cargo.toml`, under `[dependencies]`:

```toml
jsonc-parser.workspace = true
serde_json.workspace = true
serde_yaml_ng.workspace = true
vendorfiles_core.workspace = true
```

(`anyhow`, `clap` and `toml_edit` are already there.)

- [ ] **Step 2: Write the failing test**

Create `xtask/src/readme/jsonc.rs` containing only the test module:

```rust
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
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p xtask readme::jsonc`
Expected: FAIL — `cannot find function \`parse\` in this scope`.

- [ ] **Step 4: Implement the reader**

Write the rest of `xtask/src/readme/jsonc.rs` above the test module. The shape:

```rust
//! Reads a README example's JSONC into a tree that keeps key order and comments.
//!
//! `serde_json` would lose the comments and `jsonc-parser`'s value API would too, so the CST is
//! walked directly: it keeps comments as sibling nodes in document order, which is exactly the
//! information needed to tell "a note after this value" from "a note above the next one".

use anyhow::{Result, anyhow, bail};
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstLeafNode, CstNode, CstRootNode};

/// A value together with the comments written around it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Whole-line comments above the value, in source order.
    pub leading: Vec<String>,
    /// A comment on the same line as the value's last token.
    pub trailing: Option<String>,
    /// Comments inside a container that has no members of its own.
    pub inner: Vec<String>,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Kept verbatim: `0.37.0` is a string, but `2` must not become `2.0` on the way out.
    Number(String),
    String(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

/// Parses one example. Errors name the problem in terms an author can act on.
pub fn parse(source: &str) -> Result<Node> {
    let root = CstRootNode::parse(source, &ParseOptions::default())
        .map_err(|e| anyhow!("parsing the example as JSONC: {e}"))?;
    let (mut values, leftover) = collect(&root.children())?;
    if values.len() != 1 {
        bail!("an example must contain exactly one JSON value, found {}", values.len());
    }
    let mut node = values.remove(0);
    node.inner.extend(leftover);
    Ok(node)
}

/// One pass over a container's children, attaching each comment to the value it belongs to.
///
/// Returns the container's members and the comments left over — those sit inside a container
/// that has no members, which is how `{ //... }` is written.
fn collect(children: &[CstNode]) -> Result<(Vec<Node>, Vec<String>)> {
    let mut out: Vec<Node> = Vec::new();
    let mut leading: Vec<String> = Vec::new();
    // A comment reached while this is true is on the same line as the value just finished.
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
                let mut node = node_from(value)?;
                node.leading = std::mem::take(&mut leading);
                out.push(node);
                same_line = true;
            }
        }
    }

    Ok((out, leading))
}
```

`node_from` converts one non-trivia child:

- `CstNode::Container` that is an object → `collect` over its children, pairing each member with its property name (`prop.name()` returns `Option<ObjectPropName>`, whose `decoded_value()` returns `Result<String, ParseStringErrorKind>`); the leftover comments become that object's `inner`.
- `CstNode::Container` that is an array → `collect` over its children; leftovers become the array's `inner`.
- `CstNode::Container` that is an object property → recurse into `prop.value()` (`Option<CstNode>`), so the property's node carries the value's comments.
- String / number / boolean / null leaves → the matching `Value`.

`comment_text` strips the marker and rejects the form the emitters cannot place:

```rust
fn comment_text(comment: &jsonc_parser::cst::CstComment) -> Result<String> {
    let raw = comment.raw_value();
    let trimmed = raw.trim_end();
    let Some(body) = trimmed.strip_prefix("//") else {
        bail!("block comments cannot be placed in the generated YAML and TOML; use `//`: {trimmed}");
    };
    Ok(body.to_owned())
}
```

The exact accessor names on the numeric and boolean leaves are whatever the crate calls them —
resolve them against the compiler; `CstStringLit::decoded_value` and `CstComment::raw_value` are
confirmed to exist with the signatures above.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p xtask readme::jsonc`
Expected: 3 passed.

- [ ] **Step 6: Verify the lint gate**

Run: `cargo clippy -p xtask --all-targets -- -W clippy::pedantic -W clippy::cargo -W clippy::nursery -D warnings && cargo fmt --all --check`
Expected: no output, exit 0. Leave the changes in the working tree.

---

### Task 2: The YAML emitter

**Files:**
- Create: `xtask/src/readme/yaml.rs`
- Modify: `xtask/src/readme/mod.rs` (add `mod yaml;`)

**Interfaces:**
- Consumes: `super::jsonc::{Node, Value}` from Task 1.
- Produces: `pub fn to_string(node: &Node) -> String` — a YAML document ending in a newline.

Style, matching `examples/vendor.yml` and the four hand-converted README blocks: two-space indent, block sequences indented one level under their key, the first key of a mapping inside a sequence sharing the dash's line. Scalars are written bare when `vendorfiles_core::config::yaml_emit::plain_or_quoted` returns them unchanged — reusing the tool's own tested plain-safety rule — and single-quoted otherwise, with `'` doubled. A string carrying a control character falls back to `plain_or_quoted`'s double-quoted form, which is the only one that can escape it.

- [ ] **Step 1: Write the failing test**

Create `xtask/src/readme/yaml.rs` with only:

```rust
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
        let node = parse(r#"{ "a": "{vendorFolder}/x", "b": "./my-vendors", "c": "v2.2.0" }"#).unwrap();
        assert_eq!(to_string(&node), "a: '{vendorFolder}/x'\nb: ./my-vendors\nc: v2.2.0\n");
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
        assert_eq!(to_string(&node), "files:\n  - '{release}/f.zip':\n      - f.exe\n");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p xtask readme::yaml`
Expected: FAIL — `cannot find function \`to_string\``.

- [ ] **Step 3: Implement the emitter**

Write above the tests:

```rust
//! Renders a README example as YAML, comments included.
//!
//! `serde_yaml_ng` cannot emit comments at all, and the shipped `yaml_emit` takes a plain
//! `serde_json::Value`, which has nowhere to put them. Only the part worth not duplicating is
//! borrowed from it: the decision about which scalars YAML would read back unchanged.

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
fn quoted(text: &str) -> String {
    let escaped = yaml_emit::plain_or_quoted(text);
    if escaped == text {
        return escaped;
    }
    if text.chars().any(char::is_control) {
        // Single quotes cannot escape a control character; the double-quoted form can.
        return escaped;
    }
    format!("'{}'", text.replace('\'', "''"))
}
```

`write_mapping` walks `&[(String, Node)]` at an indent. Per member: each `leading` comment on its own line as `{pad}#{text}`; then

- a scalar value → `{pad}{key}: {scalar}` plus ` #{trailing}` when there is one;
- a container with members → `{pad}{key}:` plus ` #{trailing}`, then the members at `at + STEP`;
- an empty container → `{pad}{key}:` plus ` #{trailing}`, then each `inner` comment as `{pad}{STEP spaces}#{text}`.

Keys go through `quoted` as well.

`write_sequence` writes each item at `{pad}- `. A mapping item is rendered into a scratch string at `at + STEP` and appended with its leading pad trimmed, so its first key lands on the dash's line — the same trick `yaml_emit::write_sequence` uses. Leading comments on an item are written above the dash; a trailing comment goes at the end of the dash's line.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p xtask readme::yaml`
Expected: 4 passed.

- [ ] **Step 5: Pin the style to the tool's own emitter**

Add this test, which is what keeps the two from drifting:

```rust
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
    // Same structure and indentation; the only intended difference is the quoting style, which
    // this example deliberately avoids needing.
    assert_eq!(ours, shipped);
}
```

Run: `cargo test -p xtask readme::yaml`
Expected: 5 passed. If the shipped emitter disagrees about layout, the emitter here is wrong — fix it, not the test.

- [ ] **Step 6: Verify the lint gate**

Run: `cargo clippy -p xtask --all-targets -- -W clippy::pedantic -W clippy::cargo -W clippy::nursery -D warnings && cargo fmt --all --check`
Expected: exit 0.

---

### Task 3: The TOML emitter

**Files:**
- Create: `xtask/src/readme/toml.rs`
- Modify: `xtask/src/readme/mod.rs` (add `mod toml;`)

**Interfaces:**
- Consumes: `super::jsonc::{Node, Value}`.
- Produces: `pub fn to_string(node: &Node) -> String` — a TOML document ending in a newline.

Style, matching `examples/vendor.toml`: nested objects become `[table.headers]`; strings are literal single-quoted unless they contain a `'` or a control character, in which case they are basic strings with escapes; keys are bare when they match `[A-Za-z0-9_-]+` and quoted otherwise; arrays are inline when the whole line fits in 90 columns and multi-line with a two-space indent and a trailing comma when it does not; objects inside arrays are inline tables.

Within one table, scalar keys are emitted before sub-table headers because TOML requires it. That reordering is why Task 4's round-trip check exists.

- [ ] **Step 1: Write the failing test**

Create `xtask/src/readme/toml.rs` with only:

```rust
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
        assert_eq!(to_string(&node), "# above\na = 'x' # → after\n\n[b]\n#...\n");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p xtask readme::toml`
Expected: FAIL — `cannot find function \`to_string\``.

- [ ] **Step 3: Implement the emitter**

Write above the tests:

```rust
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
```

`write_table(out, props, path, owner)` does the work:

1. Write `owner`'s `leading` comments, then the `[path]` header when `path` is non-empty and this table has scalars or `inner` comments of its own, then `owner.trailing` on the header line, then the `inner` comments.
2. Emit every member whose value is a scalar or an array: leading comments on their own lines, then `{key} = {value}`, then ` #{trailing}`.
3. Recurse into every member whose value is a non-empty object, with `path` extended, preceded by a blank line.
4. A member whose value is an empty object with `inner` comments still gets its `[path.key]` header so the comments have a home.

The value helpers:

```rust
/// A value as it appears on the right of `=`.
fn inline(value: &Value) -> String {
    match value {
        Value::Null => "''".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.clone(),
        Value::String(s) => string(s),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(|i| inline(&i.value)).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(props) => {
            let parts: Vec<String> = props
                .iter()
                .map(|(k, v)| format!("{} = {}", key(k), inline(&v.value)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// Literal strings unless the content cannot survive them.
fn string(text: &str) -> String {
    if text.contains('\'') || text.chars().any(char::is_control) {
        return format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"));
    }
    format!("'{text}'")
}

fn key(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return text.to_owned();
    }
    string(text)
}
```

An array whose inline form would push the line past `WIDTH` is written as one element per line at a two-space indent with a trailing comma, exactly like `examples/vendor.toml`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p xtask readme::toml`
Expected: 4 passed.

- [ ] **Step 5: Verify the lint gate**

Run: `cargo clippy -p xtask --all-targets -- -W clippy::pedantic -W clippy::cargo -W clippy::nursery -D warnings && cargo fmt --all --check`
Expected: exit 0.

---

### Task 4: Group rendering and the round-trip check

Turns one JSON source into the three-`<details>` markdown region, and refuses to do so unless the YAML and TOML parse back to the same data.

**Files:**
- Create: `xtask/src/readme/render.rs`
- Modify: `xtask/src/readme/mod.rs` (add `mod render;`)

**Interfaces:**
- Consumes: `jsonc::parse`, `yaml::to_string`, `toml::to_string`.
- Produces:
  - `pub struct Labels { pub json: &'static str, pub yaml: &'static str, pub toml: &'static str }`
  - `pub const NAMES: Labels` = `JSON` / `YAML` / `TOML`
  - `pub const FILES: Labels` = `vendor.json` / `vendor.yml` / `vendor.toml`
  - `pub fn group(json_source: &str, fence: &str, labels: Labels) -> anyhow::Result<String>` — the full region body, from `<details open>` to the last `</details>`, ending with a newline. `fence` is the info string of the source block (`json` or `jsonc`) and is reused verbatim for the JSON tab.

- [ ] **Step 1: Write the failing test**

Create `xtask/src/readme/render.rs` with only:

```rust
#[cfg(test)]
mod tests {
    use super::{FILES, NAMES, group};

    #[test]
    fn renders_three_details_with_json_open() {
        let out = group(r#"{ "a": "x" }"#, "json", NAMES).unwrap();
        assert!(out.starts_with("<details open>\n<summary>JSON</summary>\n\n```json\n"), "{out}");
        assert_eq!(out.matches("<details").count(), 3);
        assert_eq!(out.matches("</details>").count(), 3);
        // A blank line after </summary> is what keeps GitHub's syntax highlighting working.
        assert_eq!(out.matches("</summary>\n\n```").count(), 3);
        assert!(out.contains("<summary>YAML</summary>\n\n```yml\na: x\n```"), "{out}");
        assert!(out.contains("<summary>TOML</summary>\n\n```toml\na = 'x'\n```"), "{out}");
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
        assert!(out.contains("# yaml-language-server: $schema=https://example.com/s.json"), "{out}");
        assert!(out.contains("#:schema https://example.com/s.json"), "{out}");
        // The key itself is gone from the two generated tabs.
        assert_eq!(out.matches("\"$schema\"").count(), 1, "{out}");
    }

    #[test]
    fn the_source_fence_tag_is_preserved() {
        let out = group(r#"{ "a": 1 // n
}"#, "jsonc", NAMES).unwrap();
        assert!(out.contains("```jsonc\n"), "{out}");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p xtask readme::render`
Expected: FAIL — `cannot find function \`group\``.

- [ ] **Step 3: Implement rendering**

```rust
//! Turns one example's JSON into the three collapsible blocks the README shows.

use anyhow::{Context, Result, bail};
use serde_json::Value as Json;

use super::jsonc::{self, Node, Value};
use super::{toml, yaml};

#[derive(Clone, Copy)]
pub struct Labels {
    pub json: &'static str,
    pub yaml: &'static str,
    pub toml: &'static str,
}

pub const NAMES: Labels = Labels { json: "JSON", yaml: "YAML", toml: "TOML" };
pub const FILES: Labels = Labels {
    json: "vendor.json",
    yaml: "vendor.yml",
    toml: "vendor.toml",
};

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
```

`split_schema` removes a top-level `$schema` string member and returns its URL, because in YAML and TOML the schema is a language-server directive rather than a key.

`verify` is the safety net:

```rust
/// Parses the generated text back with the same crates `vendorfiles_core` reads configs with,
/// and refuses output that does not mean what the source said.
fn verify(json_source: &str, yaml_text: &str, toml_text: &str) -> Result<()> {
    let expected = normalize(without_schema(
        jsonc_parser::parse_to_serde_value(json_source, &Default::default())?
            .unwrap_or(Json::Null),
    ));

    let from_yaml = normalize(serde_yaml_ng::from_str::<Json>(yaml_text)
        .context("re-reading the generated YAML")?);
    if from_yaml != expected {
        bail!("YAML differs:\n  expected {expected}\n  got      {from_yaml}");
    }

    let from_toml = normalize(toml_edit::de::from_str::<Json>(toml_text)
        .context("re-reading the generated TOML")?);
    if from_toml != expected {
        bail!("TOML differs:\n  expected {expected}\n  got      {from_toml}");
    }
    Ok(())
}

/// An empty map, an empty sequence and null compare equal. A container whose only content is a
/// comment has no other representation in YAML — it reads back as null.
fn normalize(value: Json) -> Json {
    match value {
        Json::Object(map) if map.is_empty() => Json::Null,
        Json::Array(items) if items.is_empty() => Json::Null,
        Json::Object(map) => Json::Object(map.into_iter().map(|(k, v)| (k, normalize(v))).collect()),
        Json::Array(items) => Json::Array(items.into_iter().map(normalize).collect()),
        other => other,
    }
}
```

`without_schema` drops a top-level `$schema` key from the comparison, since it is deliberately not a key in the generated formats.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p xtask readme::render`
Expected: 4 passed.

- [ ] **Step 5: Prove the safety net catches a real mistranslation**

Add:

```rust
#[test]
fn a_wrong_translation_is_rejected() {
    // A number that TOML would read back as a different type must not slip through.
    let out = group(r#"{ "a": 1 }"#, "json", NAMES).unwrap();
    assert!(out.contains("a = 1"), "{out}");
    assert!(super::verify(r#"{ "a": 1 }"#, "a: 2\n", "a = 1\n").is_err());
}
```

Run: `cargo test -p xtask readme::render`
Expected: 5 passed.

- [ ] **Step 6: Verify the lint gate**

Run: `cargo clippy -p xtask --all-targets -- -W clippy::pedantic -W clippy::cargo -W clippy::nursery -D warnings && cargo fmt --all --check`
Expected: exit 0.

---

### Task 5: The region scanner and the command

**Files:**
- Modify: `xtask/src/readme/mod.rs`
- Modify: `xtask/src/main.rs`

**Interfaces:**
- Consumes: `render::{FILES, NAMES, group}`.
- Produces:
  - `pub fn regenerate(markdown: &str) -> anyhow::Result<String>`
  - `pub fn run(check: bool) -> anyhow::Result<()>`

Marker grammar: a line whose trimmed text is `<!-- formats -->` or `<!-- formats: files -->` opens a region; the next line whose trimmed text is `<!-- /formats -->` closes it. Everything between is replaced by `group(...)` built from the first fenced `json` or `jsonc` block inside the region. Text outside a region is copied through untouched.

- [ ] **Step 1: Write the failing test**

In `xtask/src/readme/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::regenerate;

    const SOURCE: &str = "\
Intro paragraph.

<!-- formats -->
```json
{ \"a\": \"x\" }
```
<!-- /formats -->

Outro.
";

    #[test]
    fn fills_a_bare_region_from_its_json() {
        let out = regenerate(SOURCE).unwrap();
        assert!(out.starts_with("Intro paragraph.\n"), "{out}");
        assert!(out.ends_with("Outro.\n"), "{out}");
        assert!(out.contains("<summary>TOML</summary>"), "{out}");
        assert!(out.contains("a = 'x'"), "{out}");
    }

    #[test]
    fn is_idempotent() {
        let once = regenerate(SOURCE).unwrap();
        let twice = regenerate(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn leaves_unmarked_blocks_alone() {
        let source = "```json\n{ \"a\": 1 }\n```\n";
        assert_eq!(regenerate(source).unwrap(), source);
    }

    #[test]
    fn a_region_without_json_is_an_error() {
        let source = "<!-- formats -->\ntext\n<!-- /formats -->\n";
        let error = regenerate(source).unwrap_err().to_string();
        assert!(error.contains("no `json` or `jsonc` block"), "{error}");
    }

    #[test]
    fn the_committed_readme_is_up_to_date() {
        let path = crate::sh::workspace_root().unwrap().join("README.md");
        let source = std::fs::read_to_string(&path).unwrap();
        assert_eq!(regenerate(&source).unwrap(), source, "run `cargo xtask readme`");
    }
}
```

The last test will fail until Task 7 migrates the README; that is expected and is the point of it.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p xtask readme::tests`
Expected: FAIL — `cannot find function \`regenerate\``.

- [ ] **Step 3: Implement the scanner**

```rust
//! `cargo xtask readme` — regenerates the YAML and TOML tab of every marked config example.
//!
//! The README is the source of truth: each example is written once, in JSON, inside a
//! `<!-- formats -->` region, and the other two renderings are derived from it. GitHub strips
//! `<style>` and `class`, so three stacked `<details>` is as close to a tab strip as a README
//! can get.

mod jsonc;
mod render;
mod toml;
mod yaml;

use anyhow::{Context, Result, bail};

use crate::sh;

const OPEN: &str = "<!-- formats";
const OPEN_FILES: &str = "<!-- formats: files -->";
const CLOSE: &str = "<!-- /formats -->";

pub fn regenerate(markdown: &str) -> Result<String> { /* line walk, see below */ }

pub fn run(check: bool) -> Result<()> {
    let path = sh::workspace_root()?.join("README.md");
    let current = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let wanted = regenerate(&current)?;

    if current == wanted {
        println!("README.md is up to date.");
        return Ok(());
    }
    if check {
        bail!(
            "README.md is out of date at line {}; run `cargo xtask readme`",
            first_difference(&current, &wanted)
        );
    }
    std::fs::write(&path, &wanted).with_context(|| format!("writing {}", path.display()))?;
    println!("README.md regenerated.");
    Ok(())
}
```

`regenerate` walks the lines. Outside a region it copies. On an opening marker it copies the marker, chooses `render::FILES` when the line is `OPEN_FILES` and `render::NAMES` otherwise, collects the lines up to the closing marker, finds the first fenced block whose info string is `json` or `jsonc`, calls `render::group`, writes the result, then the closing marker. An unclosed region, a region with no such block, or a `group` failure is an error naming the line number.

`first_difference` returns the 1-based line number where the two strings diverge, so `--check` points at the section rather than dumping a diff.

- [ ] **Step 4: Wire up the command**

In `xtask/src/main.rs`, add `mod readme;`, then the variant and its arm:

```rust
    /// Regenerate the YAML and TOML tabs of the README's config examples.
    Readme {
        /// Fail instead of writing when the README is out of date.
        #[arg(long)]
        check: bool,
    },
```

```rust
        Command::Readme { check } => readme::run(check),
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p xtask readme`
Expected: every test passes except `the_committed_readme_is_up_to_date`, which fails until Task 7.

- [ ] **Step 6: Verify the command runs**

Run: `cargo run -p xtask -- readme --check`
Expected: exits 0 with "README.md is up to date." — no regions are marked yet, so nothing changes.

---

### Task 6: The CI gate

**Files:**
- Modify: `xtask/src/ci.rs`
- Modify: `.github/workflows/check.yml`

The gate goes in both, because `ci.rs` documents itself as running every gate the workflow applies.

- [ ] **Step 1: Add it to `xtask ci`**

In `xtask/src/ci.rs`, first in `run`, before the `cargo check` step — it is the cheapest of them:

```rust
    crate::readme::run(true)?;
```

Update the doc comment on `run` so the ordering note still describes what happens.

- [ ] **Step 2: Add it to the workflow**

In `.github/workflows/check.yml`, in the `lint` job, after the `Rustfmt check` step:

```yaml
      - name: README examples
        run: cargo run -p xtask -- readme --check
```

- [ ] **Step 3: Verify**

Run: `cargo run -p xtask -- ci`
Expected: the README step runs first and passes, then the existing gates run.

---

### Task 7: Migrate the README

Wraps all fourteen examples in markers, fixes the two content problems, and lets the generator fill in the rest.

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Mark the four already-converted groups**

For Quick Start, Basic Setup, and both Custom Output Paths examples: put `<!-- formats: files -->` above Quick Start's first `<details open>` and `<!-- formats -->` above the other three, and `<!-- /formats -->` after each group's final `</details>`. Delete the two generated `<details>` blocks from each group, leaving only the JSON one — the generator writes them back.

Change the per-dependency Custom Output Paths fence from ```` ```json5 ```` to ```` ```jsonc ````.

- [ ] **Step 2: Mark the nine remaining JSON examples**

For Renaming Files, Commit-Based Versioning, GitHub Releases (both), Filtering Releases, Locking Dependencies, the `fd` entry under Installing by name, and the self-update entry under Keeping vendor updated: wrap each existing ```` ```json ```` block in `<!-- formats -->` / `<!-- /formats -->`.

- [ ] **Step 3: Convert Default Options to a JSON source**

Its example is currently YAML only. Rewrite it as the equivalent JSON, wrapped in markers, so the generator produces the YAML and TOML tabs from it. Keep every key and value identical to what the YAML block says today.

- [ ] **Step 4: Handle the `$schema` example**

Wrap it in markers and change its fence to ```` ```jsonc ```` (it contains a `//...` placeholder, so it is not valid JSON).

- [ ] **Step 5: Fix the Windows path**

In the self-update example, `"vendorFolder": "C:\tools\bin"` becomes `"vendorFolder": "C:/tools/bin"`. As written, `\t` and `\b` are JSON escapes, so the value decodes to a tab and a backspace. Windows accepts forward slashes, so this is correct on every platform and needs no escaping.

- [ ] **Step 6: Rewrite the stale sentence**

Under `## Configuration`, "The first examples show every format; the rest are JSON only - TOML and YAML work identically. See [`examples/`](./examples/)." becomes a sentence that matches reality — every example is shown in all three formats, JSON open by default.

- [ ] **Step 7: Generate**

Run: `cargo run -p xtask -- readme`
Expected: "README.md regenerated." If it errors, it names the region and the reason — fix the source JSON, do not hand-edit the generated tabs.

- [ ] **Step 8: Verify the whole gate**

Run: `cargo run -p xtask -- readme --check && cargo test -p xtask && cargo run -p xtask -- ci`
Expected: all pass, including `the_committed_readme_is_up_to_date` from Task 5.

- [ ] **Step 9: Read the result**

Check the rendered diff: fourteen groups, JSON open by default, the `//` comments present as `#` comments in the YAML and TOML tabs, and no leftover `json5` fences. Leave everything in the working tree — no commit.
