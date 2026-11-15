//! `cargo xtask site` - generates the docs site's pages from `README.md`.
//!
//! The README stays the one copy of the docs. Each run splits it into the pages of
//! [`pages::PAGES`], turns every format region into synced Starlight tabs, and writes the result
//! under `site/`, where it is gitignored.

mod mdx;
mod pages;
mod sections;

use std::path::Path;

use anyhow::{Context, Result};

use crate::sh;
use pages::Page;

/// The path the site is served under on GitHub Pages.
pub const BASE: &str = "/vendorfiles-rs/";

/// Regenerates the site's pages and sidebar from the README.
pub fn run() -> Result<()> {
    let root = sh::workspace_root()?;
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).with_context(|| format!("reading {path}"))
    };
    let (pages, sidebar) = pages::generate(&read("README.md")?, &read("docs/add_to_registry.md")?)?;
    write_site(&root.join("site"), &pages, &sidebar)?;
    println!("Wrote {} pages to site/src/content/docs.", pages.len());
    Ok(())
}

/// Replaces both generated folders under `site_dir`, so a page dropped from the map is gone.
fn write_site(site_dir: &Path, pages: &[Page], sidebar: &str) -> Result<()> {
    let docs = site_dir.join("src/content/docs");
    let generated = site_dir.join("src/generated");
    for dir in [&docs, &generated] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
        }
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    for page in pages {
        let path = docs.join(&page.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &page.content)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    let path = generated.join("sidebar.json");
    std::fs::write(&path, sidebar).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{pages::Page, write_site};

    #[test]
    fn a_dropped_page_does_not_linger() {
        let dir = std::env::temp_dir().join(format!("xtask-site-{}", std::process::id()));
        let page = |path: &str| Page {
            path: path.to_owned(),
            content: "x".to_owned(),
        };

        write_site(&dir, &[page("a.mdx"), page("old/b.mdx")], "[]").unwrap();
        write_site(&dir, &[page("a.mdx")], "[]").unwrap();

        let docs = dir.join("src/content/docs");
        assert!(docs.join("a.mdx").is_file());
        assert!(!docs.join("old").exists());
        assert!(dir.join("src/generated/sidebar.json").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
