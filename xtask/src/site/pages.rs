//! Which README sections make up which site page, and the pages built from them.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use serde_json::json;

use super::mdx::{self, Links, Target};
use super::sections::{self, Section};

/// Where a page's content starts.
pub enum Start {
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
    /// One line for search results and link previews.
    pub description: &'static str,
}

const fn page(
    slug: &'static str,
    title: Option<&'static str>,
    starts_at: Start,
    description: &'static str,
) -> PageSpec {
    assert!(!description.is_empty(), "every page needs a description");
    PageSpec {
        slug,
        title,
        starts_at,
        description,
    }
}

const fn drop(heading: &'static str) -> PageSpec {
    PageSpec {
        slug: "",
        title: None,
        starts_at: Start::Drop(heading),
        description: "",
    }
}

/// Every page, in README order. Each `##` heading of the README must start one.
///
/// The README's intro under its `#` title stays off the site, which has a home page of its own.
pub const PAGES: &[PageSpec] = &[
    drop("Table of Contents"),
    page(
        "getting-started/installation",
        None,
        Start::Heading("Installation"),
        "Install vendor with a one-line script on Linux, macOS or Windows, or with Cargo.",
    ),
    page(
        "getting-started/quick-start",
        None,
        Start::Heading("Quick start"),
        "Write a small config file and pull your first files from GitHub with vendor sync.",
    ),
    page(
        "getting-started/global-programs",
        None,
        Start::Heading("Global programs"),
        "Use vendor to install programs like rg and fd for your whole machine.",
    ),
    page(
        "configuration/config-files",
        Some("Config files"),
        Start::Heading("Configuration"),
        "Where vendor looks for its config file, and the fields each dependency takes.",
    ),
    page(
        "configuration/paths",
        None,
        Start::Heading("Output folders and file names"),
        "Choose where vendored files land, and rename them as they are saved.",
    ),
    page(
        "configuration/folders-and-globs",
        Some("Folders and globs"),
        Start::Heading("Folders and globs"),
        "Take a whole folder from a repository, or every file that matches a glob pattern, and choose where each lands.",
    ),
    page(
        "configuration/versions",
        None,
        Start::Heading("Versions and releases"),
        "Track release tags, pull release assets, extract files from archives, and choose which releases count.",
    ),
    page(
        "configuration/other-sites",
        Some("Other sites"),
        Start::Heading("Downloading from other sites"),
        "Download files from any URL and track their versions with a page, a regex or git tags.",
    ),
    page(
        "configuration/locking",
        Some("Locking"),
        Start::Heading("Locking a dependency"),
        "Keep a dependency at its current version so vendor update leaves it alone.",
    ),
    page(
        "configuration/aliases",
        Some("Aliases"),
        Start::Heading("Aliases"),
        "Give a dependency other names that every command accepts.",
    ),
    page(
        "configuration/bin",
        Some("Commands inside a folder"),
        Start::Heading("Commands inside a folder"),
        "Run a toolchain's commands from the top of its folder, without spreading its tree on PATH.",
    ),
    page(
        "configuration/installers",
        Some("Installers"),
        Start::Heading("Installers"),
        "Run a program's Windows installer silently, and uninstall it again, instead of keeping the file.",
    ),
    page(
        "configuration/defaults",
        Some("Defaults"),
        Start::Heading("Default options"),
        "Set values once for every dependency that doesn't set them itself.",
    ),
    page(
        "reference/commands",
        None,
        Start::Heading("Commands"),
        "Every vendor command, with its short aliases and options.",
    ),
    page(
        "getting-started/program-registry",
        None,
        Start::Heading("Program registry"),
        "Programs vendor knows by name, so you can add them without looking up the repository or its release assets.",
    ),
    page(
        "reference/program-list",
        Some("Program list"),
        Start::Heading("Programs you can add by name"),
        "Every program you can add by name, with its aliases and supported platforms.",
    ),
    page(
        "reference/self-update",
        Some("Self-update"),
        Start::Heading("Keeping vendorfiles updated"),
        "Let vendor update itself along with everything else.",
    ),
    page(
        "reference/authentication",
        None,
        Start::Heading("Authentication"),
        "Log in to GitHub to raise the API rate limit above 60 requests an hour.",
    ),
    page(
        "reference/lockfile",
        None,
        Start::Heading("Lockfile"),
        "What vendor-lock.json records about each download, and how to keep one lockfile for everything.",
    ),
    page(
        "reference/json-schema",
        None,
        Start::Heading("JSON Schema"),
        "Point your config at the JSON Schema so your editor checks and autocompletes it.",
    ),
    page(
        "about/differences",
        None,
        Start::Heading("Differences from the TypeScript version"),
        "How this version compares to the TypeScript vendorfiles, and where its speedup comes from.",
    ),
    page(
        "about/development",
        None,
        Start::Heading("Development"),
        "Build, test and lint vendorfiles, and find your way around the workspace.",
    ),
    drop("License"),
    page(
        "about/adding-to-the-registry",
        None,
        Start::File("docs/add_to_registry.md"),
        "Add a program to the registry so anyone can install it with vendor add <name>.",
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
            "getting-started/program-registry",
        ],
    ),
    (
        "Configuration",
        &[
            "configuration/config-files",
            "configuration/paths",
            "configuration/folders-and-globs",
            "configuration/versions",
            "configuration/other-sites",
            "configuration/locking",
            "configuration/aliases",
            "configuration/bin",
            "configuration/installers",
            "configuration/defaults",
        ],
    ),
    (
        "Reference",
        &[
            "reference/commands",
            "reference/program-list",
            "reference/self-update",
            "reference/authentication",
            "reference/lockfile",
            "reference/json-schema",
        ],
    ),
    (
        "About",
        &[
            "about/differences",
            "about/development",
            "about/adding-to-the-registry",
        ],
    ),
];

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
    build(readme, add_to_registry, PAGES)
}

