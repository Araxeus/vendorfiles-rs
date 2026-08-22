//! Where the registry is kept between runs, and when to go back for it.
//!
//! `install` is the only command that reads the registry, and most installs happen within a day of
//! each other, so the common case should cost nothing: a cached copy younger than [`TTL`] is used
//! without touching the network at all. Past that, the fetch is conditional - the file carries an
//! `ETag`, so an unchanged registry costs one 304 rather than a download.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// How long a cached registry is used without asking whether it changed.
pub const TTL: Duration = Duration::from_hours(24);

/// The registry `install` reads when nothing overrides it.
pub const DEFAULT_URL: &str =
    "https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/registry.yml";

/// The variable that points `vendor` at a different registry: a URL, or a local path.
///
/// A contributor can check an entry before opening the pull request, and the tests read a fixture
/// instead of the network.
pub const OVERRIDE: &str = "VENDOR_REGISTRY";

/// What to do about the copy on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Young enough; no request at all.
    UseCache,
    /// Ask whether it changed, quoting this `ETag` if there is one.
    Revalidate(Option<String>),
    /// Nothing usable on disk.
    Download,
}

/// Decides what to do, given the age of the cached copy and whether `--refresh` was passed.
///
/// Split out from the request so the policy is testable without a network or a clock.
#[must_use]
pub fn decide(age: Option<Duration>, etag: Option<String>, refresh: bool) -> Action {
    match age {
        None => Action::Download,
        Some(_) if refresh => Action::Revalidate(etag),
        Some(age) if age < TTL => Action::UseCache,
        Some(_) => Action::Revalidate(etag),
    }
}

/// Where the cached registry and its `ETag` live.
///
/// Read from the platform's own variables rather than adding a dependency for three lookups.
#[must_use]
pub fn directory() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }?;
    Some(base.join("vendorfiles"))
}

/// How long ago `path` was last written, if it is there.
#[must_use]
pub fn age(path: &std::path::Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    // A file stamped in the future is treated as brand new rather than as an error.
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::{Action, TTL, decide, directory};
    use std::time::Duration;

    #[test]
    fn nothing_on_disk_means_download() {
        assert_eq!(decide(None, None, false), Action::Download);
        assert_eq!(decide(None, None, true), Action::Download);
    }

    #[test]
    fn a_young_copy_costs_no_request() {
        assert_eq!(
            decide(Some(Duration::from_mins(1)), None, false),
            Action::UseCache
        );
    }

    #[test]
    fn an_old_copy_is_revalidated_rather_than_refetched() {
        let etag = Some("\"abc123\"".to_owned());
        assert_eq!(
            decide(Some(TTL + Duration::from_secs(1)), etag.clone(), false),
            Action::Revalidate(etag)
        );
    }

    #[test]
    fn refresh_revalidates_however_young_the_copy_is() {
        assert_eq!(
            decide(Some(Duration::from_secs(1)), None, true),
            Action::Revalidate(None)
        );
    }

    #[test]
    fn the_cache_lands_under_the_platform_s_own_directory() {
        // Whichever platform the test runs on, the path ends in our own folder and is absolute.
        let Some(directory) = directory() else {
            return; // A stripped environment with none of the variables set.
        };
        assert!(directory.is_absolute(), "{}", directory.display());
        assert_eq!(directory.file_name().unwrap(), "vendorfiles");
    }
}
