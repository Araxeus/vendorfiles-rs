//! Which README sections make up which site page, and the pages built from them.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use serde_json::json;

use super::BASE;
use super::mdx::{self, Links, Target};
use super::sections::{self, Section};

/// Where a page's content starts.
pub enum Start {
    /// The README's intro, under its `#` title.
    Intro,
    /// A README heading. The page runs until the next start.
    Heading(&'static str),
    /// A whole file of its own, by repo path.
    File(&'static str),
    /// A README heading whose sections stay off the site.
    Drop(&'static str),
}

pub struct PageSpec {
    pub slug: &'static str,
    /// The page title when the heading's own text reads badly in a sidebar.
    pub title: Option<&'static str>,
    pub starts_at: Start,
}

const fn page(slug: &'static str, title: Option<&'static str>, starts_at: Start) -> PageSpec {
    PageSpec {
        slug,
        title,
        starts_at,
    }
}

const fn drop(heading: &'static str) -> PageSpec {
    page("", None, Start::Drop(heading))
}

/// Every page, in README order. Each `##` heading of the README must start one.
pub const PAGES: &[PageSpec] = &[
    page("index", Some("vendorfiles"), Start::Intro),
    drop("Table of Contents"),
    page(
        "getting-started/installation",
        None,
        Start::Heading("Installation"),
    ),
    page(
        "getting-started/quick-start",
        None,
        Start::Heading("Quick Start"),
    ),
    page(
        "getting-started/global-programs",
        None,
        Start::Heading("Global Programs"),
    ),
    page(
        "configuration/config-files",
        Some("Config files"),
        Start::Heading("Configuration"),
    ),
    page(
        "configuration/paths",
        Some("Output paths and renaming"),
        Start::Heading("Custom Output Paths"),
    ),
    page(
        "configuration/versions",
        Some("Versions and releases"),
        Start::Heading("Commit-Based Versioning"),
    ),
    page(
        "configuration/locking",
        Some("Locking"),
        Start::Heading("Locking Dependencies"),
    ),
    page(
        "configuration/defaults",
        Some("Defaults"),
        Start::Heading("Default Options"),
    ),
    page("reference/commands", None, Start::Heading("Commands")),
    page(
        "programs/by-name",
        None,
        Start::Heading("Installing by name"),
    ),
    page(
        "reference/self-update",
        Some("Self-update"),
        Start::Heading("Keeping vendorfiles updated"),
    ),
    page(
        "reference/authentication",
        None,
        Start::Heading("Authentication"),
    ),
    page("reference/lockfile", None, Start::Heading("Lockfile")),
    page("reference/json-schema", None, Start::Heading("JSON Schema")),
    page(
        "about/differences",
        None,
        Start::Heading("Differences from the TypeScript version"),
    ),
    page("about/development", None, Start::Heading("Development")),
    drop("License"),
    page(
        "programs/add",
        Some("Add a program"),
        Start::File("docs/add_to_registry.md"),
    ),
];

/// The sidebar groups and their pages, in display order.
pub const SIDEBAR: &[(&str, &[&str])] = &[
    (
        "Getting started",
        &[
            "getting-started/installation",
            "getting-started/quick-start",
            "getting-started/global-programs",
        ],
    ),
    (
        "Configuration",
        &[
            "configuration/config-files",
            "configuration/paths",
            "configuration/versions",
            "configuration/locking",
            "configuration/defaults",
        ],
    ),
    ("Programs by name", &["programs/by-name", "programs/add"]),
    (
        "Reference",
        &[
            "reference/commands",
            "reference/self-update",
            "reference/authentication",
            "reference/lockfile",
            "reference/json-schema",
        ],
    ),
    ("About", &["about/differences", "about/development"]),
];

const TAGLINE: &str = "Pull files from GitHub repositories and keep them up to date.";

/// One generated page, at a path under `site/src/content/docs/`.
#[derive(Debug)]
pub struct Page {
    pub path: String,
    pub content: String,
}

/// A page's spec and the sections it owns, in order.
struct Owned<'a> {
    spec: &'a PageSpec,
    title: String,
    sections: Vec<Section>,
}

/// Builds every page and the sidebar JSON from the README and the files that are pages.
pub fn generate(readme: &str, add_to_registry: &str) -> Result<(Vec<Page>, String)> {
    let (_, sections) = sections::split(readme).context("reading README.md")?;
    let owned = assign(sections)?;
    let anchors = anchor_map(&owned);

    let mut pages = Vec::new();
    for page in &owned {
        let links = Links {
            anchors: &anchors,
            source_dir: "",
        };
        pages.push(render(page, &links).with_context(|| format!("the `{}` page", page.spec.slug))?);
    }

    for spec in PAGES
        .iter()
        .filter(|spec| matches!(spec.starts_at, Start::File(_)))
    {
        let Start::File(path) = spec.starts_at else {
            unreachable!()
        };
        let (_, sections) =
            sections::split(add_to_registry).with_context(|| format!("reading {path}"))?;
        let Some(first) = sections.first() else {
            bail!("{path} has no heading to title its page");
        };
        let page = Owned {
            spec,
            title: first.title.clone(),
            sections,
        };
        let anchors = anchor_map(std::slice::from_ref(&page));
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let links = Links {
            anchors: &anchors,
            source_dir: dir,
        };
        pages.push(render(&page, &links).with_context(|| format!("the `{}` page", spec.slug))?);
    }

    Ok((pages, sidebar()))
}

/// Hands each README section to the page whose start comes last before it.
fn assign(sections: Vec<Section>) -> Result<Vec<Owned<'static>>> {
    let mut owned: Vec<Owned> = Vec::new();
    let mut dropping = false;

    for (idx, section) in sections.into_iter().enumerate() {
        if idx == 0 && section.level == 1 {
            let spec = PAGES
                .iter()
                .find(|spec| matches!(spec.starts_at, Start::Intro))
                .expect("an intro page");
            owned.push(Owned {
                spec,
                title: section.title.clone(),
                sections: vec![section],
            });
            continue;
        }

        let start = PAGES.iter().find(|spec| match spec.starts_at {
            Start::Heading(title) | Start::Drop(title) => title == section.title,
            Start::Intro | Start::File(_) => false,
        });
        match start {
            Some(spec) if matches!(spec.starts_at, Start::Drop(_)) => dropping = true,
            Some(spec) => {
                dropping = false;
                owned.push(Owned {
                    spec,
                    title: section.title.clone(),
                    sections: vec![section],
                });
            }
            None if section.level <= 2 => bail!(
                "README heading `{} {}` is in no page; add it to PAGES in xtask/src/site/pages.rs",
                "#".repeat(section.level),
                section.title
            ),
            None if dropping => {}
            None => owned
                .last_mut()
                .expect("the intro comes first")
                .sections
                .push(section),
        }
    }

    for spec in PAGES {
        if let Start::Heading(title) | Start::Drop(title) = spec.starts_at {
            let found = owned.iter().any(|page| page.sections[0].title == title)
                || matches!(spec.starts_at, Start::Drop(_));
            if !found {
                bail!("PAGES starts a page at `{title}`, but the README has no such heading");
            }
        }
    }
    Ok(owned)
}

/// Every heading's anchor, mapped to where it lands on the site.
fn anchor_map(pages: &[Owned]) -> HashMap<String, Target> {
    let mut anchors = HashMap::new();
    for page in pages {
        let slug = page.spec.slug.to_owned();
        for (idx, section) in page.sections.iter().enumerate() {
            let target = if idx == 0 {
                Target::Page(slug.clone())
            } else {
                Target::Heading {
                    page: slug.clone(),
                    anchor: section.slug.clone(),
                }
            };
            anchors.insert(section.slug.clone(), target);
        }
    }
    anchors
}

/// One page as MDX: frontmatter, imports, then the converted body.
fn render(page: &Owned, links: &Links) -> Result<Page> {
    let mut body: Vec<String> = Vec::new();
    for (idx, section) in page.sections.iter().enumerate() {
        if idx > 0 {
            body.push(format!(
                "{} {}",
                "#".repeat(section.level - 1),
                section.title
            ));
        }
        body.extend(section.body.iter().cloned());
    }
    if matches!(page.spec.starts_at, Start::Intro) {
        body = intro_lines(&body);
    }

    let converted = mdx::convert(&body, links)?;
    let depth = page.spec.slug.matches('/').count();

    let mut content = frontmatter(page);
    if converted.has_tabs {
        content.push_str("import { Tabs, TabItem } from '@astrojs/starlight/components';\n");
    }
    if converted.has_registry {
        let up = "../".repeat(depth + 2);
        let _ = writeln!(
            content,
            "import ProgramFilter from '{up}components/ProgramFilter.astro';"
        );
    }
    content.push('\n');
    content.push_str(converted.lines.join("\n").trim_matches('\n'));
    content.push('\n');

    Ok(Page {
        path: format!("{}.mdx", page.spec.slug),
        content,
    })
}

fn frontmatter(page: &Owned) -> String {
    let title = page.spec.title.unwrap_or(&page.title);
    let mut out = format!("---\ntitle: {}\n", yaml_string(title));
    if matches!(page.spec.starts_at, Start::Intro) {
        let _ = write!(
            out,
            "template: splash\n\
             hero:\n  tagline: {}\n  actions:\n    \
             - text: Get started\n      link: {BASE}getting-started/installation/\n      icon: right-arrow\n    \
             - text: GitHub\n      link: https://github.com/Araxeus/vendorfiles-rs\n      icon: external\n      variant: minimal\n",
            yaml_string(TAGLINE)
        );
    }
    out.push_str("---\n");
    out
}

/// A YAML scalar: bare when it is plain words, quoted otherwise.
fn yaml_string(text: &str) -> String {
    let plain = text
        .chars()
        .all(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '-' | '.' | ','))
        && !text.starts_with(['-', ' '])
        && !text.ends_with(' ');
    if plain {
        text.to_owned()
    } else {
        serde_json::Value::from(text).to_string()
    }
}

