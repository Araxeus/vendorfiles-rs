//! Repository automation. Run with `cargo xtask <command>`.

#![allow(clippy::multiple_crate_versions)]

mod ci;
mod gh;
mod readme;
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
    /// Regenerate the YAML and TOML tabs of the README's config examples.
    Readme {
        /// Fail instead of writing when the README is out of date.
        #[arg(long)]
        check: bool,
    },
    /// Bump the workspace version, commit and tag `v{version}`, then push, publish and open a
    /// draft release - each one asked about, or taken as read from its flag.
    Release(release::Options),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Ci => ci::run(),
        Command::Readme { check } => readme::run(check),
        Command::Release(options) => release::run(&options),
    }
}
