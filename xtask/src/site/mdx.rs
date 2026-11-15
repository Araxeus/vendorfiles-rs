//! README Markdown rewritten as Starlight MDX.
//!
//! MDX reads `{` and `<` in prose as code, so both are escaped outside code. Format regions
//! become synced tabs and the program list gets a filter box.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};

use super::BASE;
use crate::readme::render::{self, Format};
use crate::readme::{CLOSE, parse_open, source_block};

const REPO_FILES: &str = "https://github.com/Araxeus/vendorfiles-rs/blob/main/";
const REGISTRY_OPEN: &str = "<!-- registry -->";
const REGISTRY_CLOSE: &str = "<!-- /registry -->";

/// The repo file that is a page of its own, and that page.
const PAGE_FILES: [(&str, &str); 1] = [("docs/add_to_registry.md", "programs/add")];

/// Where a README anchor lives on the site.
pub enum Target {
    /// The heading became this page's title.
    Page(String),
    /// A heading inside a page.
    Heading { page: String, anchor: String },
}

/// What links resolve against.
pub struct Links<'a> {
    pub anchors: &'a HashMap<String, Target>,
    /// The source file's folder, relative to the repo root.
    pub source_dir: &'a str,
}

/// A page body ready for MDX, and the components it needs imported.
#[derive(Debug)]
pub struct Converted {
    pub lines: Vec<String>,
    pub has_tabs: bool,
    pub has_registry: bool,
}

/// Rewrites Markdown lines as MDX.
pub fn convert(source: &[String], links: &Links) -> Result<Converted> {
    let mut out = Converted {
        lines: Vec::new(),
        has_tabs: false,
        has_registry: false,
    };
    let mut in_fence = false;
    let mut at = 0;

    while at < source.len() {
        let line = &source[at];
        at += 1;

        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.lines.push(line.clone());
            continue;
        }
        if in_fence {
            out.lines.push(line.clone());
            continue;
        }

        if let Some(marker) = parse_open(line)? {
            let close = find(source, at, CLOSE)?;
            out.lines.extend(tabs(&source[at..close], &marker)?);
            out.has_tabs = true;
            at = close + 1;
            continue;
        }

        if line.trim() == REGISTRY_OPEN {
            let close = find(source, at, REGISTRY_CLOSE)?;
            out.lines
                .extend(["<ProgramFilter />".to_owned(), String::new()]);
            for row in &source[at..close] {
                if !row.starts_with("<details")
                    && !row.starts_with("<summary")
                    && !row.starts_with("</details")
                {
                    out.lines.push(prose(row, links)?);
                }
            }
            out.has_registry = true;
            at = close + 1;
            continue;
        }

        if let Some(kind) = alert(line)? {
            let mut quoted = Vec::new();
            while at < source.len() && source[at].trim_start().starts_with('>') {
                let rest = &source[at].trim_start()[1..];
                quoted.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
                at += 1;
            }
            let inner = convert(&quoted, links)?;
            out.has_tabs |= inner.has_tabs;
            out.has_registry |= inner.has_registry;
            out.lines.push(format!(":::{kind}"));
            out.lines.extend(inner.lines);
            out.lines.push(":::".to_owned());
            continue;
        }

        let trimmed = line.trim();
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            continue;
        }

        // A paragraph's lines go through together, so a code span can wrap. A table row or a
        // heading stands alone.
        let mut end = at;
        if !starts_block(line) || is_list_item(line) {
            while end < source.len() && !starts_block(&source[end]) {
                end += 1;
            }
        }
        let paragraph = std::iter::once(line.as_str())
            .chain(source[at..end].iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        out.lines
            .extend(prose(&paragraph, links)?.split('\n').map(str::to_owned));
        at = end;
    }
    Ok(out)
}

/// Whether a line ends the paragraph before it.
fn starts_block(line: &str) -> bool {
    let line = line.trim_start();
    line.is_empty()
        || ["|", "#", "```", ">", "<!--"]
            .iter()
            .any(|start| line.starts_with(start))
        || is_list_item(line)
}

fn is_list_item(line: &str) -> bool {
    let line = line.trim_start();
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    ["- ", "* ", "+ "]
        .iter()
        .any(|bullet| line.starts_with(bullet))
        || (digits > 0 && (line[digits..].starts_with(". ") || line[digits..].starts_with(") ")))
}