/// The intro without the badge row and the README-only banner.
pub(super) fn intro_lines(body: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_banner = false;
    for line in body {
        let trimmed = line.trim_start();
        if trimmed.starts_with("<p") {
            in_banner = true;
        }
        if in_banner {
            in_banner = !trimmed.contains("</p>");
            continue;
        }
        if !trimmed.starts_with("[![") {
            out.push(line.clone());
        }
    }
    out
}

fn sidebar() -> String {
    let groups: Vec<_> = SIDEBAR
        .iter()
        .map(|(label, slugs)| {
            json!({ "label": label, "items": slugs.iter().map(|slug| json!({ "slug": slug })).collect::<Vec<_>>() })
        })
        .collect();
    serde_json::Value::from(groups).to_string()
}

#[cfg(test)]
mod tests {
    use super::{PAGES, SIDEBAR, Start, generate};

    fn readme() -> String {
        std::fs::read_to_string(crate::sh::workspace_root().unwrap().join("README.md")).unwrap()
    }

    const ADD: &str = "### Adding a program to the registry\n";

    #[test]
    fn the_committed_readme_generates_every_page() {
        let root = crate::sh::workspace_root().unwrap();
        let add = std::fs::read_to_string(root.join("docs/add_to_registry.md")).unwrap();
        let (pages, sidebar) = generate(&readme(), &add).unwrap();
        let paths: Vec<_> = pages.iter().map(|p| p.path.as_str()).collect();
        assert!(paths.contains(&"index.mdx"));
        assert!(paths.contains(&"configuration/versions.mdx"));
        assert_eq!(
            paths.len(),
            PAGES
                .iter()
                .filter(|p| !matches!(p.starts_at, Start::Drop(_)))
                .count()
        );
        assert!(
            sidebar.contains("\"slug\":\"reference/commands\""),
            "{sidebar}"
        );
    }

