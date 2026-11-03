//! `cargo xtask readme` - regenerates the YAML and TOML tab of every marked config example, and
//! the list of programs installable by name.
//!
//! The README is the source of truth for the examples: each is written once, in JSON, inside a
//! `<!-- formats -->` region, and the other two renderings are derived from it. GitHub strips
//! `<style>` and `class` from a README, so three stacked `<details>` is as close to a tab strip
//! as the page can get, and keeping three hand-written renderings in step is what this avoids.
//!
//! `registry.yml` is the source of truth for the program list. See [`registry`].

mod jsonc;
mod registry;
mod render;
mod toml;
mod yaml;

use anyhow::{Context, Result, anyhow, bail};

use crate::sh;

const OPEN_PREFIX: &str = "<!-- formats";
const OPEN_SUFFIX: &str = "-->";
const CLOSE: &str = "<!-- /formats -->";

/// Reads a region's opening marker, or returns `None` for any other line.
///
/// A marker is `<!-- formats -->` or `<!-- formats: options -->`, with comma-separated options:
///
/// - `files` labels the tabs by file name, for an example about creating the file
/// - `json first` (the default), `yml first` or `toml first` picks the tab that leads and is open
fn parse_open(line: &str) -> Result<Option<(render::Labels, render::Format)>> {
    let marker = line
        .trim()
        .strip_prefix(OPEN_PREFIX)
        .and_then(|rest| rest.strip_suffix(OPEN_SUFFIX));
    let Some(marker) = marker else {
        return Ok(None);
    };

    let mut labels = render::NAMES;
    let mut first = render::Format::Json;

    let options = marker.trim().strip_prefix(':').unwrap_or(marker);
    for option in options.split(',').map(str::trim) {
        match option {
            "" => {}
            "files" => labels = render::FILES,
            "json first" => first = render::Format::Json,
            "yml first" => first = render::Format::Yaml,
            "toml first" => first = render::Format::Toml,
            other => bail!("unknown option `{other}`"),
        }
    }
    Ok(Some((labels, first)))
}

/// Rewrites every marked region from the JSON inside it, leaving the rest of the file alone.
pub fn regenerate(markdown: &str) -> Result<String> {
    let source: Vec<&str> = markdown.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(source.len());

    let mut at = 0;
    while at < source.len() {
        let line = source[at];
        let opened = parse_open(line).with_context(|| format!("the marker at line {}", at + 1))?;
        let Some((labels, first)) = opened else {
            out.push(line.to_owned());
            at += 1;
            continue;
        };

        let close = (at + 1..source.len())
            .find(|&idx| source[idx].trim() == CLOSE)
            .ok_or_else(|| anyhow!("the region at line {} is never closed by `{CLOSE}`", at + 1))?;

        let (fence, example) = source_block(&source[at + 1..close])
            .with_context(|| format!("the region at line {}", at + 1))?;
        let rendered = render::group(&example, &fence, labels, first)
            .with_context(|| format!("the example at line {}", at + 1))?;

        out.push(line.to_owned());
        out.extend(
            rendered
                .trim_end_matches('\n')
                .split('\n')
                .map(str::to_owned),
        );
        out.push(source[close].to_owned());
        at = close + 1;
    }

    Ok(out.join("\n"))
}

/// The first `json` or `jsonc` block in a region: the one example everything else is derived
/// from.
fn source_block(region: &[&str]) -> Result<(String, String)> {
    let opened = region.iter().enumerate().find_map(|(idx, line)| {
        let info = line.trim_start().strip_prefix("```")?.trim();
        matches!(info, "json" | "jsonc").then(|| (idx, info.to_owned()))
    });
    let Some((start, fence)) = opened else {
        bail!("holds no `json` or `jsonc` block to generate the other formats from");
    };

    let end = region[start + 1..]
        .iter()
        .position(|line| line.trim() == "```")
        .ok_or_else(|| anyhow!("the `{fence}` block is never closed"))?
        + start
        + 1;

    Ok((fence, region[start + 1..end].join("\n")))
}

