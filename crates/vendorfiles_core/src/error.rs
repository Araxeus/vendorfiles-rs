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

    #[error("{0} describes one dependency, so it cannot be used with more than one source")]
    SingleSourceOption(&'static str),

    #[error("'{version}' looks like a version, not a source. Did you mean '{suggestion}'?")]
    VersionAsSource { version: String, suggestion: String },

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
        "{source}:\nCould not download file \"{file}\" from {repository}{}",
        at_version(version)
    )]
    FileDownloadFailed {
        file: String,
        repository: String,
        version: String,
        #[source]
        source: Box<Self>,
    },

    /// A repository that GitHub says is not there.
    ///
    /// Distinct from a missing file, which answers identically - `404`, "Not Found" - and which
    /// is what a mistyped `repository` used to be reported as: a complaint about a file, at a
    /// version that had resolved to nothing because the release lookup 404ed for the same
    /// reason. Says "or" about access because a private repository and a nonexistent one are
    /// deliberately indistinguishable from outside - and phrased without mentioning a token,
    /// since a run with no token at all reaches this too.
    #[error("Repository {repository} does not exist, or you do not have access to it")]
    RepositoryNotFound { repository: String },

    /// A request GitHub refused, carrying whatever it said about why.
    ///
    /// The status on its own is often nothing to act on. A `403` is a SAML-gated organization, a
    /// fine-grained token missing one permission, or a secondary rate limit - three different
    /// things to go and do, and the number tells them apart not at all. GitHub's own body does
    /// ("Resource protected by organization SAML enforcement..."), so it is kept and shown.
    /// `message` is empty only when there was nothing readable to keep, which is when this reads
    /// exactly as it always did.
    #[error("Request failed with status {status}{}", status_detail(message))]
    RequestFailed { status: u16, message: String },

    /// GitHub refused the credentials on an ordinary API request.
    ///
    /// Distinct from [`InvalidToken`](Self::InvalidToken), which answers "is this token you just
    /// gave me any good?" during `vendor login`. Here nobody was asked for a token, so the
    /// message has to say where one comes from.
    #[error(
        "GitHub rejected the credentials (401 Bad credentials).\n\
         Check your GITHUB_TOKEN, or run `vendor login`"
    )]
    BadCredentials,

    // The crate reads its own responses and adds the `x-ratelimit-*` headers to them.
    // Octocrab deserializes the body and ignores the headers, so it does not know the limit from the status and message.
    #[error(
        "GitHub API rate limit reached{0}\nRun `vendor login` or use a GITHUB_TOKEN env \
         variable - unauthenticated requests are limited to 60 an hour, a token to 5000"
    )]
    RateLimited(String),

    #[error("Could not save {path}:\n{source}{}", in_use_hint(source))]
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
    /// An archive that could not be unpacked, and why.
    ///
    /// The first two lines are the reference's, to the letter, because they are what a user has
    /// seen for years and what the fixtures check. The cause goes underneath rather than
    /// replacing them: "check that it's either a zip | tar | tar.gz" is advice for one of the
    /// things that lands here and a waste of the reader's time for the rest - a full disk, a
    /// file held open, an asset that never finished downloading.
    #[error(
        "file \"{file}\" cannot be extracted.\nplease check that it's either a zip | tar | tar.gz\n{source}"
    )]
    CannotExtract {
        file: String,
        #[source]
        source: Box<Self>,
    },

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

    #[error("Could not remove the stored token: {0}")]
    KeyringDelete(String),

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

    /// A file that would not go away.
    ///
    /// Separate from [`ReadFile`](Self::ReadFile), which is what a failed delete used to be
    /// reported as: "Could not read" sends the reader to look at permissions on a file they were
    /// trying to remove. On Windows the common cause is an executable that is currently running,
    /// which refuses deletion with `os error 5`.
    #[error("Could not delete {path}: {source}{}", in_use_hint(source))]
    DeleteFailed {
        path: PathBuf,
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
            // A `reqwest::Error` is raised after the body is gone, so there is no message to
            // be had on this route - `GitHubClient::send` is the one that reads its own.
            |s| Self::RequestFailed {
                status: s.as_u16(),
                message: String::new(),
            },
        )
    }
}

impl From<octocrab::Error> for VendorError {
    fn from(source: octocrab::Error) -> Self {
        match &source {
            octocrab::Error::GitHub { source: gh, .. } => {
                let status = gh.status_code.as_u16();
                if is_rate_limited(status, &gh.message) {
                    return Self::RateLimited(String::new());
                }
                // No headers on this route to say how much was left, but the status is enough
                // to say whose fault it is.
                if status == 401 {
                    return Self::BadCredentials;
                }
                Self::RequestFailed {
                    status,
                    message: one_line(&gh.message),
                }
            }
            _ => Self::Http(source.to_string()),
        }
    }
}