/// The Starlight aside a GitHub alert line opens, like `> [!NOTE]`.
fn alert(line: &str) -> Result<Option<&'static str>> {
    let Some(kind) = line
        .trim()
        .strip_prefix("> [!")
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return Ok(None);
    };
    Ok(Some(match kind {
        "NOTE" => "note",
        "TIP" => "tip",
        "IMPORTANT" => "note[Important]",
        "WARNING" => "caution",
        "CAUTION" => "danger",
        other => bail!("`> [!{other}]` is not a GitHub alert"),
    }))
}

/// The index of the line that closes a region opened before `from`.
fn find(lines: &[String], from: usize, close: &str) -> Result<usize> {
    (from..lines.len())
        .find(|&idx| lines[idx].trim() == close)
        .ok_or_else(|| anyhow!("a region before line {from} is never closed by `{close}`"))
}

/// A format region as three synced tabs, generated from its JSON.
fn tabs(region: &[String], marker: &crate::readme::Marker) -> Result<Vec<String>> {
    let region: Vec<&str> = region.iter().map(String::as_str).collect();
    let (fence, example) = source_block(&region)?;
    let texts = render::texts(&example, &fence)?;

    let mut tabs = [
        (
            Format::Json,
            "JSON",
            "vendor.json",
            texts.json_fence.as_str(),
            texts.json.as_str(),
        ),
        (
            Format::Yaml,
            "YAML",
            "vendor.yml",
            "yml",
            texts.yaml.trim_end(),
        ),
        (
            Format::Toml,
            "TOML",
            "vendor.toml",
            "toml",
            texts.toml.trim_end(),
        ),
    ];

    // The sort is stable, so the other two tabs keep their order.
    tabs.sort_by_key(|(format, ..)| *format != marker.first);

    let mut out = vec!["<Tabs syncKey=\"format\">".to_owned()];
    for (_, label, file, fence, body) in tabs {
        let info = if marker.files {
            format!("{fence} title=\"{file}\"")
        } else {
            fence.to_owned()
        };
        out.extend([
            format!("<TabItem label=\"{label}\">"),
            String::new(),
            format!("```{info}"),
        ]);
        out.extend(body.split('\n').map(str::to_owned));
        out.extend(["```".to_owned(), String::new(), "</TabItem>".to_owned()]);
    }
    out.push("</Tabs>".to_owned());
    Ok(out)
}

/// Prose, with code spans kept verbatim and everything else made MDX-safe.
///
/// A code span opens with a run of backticks and closes at the next run of the same length,
/// which may sit on a later line of the same paragraph. A run that never closes is plain text.
fn prose(source: &str, links: &Links) -> Result<String> {
    let mut out = String::new();
    let mut plain = 0;
    let mut at = 0;

    while let Some(offset) = source[at..].find('`') {
        let open = at + offset;
        let ticks = backtick_run(source, open);
        match closing_run(source, open + ticks, ticks) {
            Some(close) => {
                out.push_str(&text(&source[plain..open], links)?);
                out.push_str(&source[open..close + ticks]);
                plain = close + ticks;
                at = plain;
            }
            None => at = open + ticks,
        }
    }
    out.push_str(&text(&source[plain..], links)?);
    Ok(out)
}

fn backtick_run(source: &str, from: usize) -> usize {
    source[from..]
        .bytes()
        .take_while(|&byte| byte == b'`')
        .count()
}

/// Where the next run of exactly `ticks` backticks starts.
fn closing_run(source: &str, from: usize, ticks: usize) -> Option<usize> {
    let mut at = from;
    while let Some(offset) = source[at..].find('`') {
        let start = at + offset;
        let len = backtick_run(source, start);
        if len == ticks {
            return Some(start);
        }
        at = start + len;
    }
    None
}

/// Text outside code: comments dropped, links resolved, then `{`, `}` and `<` escaped.
fn text(segment: &str, links: &Links) -> Result<String> {
    let mut segment = segment.to_owned();
    while let Some(start) = segment.find("<!--") {
        let end = segment[start..]
            .find("-->")
            .map_or(segment.len(), |at| start + at + 3);
        segment.replace_range(start..end, "");
    }

    let mut out = String::new();
    let mut rest = segment.as_str();
    while let Some(at) = rest.find("](") {
        let target_start = at + 2;
        let Some(len) = rest[target_start..].find(')') else {
            break;
        };
        out.push_str(&escape(&rest[..target_start]));
        out.push_str(&resolve(&rest[target_start..target_start + len], links)?);
        out.push(')');
        rest = &rest[target_start + len + 1..];
    }
    out.push_str(&escape(rest));
    Ok(out)
}