/// Regenerates `README.md`, or with `check` reports that it needs regenerating.
pub fn run(check: bool) -> Result<()> {
    let root = sh::workspace_root()?;
    let path = root.join("README.md");
    let current =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let wanted = registry::regenerate(&regenerate(&current)?, &read_registry(&root)?)?;

    if current == wanted {
        println!("README.md is up to date.");
        return Ok(());
    }
    if check {
        bail!(
            "README.md is out of date, from line {}; run `cargo xtask readme`",
            first_difference(&current, &wanted)
        );
    }
    std::fs::write(&path, &wanted).with_context(|| format!("writing {}", path.display()))?;
    println!("README.md regenerated.");
    Ok(())
}

/// The registry the README lists.
fn read_registry(root: &std::path::Path) -> Result<String> {
    let path = root.join("registry.yml");
    std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
}

/// The 1-based line the two texts start to disagree on, so `--check` points at the section
/// rather than printing a diff of a file this size.
fn first_difference(current: &str, wanted: &str) -> usize {
    current
        .split('\n')
        .zip(wanted.split('\n'))
        .position(|(a, b)| a != b)
        .map_or_else(
            || current.split('\n').count().min(wanted.split('\n').count()) + 1,
            |at| at + 1,
        )
}

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

    fn region(marker: &str) -> String {
        format!("{marker}\n```json\n{{ \"a\": \"x\" }}\n```\n<!-- /formats -->\n")
    }

    #[test]
    fn marker_options_pick_the_labels_and_the_first_tab() {
        for (marker, summary) in [
            ("<!-- formats -->", "JSON"),
            ("<!-- formats: files -->", "vendor.json"),
            ("<!-- formats: json first -->", "JSON"),
            ("<!-- formats: yml first -->", "YAML"),
            ("<!-- formats: toml first -->", "TOML"),
            ("<!-- formats: files, yml first -->", "vendor.yml"),
            ("<!-- formats: toml first, files -->", "vendor.toml"),
        ] {
            let once = regenerate(&region(marker)).unwrap();
            let opening = format!("{marker}\n<details open>\n<summary>{summary}</summary>");
            assert!(once.starts_with(&opening), "{once}");
            assert_eq!(once, regenerate(&once).unwrap());
        }
    }

    #[test]
    fn an_unknown_marker_option_is_an_error() {
        let error = format!(
            "{:#}",
            regenerate(&region("<!-- formats: yaml first -->")).unwrap_err()
        );
        assert!(error.contains("line 1"), "{error}");
        assert!(error.contains("unknown option `yaml first`"), "{error}");
    }

    #[test]
    fn leaves_unmarked_blocks_alone() {
        let source = "```json\n{ \"a\": 1 }\n```\n";
        assert_eq!(regenerate(source).unwrap(), source);
    }

    #[test]
    fn a_region_without_json_is_an_error() {
        let source = "<!-- formats -->\ntext\n<!-- /formats -->\n";
        let error = format!("{:#}", regenerate(source).unwrap_err());
        assert!(error.contains("no `json` or `jsonc` block"), "{error}");
    }

    #[test]
    fn an_unclosed_region_is_an_error() {
        let source = "<!-- formats -->\n```json\n{}\n```\n";
        let error = format!("{:#}", regenerate(source).unwrap_err());
        assert!(error.contains("never closed"), "{error}");
    }

    #[test]
    fn the_committed_readme_is_up_to_date() {
        let root = crate::sh::workspace_root().unwrap();
        let source = std::fs::read_to_string(root.join("README.md")).unwrap();
        let registry = super::read_registry(&root).unwrap();
        assert_eq!(
            super::registry::regenerate(&regenerate(&source).unwrap(), &registry).unwrap(),
            source,
            "run `cargo xtask readme`"
        );
    }
}
