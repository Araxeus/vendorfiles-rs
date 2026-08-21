//! Repository automation. Run with `cargo xtask <command>`.

#![allow(clippy::multiple_crate_versions)]

mod ci;
mod release;
mod sh;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "Repository automation for vendorfiles-rs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run every check CI runs: cargo check, rustfmt, clippy and the tests.
    Ci,
    /// Bump the workspace version, commit, and tag `v{version}`.
    Release,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Ci => ci::run(),
        Command::Release => release::run(),
    }
}
