//! The commands: `sync`, `install`, `uninstall`, and the version logic they share.

pub mod install;
mod sync;
mod uninstall;
mod version;

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::Workspace;
use crate::error::Result;
use crate::github::GitHubClient;

pub use install::InstallOptions;
pub use sync::SyncOptions;

/// A command run: the loaded workspace plus the GitHub client acting on it.
///
/// This is the top of the ownership tree. The workspace is owned outright and only ever
/// touched through `&mut self`, which is what serialises config writes. The client sits behind
/// an [`Arc`] so download work can be handed to independent tasks — the one place the tool
/// needs shared ownership rather than a borrow.
#[derive(Debug)]
pub struct Session {
    pub github: Arc<GitHubClient>,
    pub workspace: Workspace,
}

impl Session {
    /// Pairs a workspace with a client.
    #[must_use]
    pub fn new(github: GitHubClient, workspace: Workspace) -> Self {
        Self {
            github: Arc::new(github),
            workspace,
        }
    }

    /// The lockfile path for a dependency folder.
    #[must_use]
    pub fn lockfile_path(folder: &std::path::Path) -> PathBuf {
        folder.join("vendor-lock.json")
    }
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