    #[test]
    fn an_unmapped_section_names_the_map() {
        let readme = readme().replace("## Lockfile", "## Brand New\n\ntext\n\n## Lockfile");
        let error = format!("{:#}", generate(&readme, ADD).unwrap_err());
        assert!(
            error.contains("`## Brand New`") && error.contains("xtask/src/site/pages.rs"),
            "{error}"
        );
    }

    #[test]
    fn a_missing_start_heading_is_named() {
        let readme = readme().replace("### Global Programs", "### Global programs");
        let error = format!("{:#}", generate(&readme, ADD).unwrap_err());
        assert!(error.contains("`Global Programs`"), "{error}");
    }

    #[test]
    fn headings_move_up_one_level() {
        let (pages, _) = generate(&readme(), ADD).unwrap();
        let paths = pages
            .iter()
            .find(|p| p.path == "configuration/paths.mdx")
            .unwrap();
        assert!(
            paths
                .content
                .contains("\ntitle: Output paths and renaming\n")
        );
        assert!(
            paths.content.contains("\n## Renaming Files\n"),
            "{}",
            paths.content
        );
    }

    #[test]
    fn the_splash_page_drops_badges_and_banner() {
        let body: Vec<String> = [
            "",
            "[![x](y)](z)",
            "<p align=\"center\">",
            "  <img src=\"b.svg\">",
            "</p>",
            "Intro text.",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(super::intro_lines(&body), ["", "Intro text."]);
    }

    #[test]
    fn imports_match_the_page_depth() {
        let (pages, _) = generate(&readme(), ADD).unwrap();
        let by_name = pages
            .iter()
            .find(|p| p.path == "programs/by-name.mdx")
            .unwrap();
        assert!(
            by_name
                .content
                .contains("import ProgramFilter from '../../../components/ProgramFilter.astro';")
        );
        let versions = pages
            .iter()
            .find(|p| p.path == "configuration/versions.mdx")
            .unwrap();
        assert!(
            versions
                .content
                .contains("import { Tabs, TabItem } from '@astrojs/starlight/components';")
        );
    }

    #[test]
    fn every_page_is_in_the_sidebar_once() {
        let mut listed: Vec<&str> = SIDEBAR
            .iter()
            .flat_map(|(_, slugs)| slugs.iter().copied())
            .collect();
        let mut pages: Vec<&str> = PAGES
            .iter()
            .filter(|p| !matches!(p.starts_at, Start::Drop(_) | Start::Intro))
            .map(|p| p.slug)
            .collect();
        listed.sort_unstable();
        pages.sort_unstable();
        assert_eq!(listed, pages);
    }
}
