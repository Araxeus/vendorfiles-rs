//! Probing the GitHub CLI, so `cargo xtask release` knows whether it can cut a release itself.

use std::io::ErrorKind;
use std::process::Command;
use std::thread::JoinHandle;

/// What `gh` can do for us, as far as one `gh auth status` could tell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// Installed and holding credentials: a release can be created from here.
    Ready,
    /// No `gh` on `PATH`.
    Missing,
    /// Installed, but `gh auth status` says nobody is logged in.
    LoggedOut,
}

impl Status {
    /// Why a release cannot be created here, or `None` when it can.
    pub const fn blocker(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::Missing => Some("the GitHub CLI is not installed"),
            Self::LoggedOut => Some("the GitHub CLI is not logged in"),
        }
    }
}

/// Starts the probe on a thread of its own.
///
/// `gh auth status` answers both halves of the question at once - a spawn failure means there is
/// no `gh`, a non-zero exit means there are no credentials - and it costs a process spawn plus,
/// when `gh` reaches the API to check the token, a round trip. Running it alongside the
/// commit-and-tag work keeps that off the critical path: the answer is waiting by the time the
/// release step asks for it.
pub fn probe() -> JoinHandle<Status> {
    std::thread::spawn(|| {
        match Command::new("gh").args(["auth", "status"]).output() {
            Ok(output) if output.status.success() => Status::Ready,
            Ok(_) => Status::LoggedOut,
            Err(error) if error.kind() == ErrorKind::NotFound => Status::Missing,
            // A shim that cannot be executed is no more usable than an absent one, and the
            // release has the same fallback either way.
            Err(_) => Status::Missing,
        }
    })
}
