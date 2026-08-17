//! Repository automation. Run with `cargo xtask <command>`.

#![allow(clippy::multiple_crate_versions)]

mod release;

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
    /// Bump the workspace version, commit, and tag `v{version}`.
    Release,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Release => release::run(),
    }
}