impl VendorError {
    /// The first line of this error's message.
    ///
    /// Several of these messages run to two lines - a wrapped source, an instruction on its own
    /// line - and the live display has one row per dependency to say what went wrong in. The
    /// whole message still reaches the user as the command's `ERROR:` line.
    ///
    /// A trailing colon goes with it. The variants that wrap a source render as `{source}:` and
    /// then the context on the next line, so keeping the first line verbatim would end a row on
    /// punctuation pointing at a line that is not there.
    #[must_use]
    pub fn brief(&self) -> String {
        let rendered = self.to_string();
        rendered
            .lines()
            .next()
            .unwrap_or_default()
            .trim_end()
            .trim_end_matches(':')
            .trim_end()
            .to_owned()
    }
}

/// A hint for the sharing violations a tool that vendors executables runs into.
///
/// `os error 5` on a delete says only "Access is denied", which is equally true of a file whose
/// permissions are wrong and of one that is merely running - and those want opposite things done
/// about them. `os error 32` says more, but still not what to do. Vendoring executables onto
/// `PATH` is most of what this tool is for, so meeting one that is running is ordinary rather
/// than exotic.
///
/// Windows only: 5 and 32 mean other things on Unix (`EIO`, `EPIPE`), where a running binary
/// does not lock its own image in the first place.
fn in_use_hint(source: &std::io::Error) -> &'static str {
    if cfg!(windows) && matches!(source.raw_os_error(), Some(5 | 32)) {
        "\nThe file may be in use - close whatever is running it and try again."
    } else {
        ""
    }
}

/// `" with version {version}"`, or nothing when the version resolved to nothing.
///
/// A repository with no usable release leaves the version empty, and the clause was printed
/// anyway - `with version ` with nothing after it, which reads like the sentence was cut off.
fn at_version(version: &str) -> String {
    if version.is_empty() {
        String::new()
    } else {
        format!(" with version {version}")
    }
}

/// `": {message}"`, or nothing at all when GitHub sent no message.
///
/// Separate from the `#[error]` attribute so that a refusal with nothing readable in it renders
/// exactly the sentence it always did, rather than a status code with a dangling colon.
fn status_detail(message: &str) -> String {
    if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    }
}

/// Trims a message GitHub sent to one short line, fit to sit after a status code.
///
/// Remote text, so it is bounded on both axes: the first line only, and capped, because this ends
/// up on a terminal row and an error body is not obliged to be either short or single-line.
#[must_use]
pub fn one_line(message: &str) -> String {
    const LIMIT: usize = 200;
    let trimmed = message.lines().next().unwrap_or_default().trim();
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_owned();
    }
    let kept: String = trimmed.chars().take(LIMIT - 1).collect();
    format!("{kept}\u{2026}")
}

/// GitHub answers an exhausted limit with `403` - or `429` for the secondary limits - and says so
/// in the body.
///
/// The status alone is not enough to go on: a bad token is also a `403`, and telling someone to
/// run `vendor login` when they already did would send them the wrong way.
#[must_use]
pub fn is_rate_limited(status: u16, message: &str) -> bool {
    matches!(status, 403 | 429) && message.to_ascii_lowercase().contains("rate limit")
}