/// Escapes what MDX would read as an expression or a tag. `<br>` is the one tag the README
/// uses in prose, so it becomes the self-closing form JSX requires.
fn escape(text: &str) -> String {
    text.split("<br>")
        .map(|part| {
            part.replace('{', "\\{")
                .replace('}', "\\}")
                .replace('<', "&lt;")
        })
        .collect::<Vec<_>>()
        .join("<br />")
}

/// A link target as the site serves it.
fn resolve(target: &str, links: &Links) -> Result<String> {
    if target.contains("://") || target.starts_with("mailto:") || target.starts_with('/') {
        return Ok(target.to_owned());
    }
    if let Some(anchor) = target.strip_prefix('#') {
        return match links.anchors.get(anchor) {
            Some(Target::Page(page)) => Ok(page_url(page)),
            Some(Target::Heading { page, anchor }) => Ok(format!("{}#{anchor}", page_url(page))),
            None => bail!("the link to `#{anchor}` names no heading in the README"),
        };
    }

    let (path, fragment) = target
        .split_once('#')
        .map_or((target, None), |(p, f)| (p, Some(f)));
    let path = normalize(links.source_dir, path)
        .with_context(|| format!("the link to `{target}` leaves the repository"))?;
    let fragment = fragment.map_or_else(String::new, |f| format!("#{f}"));
    match PAGE_FILES.iter().find(|(file, _)| *file == path) {
        Some((_, page)) => Ok(format!("{}{fragment}", page_url(page))),
        None => Ok(format!("{REPO_FILES}{path}{fragment}")),
    }
}

fn page_url(page: &str) -> String {
    if page == "index" {
        BASE.to_owned()
    } else {
        format!("{BASE}{page}/")
    }
}