/// [`generate`] against any page map, so tests can use a small one.
fn build(
    readme: &str,
    add_to_registry: &str,
    pages_map: &'static [PageSpec],
) -> Result<(Vec<Page>, String)> {
    let (_, sections) = sections::split(readme).context("reading README.md")?;
    let owned = assign(sections, pages_map)?;
    let anchors = anchor_map(&owned);

    let mut pages = Vec::new();
    for page in &owned {
        let links = Links {
            anchors: &anchors,
            source_dir: "",
        };
        pages.push(render(page, &links).with_context(|| format!("the `{}` page", page.spec.slug))?);
    }

    for spec in pages_map
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
///
/// A section joins that page only as a child of the page's own heading. One at the same level
/// would read as a sibling of the page's title, so it needs a page of its own.
fn assign(sections: Vec<Section>, pages_map: &'static [PageSpec]) -> Result<Vec<Owned<'static>>> {
    let mut owned: Vec<Owned> = Vec::new();
    let mut dropping = false;

    for (idx, section) in sections.into_iter().enumerate() {
        if idx == 0 && section.level == 1 {
            dropping = true;
            continue;
        }

        let start = pages_map.iter().find(|spec| match spec.starts_at {
            Start::Heading(title) | Start::Drop(title) => title == section.title,
            Start::File(_) => false,
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
            None => {
                let Some(page) = owned.last_mut() else {
                    bail!(
                        "README heading `{}` comes before any page starts",
                        section.title
                    );
                };
                let head = &page.sections[0];
                if section.level <= head.level {
                    bail!(
                        "README heading `{} {}` sits beside `{} {}`, which starts the `{}` page; \
                         nest it one level deeper or add it to PAGES in xtask/src/site/pages.rs",
                        "#".repeat(section.level),
                        section.title,
                        "#".repeat(head.level),
                        head.title,
                        page.spec.slug
                    );
                }
                page.sections.push(section);
            }
        }
    }

    for spec in pages_map {
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
///
/// The page's own heading becomes its title, and the headings under it move up so the
/// shallowest one is an `h2`, however deep the page starts in the README.
fn render(page: &Owned, links: &Links) -> Result<Page> {
    let shallowest = page.sections[1..].iter().map(|section| section.level).min();
    let lift = shallowest.map_or(0, |level| level - 2);

    let mut body: Vec<String> = Vec::new();
    for (idx, section) in page.sections.iter().enumerate() {
        if idx > 0 {
            body.push(format!(
                "{} {}",
                "#".repeat(section.level - lift),
                section.title
            ));
        }
        body.extend(section.body.iter().cloned());
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
    format!(
        "---\ntitle: {}\ndescription: {}\n---\n",
        yaml_string(title),
        yaml_string(page.spec.description)
    )
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

    const TEST_PAGES: &[super::PageSpec] = &[
        super::page("group", None, Start::Heading("Group"), "Group page."),
        super::page("start", None, Start::Heading("Start"), "Start page."),
    ];

    #[test]
    fn children_of_a_deep_page_start_at_h2() {
        let readme = "# T\n\n## Group\n\n### Start\n\ntext\n\n#### Child\n\nbody\n\n##### Grandchild\n\nmore\n";
        let (pages, _) = super::build(readme, "", TEST_PAGES).unwrap();
        let start = pages.iter().find(|p| p.path == "start.mdx").unwrap();
        assert!(
            start.content.contains("\ntitle: Start\n"),
            "{}",
            start.content
        );
        assert!(start.content.contains("\n## Child\n"), "{}", start.content);
        assert!(
            start.content.contains("\n### Grandchild\n"),
            "{}",
            start.content
        );
    }

    #[test]
    fn a_sibling_heading_needs_its_own_page() {
        let readme = "# T\n\n## Group\n\n### Start\n\ntext\n\n### Other\n\nmore\n";
        let error = format!("{:#}", super::build(readme, "", TEST_PAGES).unwrap_err());
        assert!(
            error.contains("`### Other`") && error.contains("xtask/src/site/pages.rs"),
            "{error}"
        );
    }

    #[test]
    fn the_committed_readme_generates_every_page() {
        let root = crate::sh::workspace_root().unwrap();
        let add = std::fs::read_to_string(root.join("docs/add_to_registry.md")).unwrap();
        let (pages, sidebar) = generate(&readme(), &add).unwrap();
        let paths: Vec<_> = pages.iter().map(|p| p.path.as_str()).collect();
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
    fn the_readme_intro_stays_off_the_site() {
        let (pages, _) = generate(&readme(), ADD).unwrap();
        assert!(pages.iter().all(|p| p.path != "index.mdx"));
    }

    #[test]
    fn a_missing_start_heading_is_named() {
        let readme = readme().replace("### Global programs", "### Global tools");
        let error = format!("{:#}", generate(&readme, ADD).unwrap_err());
        assert!(error.contains("`Global programs`"), "{error}");
    }

    #[test]
    fn readme_pages_start_their_children_at_h2() {
        let (pages, _) = generate(&readme(), ADD).unwrap();
        let paths = pages
            .iter()
            .find(|p| p.path == "configuration/paths.mdx")
            .unwrap();
        assert!(
            paths
                .content
                .contains("\ntitle: Output folders and file names\n")
        );
        assert!(
            paths.content.contains("\n## Renaming files\n"),
            "{}",
            paths.content
        );
    }

    #[test]
    fn imports_match_the_page_depth() {
        let (pages, _) = generate(&readme(), ADD).unwrap();
        let by_name = pages
            .iter()
            .find(|p| p.path == "reference/program-list.mdx")
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
            .filter(|p| !matches!(p.starts_at, Start::Drop(_)))
            .map(|p| p.slug)
            .collect();
        listed.sort_unstable();
        pages.sort_unstable();
        assert_eq!(listed, pages);
    }
}