/// Convenience alias used throughout the library.
pub type Result<T, E = VendorError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::{VendorError, is_rate_limited, one_line};

    #[test]
    fn a_refusal_is_only_a_rate_limit_when_github_says_so() {
        assert!(is_rate_limited(
            403,
            "API rate limit exceeded for 1.2.3.4. (But here's the good news: ...)"
        ));
        // The secondary limits answer 429.
        assert!(is_rate_limited(
            429,
            "You have exceeded a secondary rate limit"
        ));
        // A bad or expired token is a 403 too, and must not be blamed on quota - the advice
        // would be to log in, which is the one thing that would not help.
        assert!(!is_rate_limited(403, "Bad credentials"));
        assert!(!is_rate_limited(
            403,
            "Resource not accessible by integration"
        ));
        // A limit message on some other status is not a refusal for quota.
        assert!(!is_rate_limited(404, "API rate limit exceeded"));
    }

    #[test]
    fn brief_is_the_first_line_of_a_message_that_has_several() {
        // The 401 message puts its instruction on a second line; a row has space for one.
        let full = VendorError::BadCredentials.to_string();
        assert!(full.contains('\n'), "{full}");
        assert_eq!(
            VendorError::BadCredentials.brief(),
            "GitHub rejected the credentials (401 Bad credentials)."
        );
        // A wrapped source ends its first line with the colon that joins it to the context
        // below, and a row has nothing below it to point at.
        let wrapped = VendorError::FileDownloadFailed {
            file: "missing.md".to_owned(),
            repository: "https://github.com/o/r".to_owned(),
            version: "v1.0.0".to_owned(),
            source: Box::new(VendorError::RequestFailed {
                status: 404,
                message: "Not Found".to_owned(),
            }),
        };
        assert!(
            wrapped.to_string().contains(
                "Not Found:
"
            ),
            "{wrapped}"
        );
        assert_eq!(wrapped.brief(), "Request failed with status 404: Not Found");

        // A single-line message is its own brief.
        assert_eq!(
            VendorError::RequestFailed {
                status: 500,
                message: String::new(),
            }
            .brief(),
            "Request failed with status 500"
        );
    }

    #[test]
    fn a_refusal_says_what_github_said_about_it_when_it_said_anything() {
        // The case this was written for: a 403 whose number could mean three different things,
        // and whose body says which.
        assert_eq!(
            VendorError::RequestFailed {
                status: 403,
                message: "Resource protected by organization SAML enforcement".to_owned(),
            }
            .to_string(),
            "Request failed with status 403: Resource protected by organization SAML enforcement"
        );
        // Nothing readable in the body leaves the old sentence untouched - no trailing colon.
        assert_eq!(
            VendorError::RequestFailed {
                status: 500,
                message: String::new(),
            }
            .to_string(),
            "Request failed with status 500"
        );
    }

    #[test]
    #[cfg_attr(not(windows), ignore = "the hint is for Windows sharing violations")]
    fn a_file_that_will_not_budge_says_it_may_be_in_use() {
        let denied = VendorError::DeleteFailed {
            path: std::path::PathBuf::from("tool.exe"),
            source: std::io::Error::from_raw_os_error(5),
        };
        let rendered = denied.to_string();
        assert!(
            rendered.starts_with("Could not delete tool.exe: "),
            "{rendered}"
        );
        assert!(
            rendered.ends_with("close whatever is running it and try again."),
            "{rendered}"
        );
        // The row still carries the failure, not the advice.
        assert_eq!(
            denied.brief(),
            "Could not delete tool.exe: Access is denied. (os error 5)"
        );

        // An unrelated failure gets no hint invented for it.
        let other = VendorError::DeleteFailed {
            path: std::path::PathBuf::from("tool.exe"),
            source: std::io::Error::from(std::io::ErrorKind::InvalidData),
        };
        assert!(!other.to_string().contains("may be in use"), "{other}");
    }

    #[test]
    fn a_download_failure_names_a_version_only_when_there_is_one() {
        let failed = |version: &str| {
            VendorError::FileDownloadFailed {
                file: "README.md".to_owned(),
                repository: "https://github.com/o/r".to_owned(),
                version: version.to_owned(),
                source: Box::new(VendorError::RequestFailed {
                    status: 404,
                    message: "Not Found".to_owned(),
                }),
            }
            .to_string()
        };
        assert!(failed("v1.0.0").ends_with("from https://github.com/o/r with version v1.0.0"));
        // A repository with no usable release leaves the version empty, and the clause used to
        // be printed anyway - ending the sentence on "with version " and nothing else.
        assert!(
            failed("").ends_with("from https://github.com/o/r"),
            "{}",
            failed("")
        );
    }

    #[test]
    fn a_message_from_github_is_bounded_before_it_reaches_a_row() {
        assert_eq!(
            one_line("  Resource not accessible  "),
            "Resource not accessible"
        );
        // An error page, or anything else multi-line, contributes its first line and no more.
        assert_eq!(one_line("Not Found\nthen a stack trace"), "Not Found");
        assert_eq!(one_line(""), "");

        let long = one_line(&"x".repeat(500));
        assert_eq!(long.chars().count(), 200);
        assert!(long.ends_with('\u{2026}'), "{long}");
    }

    #[test]
    fn the_rate_limit_message_carries_its_specifics_and_always_its_advice() {
        let bare = VendorError::RateLimited(String::new()).to_string();
        assert!(
            bare.starts_with("GitHub API rate limit reached\n"),
            "{bare}"
        );
        assert!(bare.contains("vendor login"));
        assert!(bare.contains("60 an hour"));

        let detailed =
            VendorError::RateLimited(" - 0 of 60 left, resets in 12 min".to_owned()).to_string();
        assert!(
            detailed
                .starts_with("GitHub API rate limit reached - 0 of 60 left, resets in 12 min\n"),
            "{detailed}"
        );
    }
}
