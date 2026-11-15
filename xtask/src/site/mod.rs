//! `cargo xtask site` - generates the docs site's pages from `README.md`.
//!
//! The README stays the one copy of the docs. Each run splits it into the pages of
//! [`pages::PAGES`], turns every format region into synced Starlight tabs, and writes the result
//! under `site/`, where it is gitignored.

mod mdx;
mod sections;

/// The path the site is served under on GitHub Pages.
pub const BASE: &str = "/vendorfiles-rs/";
