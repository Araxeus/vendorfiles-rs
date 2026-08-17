//! `vendorfiles_core` — pull files from GitHub repositories and keep them up to date.
//!
//! This crate holds all behaviour; the `vendor` binary is a thin shell that owns the terminal
//! contract (help text, exit codes, the `ERROR:` prefix). Nothing here exits the process or
//! prints usage, which is what makes the operations testable in-process.
//!
//! ```no_run
//! # async fn run() -> Result<(), vendorfiles_core::VendorError> {
//! use vendorfiles_core::{GitHubClient, Session, SyncOptions, Workspace, auth};
//!
//! let workspace = Workspace::load(None).await?;
//! let github = GitHubClient::new(auth::resolve_token())?;
//! Session::new(github, workspace).sync(SyncOptions::default()).await
//! # }
//! ```

#![forbid(unsafe_code)]
// `octocrab` and `reqwest` legitimately pull in overlapping minor versions of shared crates;
// nothing in this workspace can resolve that. Kept as `allow`, not `expect`: a dependency bump
// that happens to unify them should not fail the build.
#![allow(clippy::multiple_crate_versions)]

pub mod archive;
pub mod config;
pub mod error;
pub mod fsx;
pub mod github;
pub mod lockfile;
pub mod model;
pub mod ops;
pub mod template;
pub mod ui;

pub use config::{ConfigFile, ConfigFormat, Workspace};
pub use error::{Result, VendorError};
pub use github::{GitHubClient, auth};
pub use model::{Dependency, FileEntry, FileTarget, RawDependency, Repository, VendorConfig};
pub use ops::{InstallOptions, Session, SyncOptions};

/// The version reported by `vendor --version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
