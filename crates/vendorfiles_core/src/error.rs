//! Typed errors whose [`Display`](std::fmt::Display) output is the exact message the
//! reference `vendorfiles` CLI prints after its `ERROR: ` prefix.
//!
//! Keeping the wording in the type (rather than formatting at the call site) is what makes
//! the parity tests meaningful: a message can only drift if this file changes.

use std::path::PathBuf;

/// Every failure the library can surface to the user.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VendorError {
    // ---- configuration -------------------------------------------------------------
    #[error("No configuration file found in the current directory.")]
    NoConfigFile,

    #[error("Invalid vendorDependencies key in {0}")]
    InvalidDependenciesKey(String),

    #[error("Invalid vendorConfig key in {0}")]
    InvalidConfigKey(String),

    #[error("config key 'vendorDependencies.{name}.repository' is not a valid github url")]
    InvalidRepositoryKey { name: String },

    #[error("config key 'vendorDependencies.{name}.files' is not a valid array")]
    InvalidFilesKey { name: String },

    #[error("config key 'vendorDependencies.{name}.hashVersionFile' must be a string or true")]
    InvalidHashVersionFileKey { name: String },

    #[error("config key 'vendorDependencies.{name}.releaseRegex' must be a valid regex string")]
    InvalidReleaseRegexKey { name: String },

    #[error("Dependency {name} not found in {path}")]
    DependencyNotFound { name: String, path: String },

    #[error("No dependency found with name {0}")]
    NoDependencyNamed(String),

    #[error("No repository found for dependency {0}")]
    NoRepositoryForDependency(String),

    #[error("No files found for dependency {0}")]
    NoFilesForDependency(String),

    #[error("Dependency {0} is locked and cannot be upgraded")]
    DependencyLocked(String),

    // ---- CLI-level validation ------------------------------------------------------
    #[error("Invalid GitHub URL \"{0}\"")]
    InvalidGitHubUrlQuoted(String),

    #[error("Invalid GitHub URL: {0}")]
    InvalidGitHubUrl(String),

    #[error("you must provide files to install with -f or --files <files...>")]
    MissingFilesOption,

    #[error("No package names provided")]
    NoPackageNames,

    // ---- version resolution --------------------------------------------------------
    #[error("files[0] is invalid for hashVersionFile, must be a string or an object - got {0}")]
    InvalidHashVersionFileTarget(&'static str),

    #[error("hashVersionFile is invalid, must be a string or true")]
    InvalidHashVersionFile,

    #[error("Error while getting commit sha for {path}:\n{source}")]
    CommitShaLookup {
        path: String,
        #[source]
        source: Box<Self>,
    },

    #[error("No commits found for {owner}/{repo}: {path}")]
    NoCommitsFound {
        owner: String,
        repo: String,
        path: String,
    },

    #[error("Could not find a version for {0}")]
    NoVersionFound(String),

    // ---- files & downloads ---------------------------------------------------------
    #[error("File {0}\nis not a string, and {{release}} is not used}}")]
    NonStringOutputWithoutRelease(String),

    #[error("File \"{file}\" was not found in {repository}")]
    FileNotFoundInRepo { file: String, repository: String },

    #[error(
        "{source}:\nCould not download file \"{file}\" from {repository} with version {version}"
    )]
    FileDownloadFailed {
        file: String,
        repository: String,
        version: String,
        #[source]
        source: Box<Self>,
    },

    #[error("Request failed with status {0}")]
    RequestFailed(u16),

    #[error("Could not save {path}:\n{source}")]
    SaveFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },

    // ---- releases ------------------------------------------------------------------
    #[error("Release \"{version}\" was not found in {owner}/{repo}")]
    ReleaseNotFound {
        version: String,
        owner: String,
        repo: String,
    },

    #[error("Release assets were not found in {0}")]
    ReleaseAssetsMissing(String),

    #[error(
        "Release asset \"{asset}\" was not found in {url}\nDid you forget to add a \"v\" before the version?"
    )]
    ReleaseAssetNotFound { asset: String, url: String },

    #[error("Release asset \"{asset}\" failed to download from {url}")]
    ReleaseAssetDownloadFailed { asset: String, url: String },

    #[error("No releases found matching {regex} in {owner}/{repo}")]
    NoMatchingRelease {
        regex: String,
        owner: String,
        repo: String,
    },

    // ---- archives ------------------------------------------------------------------
    #[error(
        "file \"{0}\" cannot be extracted.\nplease check that it's either a zip | tar | tar.gz"
    )]
    CannotExtract(String),

    #[error("Error while moving file \"{from}\" to \"{to}\":\n{source}")]
    MoveFailed {
        from: String,
        to: String,
        #[source]
        source: std::io::Error,
    },

    // ---- search & auth -------------------------------------------------------------
    #[error("No results found for \"{0}\"")]
    NoSearchResults(String),

    #[error("No results found for \"{name}\"\nDid you mean {suggestion}?")]
    NoSearchResultsDidYouMean { name: String, suggestion: String },

    #[error("Invalid token")]
    InvalidToken,

    #[error("Token is rate limited")]
    TokenRateLimited,

    // ---- registry -----------------------------------------------------------------
    #[error("Could not reach the program registry: {0}")]
    RegistryUnreachable(String),

    #[error("The program registry could not be read: {0}")]
    RegistryUnreadable(String),

    #[error(
        "The program registry is version {found}, but this vendor understands {supported}. Update vendor to install by name."
    )]
    RegistryTooNew { found: u32, supported: u32 },

    #[error("The registry entry for '{name}' is unusable: {reason}")]
    RegistryInvalidEntry { name: String, reason: String },

    #[error("'{name}' is in the registry but has no release for {host}")]
    RegistryUnsupportedHost { name: String, host: String },

    #[error("Something went wrong, try again later")]
    AuthUnknownFailure,

    #[error("{0}")]
    DeviceFlow(String),

    // ---- infrastructure ------------------------------------------------------------
    #[error("{source}")]
    Io {
        #[source]
        source: std::io::Error,
    },

    #[error("Could not read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Could not write {path}: {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse {path}: {message}")]
    ParseConfig { path: String, message: String },

    #[error("Failed to serialize {path}: {message}")]
    SerializeConfig { path: String, message: String },

    #[error("{0}")]
    Http(String),
}

impl From<std::io::Error> for VendorError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

impl From<reqwest::Error> for VendorError {
    fn from(source: reqwest::Error) -> Self {
        source.status().map_or_else(
            || Self::Http(source.to_string()),
            |s| Self::RequestFailed(s.as_u16()),
        )
    }
}

impl From<octocrab::Error> for VendorError {
    fn from(source: octocrab::Error) -> Self {
        match &source {
            octocrab::Error::GitHub { source: gh, .. } => {
                Self::RequestFailed(gh.status_code.as_u16())
            }
            _ => Self::Http(source.to_string()),
        }
    }
}

/// Convenience alias used throughout the library.
pub type Result<T, E = VendorError> = std::result::Result<T, E>;
