//! `cargo xtask ci` — every gate `.github/workflows/check.yml` applies, in one command.
//!
//! The lint groups are spelled out here even though `[workspace.lints]` already sets them, so
//! this stays the same gate as the workflow whichever of the two is read.

use anyhow::Result;

use crate::sh;

/// The workflow's clippy invocation, argument for argument.
const CLIPPY: &[&str] = &[
    "clippy",
    "--all-targets",
    "--all-features",
    "--",
    "-W",
    "clippy::pedantic",
    "-W",
    "clippy::cargo",
    "-W",
    "clippy::nursery",
    "-D",
    "warnings",
];

/// Runs the README check, cargo check, format, clippy and the tests, stopping at the first
/// failure.
///
/// Ordered cheapest first, so a stale README, a compile error or a stray space is reported in
/// seconds rather than after the whole suite has run.
pub fn run() -> Result<()> {
    crate::readme::run(true)?;
    let root = sh::workspace_root()?;
    sh::run(
        &root,
        "Checking",
        "cargo",
        &["check", "--workspace", "--all-targets"],
    )?;
    sh::run(&root, "Formatting", "cargo", &["fmt", "--all", "--check"])?;
    sh::run(&root, "Linting", "cargo", CLIPPY)?;
    sh::run(&root, "Testing", "cargo", &["test", "--workspace"])?;
    println!("\nAll checks passed.");
    Ok(())
}
