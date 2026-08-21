//! `vendor` — the command-line entry point.
//!
//! This binary owns the terminal contract: Commander-identical help text, Commander-identical
//! parse errors, and exit code 1 for every failure (`clap` would use 2).

#![forbid(unsafe_code)]
#![allow(clippy::multiple_crate_versions)]

mod cli;
mod help;
mod known;
mod run;
mod spec;

use std::process::ExitCode;

use clap::Parser;

use crate::help::Intercept;

/// Exit code used for every failure, matching the reference CLI.
const FAILURE: u8 = 1;

/// Exit code for a run the user interrupted: the shell convention of 128 plus SIGINT.
const INTERRUPTED: u8 = 130;

/// Gives the terminal back when the user interrupts a run.
///
/// A signal runs no destructor, so without this an interrupted `sync` leaves the cursor hidden
/// for the rest of the session — the display hides it on every frame. Listening rather than
/// masking: the process still stops, it just stops tidily.
fn restore_terminal_on_interrupt() {
    tokio::spawn(async {
        if tokio::signal::ctrl_c().await.is_err() {
            return; // No handler to be had; the default behaviour still stops the process.
        }
        vendorfiles_core::progress::restore_terminal();
        // A second press falls through to the operating system's own handling, so an impatient
        // user is never stuck waiting on the tidy-up.
        std::process::exit(i32::from(INTERRUPTED));
    });
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match help::intercept(&args) {
        Intercept::Print(text) => {
            print!("{text}");
            return ExitCode::SUCCESS;
        }
        Intercept::Version => {
            println!("{}", vendorfiles_core::VERSION);
            return ExitCode::SUCCESS;
        }
        Intercept::UsageError => {
            eprint!("{}", spec::ROOT_HELP);
            return ExitCode::from(FAILURE);
        }
        Intercept::Parse => {}
    }

    let parsed = match cli::Cli::try_parse_from(
        std::iter::once("vendor".to_owned()).chain(args.iter().cloned()),
    ) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("error: {}", cli::commander_message(&error, &args));
            return ExitCode::from(FAILURE);
        }
    };

    restore_terminal_on_interrupt();

    match run::dispatch(parsed).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            vendorfiles_core::ui::error(format!("{error}"));
            ExitCode::from(FAILURE)
        }
    }
}