/// `path` joined onto `dir` with `.` and `..` folded away, keeping a trailing `/`.
fn normalize(dir: &str, path: &str) -> Result<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or_else(|| anyhow!("too many `..`"))?;
            }
            part => parts.push(part),
        }
    }
    let trailing = if path.ends_with('/') { "/" } else { "" };
    Ok(format!("{}{trailing}", parts.join("/")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{Links, Target, convert};

    fn lines(text: &str) -> Vec<String> {
        text.split('\n').map(str::to_owned).collect()
    }

    fn anchors() -> HashMap<String, Target> {
        HashMap::from([
            (
                "paths".to_owned(),
                Target::Page("configuration/paths".to_owned()),
            ),
            (
                "renaming-files".to_owned(),
                Target::Heading {
                    page: "configuration/paths".to_owned(),
                    anchor: "renaming-files".to_owned(),
                },
            ),
        ])
    }

    fn convert_readme(text: &str) -> anyhow::Result<super::Converted> {
        let anchors = anchors();
        convert(
            &lines(text),
            &Links {
                anchors: &anchors,
                source_dir: "",
            },
        )
    }

    #[test]
    fn inline_code_is_untouched_and_prose_braces_are_escaped() {
        let out = convert_readme("Use `{version}` or {x} and `a\\|b`<br>").unwrap();
        assert_eq!(out.lines, ["Use `{version}` or \\{x\\} and `a\\|b`<br />"]);
    }

    #[test]
    fn a_page_title_anchor_drops_the_hash() {
        let out = convert_readme("[p](#paths) [r](#renaming-files)").unwrap();
        assert_eq!(
            out.lines,
            [
                "[p](/vendorfiles-rs/configuration/paths/) [r](/vendorfiles-rs/configuration/paths/#renaming-files)"
            ]
        );
    }

    #[test]
    fn an_unknown_anchor_is_an_error() {
        let error = format!("{:#}", convert_readme("[x](#nope)").unwrap_err());
        assert!(error.contains("#nope"), "{error}");
    }

    #[test]
    fn repo_paths_become_github_urls() {
        let out = convert_readme("[e](./examples/) [d](./docs/add_to_registry.md)").unwrap();
        assert_eq!(
            out.lines,
            [
                "[e](https://github.com/Araxeus/vendorfiles-rs/blob/main/examples/) [d](/vendorfiles-rs/programs/add/)"
            ]
        );
    }

    #[test]
    fn paths_resolve_against_the_source_folder() {
        let anchors = anchors();
        let links = Links {
            anchors: &anchors,
            source_dir: "docs",
        };
        let out = convert(&lines("[r](../registry.yml) [u](https://x.dev/a)"), &links).unwrap();
        assert_eq!(
            out.lines,
            [
                "[r](https://github.com/Araxeus/vendorfiles-rs/blob/main/registry.yml) [u](https://x.dev/a)"
            ]
        );
    }

    #[test]
    fn a_files_region_becomes_synced_tabs_with_titles() {
        let region =
            "<!-- formats: files, yml first -->\n```json\n{ \"a\": \"x\" }\n```\n<!-- /formats -->";
        let out = convert_readme(region).unwrap();
        let text = out.lines.join("\n");
        assert!(out.has_tabs);
        assert!(
            text.starts_with(
                "<Tabs syncKey=\"format\">\n<TabItem label=\"YAML\">\n\n```yml title=\"vendor.yml\""
            ),
            "{text}"
        );
        assert!(
            text.contains("```json title=\"vendor.json\"\n{ \"a\": \"x\" }\n```"),
            "{text}"
        );
        assert!(text.ends_with("</Tabs>"), "{text}");
    }

    #[test]
    fn a_plain_region_keeps_json_first_and_no_titles() {
        let out = convert_readme("<!-- formats -->\n```json\n{ \"a\": 1 }\n```\n<!-- /formats -->")
            .unwrap();
        let text = out.lines.join("\n");
        assert!(
            text.starts_with("<Tabs syncKey=\"format\">\n<TabItem label=\"JSON\">\n\n```json\n"),
            "{text}"
        );
        assert!(!text.contains("title="), "{text}");
    }

    #[test]
    fn a_registry_region_becomes_a_filtered_table() {
        let region = "<!-- registry -->\n<details>\n<summary>Programs (1)</summary>\n\n| A |\n| --- |\n| `b` |\n\n</details>\n<!-- /registry -->";
        let out = convert_readme(region).unwrap();
        assert!(out.has_registry);
        assert_eq!(
            out.lines,
            [
                "<ProgramFilter />",
                "",
                "",
                "| A |",
                "| --- |",
                "| `b` |",
                ""
            ]
        );
    }

    #[test]
    fn comments_are_removed_and_stray_angles_escaped() {
        let out = convert_readme("<!-- note -->\na <b> c <!-- x --> d").unwrap();
        assert_eq!(out.lines, ["a &lt;b> c  d"]);
    }

    #[test]
    fn a_code_span_may_wrap_inside_a_paragraph() {
        let out = convert_readme("Use `{a\nb}` here {c}").unwrap();
        assert_eq!(out.lines, ["Use `{a", "b}` here \\{c\\}"]);
    }

    #[test]
    fn a_code_span_never_spans_table_rows() {
        let out = convert_readme("| ` |\n| {c} ` |").unwrap();
        assert_eq!(out.lines, ["| ` |", "| \\{c\\} ` |"]);
    }

    #[test]
    fn an_unclosed_backtick_is_literal() {
        let out = convert_readme("a `` b `{c}` {d}").unwrap();
        assert_eq!(out.lines, ["a `` b `{c}` \\{d\\}"]);
    }

    #[test]
    fn a_github_alert_becomes_an_aside() {
        let out = convert_readme("> [!WARNING]\n> Mind `{x}` and {y}.\n> Second line.").unwrap();
        assert_eq!(
            out.lines,
            [
                ":::caution",
                "Mind `{x}` and \\{y\\}.",
                "Second line.",
                ":::"
            ]
        );
    }

    #[test]
    fn an_important_alert_keeps_its_label() {
        let out = convert_readme("> [!IMPORTANT]\n> Hi").unwrap();
        assert_eq!(out.lines, [":::note[Important]", "Hi", ":::"]);
    }

    #[test]
    fn fenced_code_is_untouched() {
        let out = convert_readme("```toml\na = '{x}' # <y>\n```").unwrap();
        assert_eq!(out.lines.join("\n"), "```toml\na = '{x}' # <y>\n```");
    }
}
