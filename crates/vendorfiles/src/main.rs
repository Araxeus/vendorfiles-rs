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
use std::time::Duration;

use clap::Parser;

use crate::help::Intercept;

/// Exit code used for every failure, matching the reference CLI.
const FAILURE: u8 = 1;

/// Exit code for a run the user interrupted: the shell convention of 128 plus SIGINT.
const INTERRUPTED: u8 = 130;

/// How long an interrupted run may spend tidying up before it ends regardless.
///
/// Restoration takes about a tick, so this only ever fires when the terminal has stopped
/// accepting output — and that is exactly when it matters: listening for the signal took the
/// operating system's own handling away, so a wait that never finishes would leave Ctrl-C doing
/// nothing at all for the rest of the run. Long enough not to cut a healthy teardown short.
const GRACE: Duration = Duration::from_millis(500);

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
        // Listening for the signal replaced the operating system's own handling for the rest of
        // the process, which makes this the only thing left that can stop the run: it has to
        // stop it whatever the terminal does. So the tidy-up is given a thread of its own and
        // three ways to be over — it finishes, the user says again that they are done with it,
        // or it runs out of time — and none of them is a wait on the terminal answering.
        let restoring = tokio::task::spawn_blocking(vendorfiles_core::progress::restore_terminal);
        tokio::select! {
            _ = restoring => {}
            // Nothing to do on a second press: restoration leads with the cursor, so all that
            // is being given up here is the wait for the region to come down.
            _ = tokio::signal::ctrl_c() => {}
            () = tokio::time::sleep(GRACE) => {}
        }
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
