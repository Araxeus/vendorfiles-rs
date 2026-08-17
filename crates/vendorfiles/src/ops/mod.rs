//! The commands: `sync`, `install`, `uninstall`, and the version logic they share.

mod install;
mod sync;
mod uninstall;
mod version;

use std::path::PathBuf;

use crate::config::Workspace;
use crate::error::Result;
use crate::github::GitHubClient;

pub use install::InstallOptions;
pub use sync::SyncOptions;

/// A command run: the loaded workspace plus the GitHub client acting on it.
///
/// This is the top of the ownership tree — everything below borrows from it.
#[derive(Debug)]
pub struct Session {
    pub github: GitHubClient,
    pub workspace: Workspace,
}

impl Session {
    /// Pairs a workspace with a client.
    #[must_use]
    pub const fn new(github: GitHubClient, workspace: Workspace) -> Self {
        Self { github, workspace }
    }

    /// The lockfile path for a dependency folder.
    #[must_use]
    pub fn lockfile_path(folder: &std::path::Path) -> PathBuf {
        folder.join("vendor-lock.json")
    }
}

/// Formats an optional version the way JavaScript interpolates `undefined`.
#[must_use]
pub fn display_version(version: Option<&str>) -> &str {
    version.unwrap_or("undefined")
}

/// Result of resolving what a dependency's version *should* be.
#[derive(Debug, Clone)]
pub struct VersionDecision {
    /// The version to install; empty when no release could be resolved.
    pub version: String,
    /// Whether anything needs to be written.
    pub needs_update: bool,
}

pub(crate) type OpResult = Result<Option<String>>;
