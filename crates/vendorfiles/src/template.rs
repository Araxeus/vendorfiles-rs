//! String templating and URL helpers.
//!
//! Every substitution here replaces the **first** occurrence only, because the reference
//! implementation uses JavaScript's `String.prototype.replace` with a string pattern.

// `{version}`, `{release}/` and `{vendorFolder}` are config placeholders, not format arguments.
#![allow(clippy::literal_string_with_formatting_args)]

use std::sync::LazyLock;

use regex::Regex;

use crate::error::{Result, VendorError};
use crate::model::Repository;

static GITHUB_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^https?://(?:www\.)?github\.com/[^/]+/[^/]+$").expect("valid regex")
});

static REPO_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"github\.com/([^/]+)/([^/]+)").expect("valid regex"));

static SEMVER_CORE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+").expect("valid regex"));

static OWNER_REPO_SHORTHAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^/]+/[^/]+$").expect("valid regex"));

/// Whether `url` is a bare `https://github.com/owner/repo` URL.
#[must_use]
pub fn is_github_url(url: &str) -> bool {
    GITHUB_URL.is_match(url)
}

/// Whether `source` is in `owner/repo` shorthand.
#[must_use]
pub fn is_owner_repo_shorthand(source: &str) -> bool {
    OWNER_REPO_SHORTHAND.is_match(source)
}

/// Extracts the owner and repository name from any URL containing `github.com/owner/repo`.
///
/// # Errors
///
/// Returns [`VendorError::InvalidGitHubUrl`] when the URL contains no `github.com/owner/repo`.
pub fn owner_and_name_from_repo_url(url: &str) -> Result<Repository> {
    let caps = REPO_PATH
        .captures(url)
        .ok_or_else(|| VendorError::InvalidGitHubUrl(url.to_owned()))?;
    Ok(Repository {
        owner: caps[1].to_owned(),
        name: caps[2].to_owned(),
    })
}

/// Strips every leading occurrence of `prefix`.
#[must_use]
pub fn trim_start_matches_str<'a>(mut s: &'a str, prefix: &str) -> &'a str {
    if prefix.is_empty() {
        return s;
    }
    while let Some(rest) = s.strip_prefix(prefix) {
        s = rest;
    }
    s
}

/// Strips every trailing occurrence of `suffix`.
#[must_use]
pub fn trim_end_matches_str<'a>(mut s: &'a str, suffix: &str) -> &'a str {
    if suffix.is_empty() {
        return s;
    }
    while let Some(rest) = s.strip_suffix(suffix) {
        s = rest;
    }
    s
}

/// The value substituted for `{version}`: the first `x.y.z` found in the tag, or the tag with
/// leading `v`s stripped.
#[must_use]
pub fn version_token(version: &str) -> &str {
    SEMVER_CORE
        .find(version)
        .map_or_else(|| trim_start_matches_str(version, "v"), |m| m.as_str())
}

/// Replaces the first `{version}` placeholder in `path`.
#[must_use]
pub fn replace_version(path: &str, version: &str) -> String {
    path.replacen("{version}", version_token(version), 1)
}

/// Replaces the first `{vendorFolder}` placeholder in `path`.
#[must_use]
pub fn replace_vendor_folder(path: &str, vendor_folder: &str) -> String {
    path.replacen("{vendorFolder}", vendor_folder, 1)
}

/// Strips the leading `{release}/` marker, if present.
#[must_use]
pub fn strip_release_prefix(path: &str) -> String {
    path.replacen("{release}/", "", 1)
}

/// Whether `path` refers to a GitHub release asset rather than a repository file.
#[must_use]
pub fn is_release_path(path: &str) -> bool {
    path.starts_with("{release}/")
}

/// The final component of a path, with the platform's separator rules — matching Node's
/// `path.basename`, which the reference uses to derive default output names.
#[must_use]
pub fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        basename, is_github_url, is_owner_repo_shorthand, owner_and_name_from_repo_url,
        replace_version, strip_release_prefix, trim_end_matches_str, trim_start_matches_str,
        version_token,
    };

    #[test]
    fn github_url_matching_mirrors_the_reference_regex() {
        assert!(is_github_url("https://github.com/a/b"));
        assert!(is_github_url("http://www.github.com/a/b"));
        assert!(!is_github_url("https://github.com/a/b/c"));
        assert!(!is_github_url("https://gitlab.com/a/b"));
        assert!(!is_github_url("github.com/a/b"));
    }

    #[test]
    fn owner_and_name_are_extracted_from_any_github_url() {
        let repo = owner_and_name_from_repo_url("https://www.github.com/junegunn/fzf").unwrap();
        assert_eq!(repo.owner, "junegunn");
        assert_eq!(repo.name, "fzf");
        assert_eq!(
            owner_and_name_from_repo_url("nope")
                .unwrap_err()
                .to_string(),
            "Invalid GitHub URL: nope"
        );
    }

    #[test]
    fn version_token_prefers_the_semver_core() {
        assert_eq!(version_token("v1.2.3"), "1.2.3");
        assert_eq!(version_token("1.2.3-alpha.1"), "1.2.3");
        assert_eq!(version_token("release-2.0.0-rc"), "2.0.0");
        assert_eq!(version_token("vvnightly"), "nightly");
        assert_eq!(version_token(""), "");
    }

    #[test]
    fn only_the_first_placeholder_is_replaced() {
        assert_eq!(
            replace_version("a-{version}/b-{version}.zip", "v1.2.3"),
            "a-1.2.3/b-{version}.zip"
        );
        assert_eq!(
            strip_release_prefix("{release}/x-{release}/y"),
            "x-{release}/y"
        );
    }

    #[test]
    fn trims_repeat_until_exhausted() {
        assert_eq!(trim_start_matches_str("vvv1.0", "v"), "1.0");
        assert_eq!(trim_end_matches_str("a...", "."), "a");
        assert_eq!(trim_start_matches_str("abc", ""), "abc");
    }

    #[test]
    fn basename_matches_node() {
        assert_eq!(basename("dist/coloris.min.js"), "coloris.min.js");
        assert_eq!(basename("LICENSE"), "LICENSE");
        assert_eq!(basename(""), "");
    }

    #[test]
    fn owner_repo_shorthand_is_recognised() {
        assert!(is_owner_repo_shorthand("Araxeus/vendorfiles"));
        assert!(!is_owner_repo_shorthand("vendorfiles"));
        assert!(!is_owner_repo_shorthand("a/b/c"));
    }
}
