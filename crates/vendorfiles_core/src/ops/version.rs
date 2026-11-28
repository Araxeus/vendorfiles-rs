//! Deciding which version a dependency should be at, and whether anything must change.

use std::path::Path;

use crate::error::{Result, VendorError};
use crate::fsx::anchor;
use crate::installer;
use crate::lockfile::{expanded_lock, flat_files, locked_matches, read_lockfile, saved_folder};
use crate::model::{Dependency, Repository, Source};
use crate::ops::Session;
use crate::url_source::latest_version;

/// Whether a failed release lookup means the repository simply has no release.
///
/// A `404` does, and so does a `releaseRegex` that matched nothing - both are answers rather
/// than failures. Everything else is the absence of an answer, whether GitHub sent the refusal
/// itself (`401`, an exhausted quota, a `503`) or the request never arrived at all (a dropped
/// connection), and none of it says anything about which releases exist.
const fn means_no_release(error: &VendorError) -> bool {
    matches!(
        error,
        VendorError::RequestFailed { status: 404, .. } | VendorError::NoMatchingRelease { .. }
    )
}

impl Session {
    /// Resolves the version to install.
    ///
    /// With `hashVersionFile` this is the tracked file's latest commit SHA; otherwise it is
    /// the latest release tag. A repository with no usable release yields an empty string -
    /// except under `outdated`, where the reference treats it as fatal.
    ///
    /// A deliberate divergence: the reference catches *every* failure of the release lookup
    /// here, so a refused token yields the same empty version as a repository that genuinely
    /// has no release, and the run goes on to blame a version. Only a real answer - see
    /// [`means_no_release`] - counts as one; everything else is returned as the failure it is.
    pub(crate) async fn resolve_new_version(
        &self,
        dependency: &Dependency,
        show_outdated_only: bool,
    ) -> Result<String> {
        let repo = match &dependency.source {
            Source::GitHub(repo) => repo,
            // Without a `versionUrl` the dependency is unversioned: the configured version
            // stands, and an empty one means "install once".
            Source::Url(source) => {
                let latest = latest_version(self.github.http(), source).await?;
                return Ok(latest
                    .or_else(|| dependency.version.clone())
                    .unwrap_or_default());
            }
        };
        if let Some(path) = dependency.hash_version_target()? {
            let error = match self.github.file_commit_sha(repo, &path).await {
                Ok(sha) => return Ok(sha),
                Err(error) => {
                    self.unless_repository_missing(dependency, repo, error)
                        .await
                }
            };
            return Err(match error {
                missing @ VendorError::RepositoryNotFound { .. } => missing,
                source => VendorError::CommitShaLookup {
                    path,
                    source: Box::new(source),
                },
            });
        }

        match self
            .github
            .latest_release(repo, dependency.release_regex.as_deref())
            .await
        {
            Ok(release) => Ok(release.tag_name.clone()),
            // A repository with no release to find is the case the two arms below are for. A
            // request that *failed* is not that, and reporting it as "could not find a version"
            // buries the reason - a refused token, an exhausted quota - under a sentence about
            // versions. Those are reported as themselves.
            Err(error) if !means_no_release(&error) => Err(error),
            Err(error) if show_outdated_only => Err(
                match self
                    .unless_repository_missing(dependency, repo, error)
                    .await
                {
                    missing @ VendorError::RepositoryNotFound { .. } => missing,
                    _ => VendorError::NoVersionFound(dependency.name.clone()),
                },
            ),
            // An install reports a missing repository once its downloads fail, so a repository
            // without releases costs no extra request here.
            Err(_) => Ok(String::new()),
        }
    }

    /// Turns a `404` into [`VendorError::RepositoryNotFound`] when the repository itself is
    /// what GitHub cannot find.
    ///
    /// A private repository answers `404` to a token that cannot read it, so a missing release
    /// or commit is often a missing repository. Telling them apart costs one request, and this
    /// runs only after a lookup has already failed.
    async fn unless_repository_missing(
        &self,
        dependency: &Dependency,
        repo: &Repository,
        error: VendorError,
    ) -> VendorError {
        if !matches!(error, VendorError::RequestFailed { status: 404, .. }) {
            return error;
        }
        if self.github.repository_exists(repo).await == Some(false) {
            return VendorError::RepositoryNotFound {
                repository: dependency.repository.clone(),
            };
        }
        error
    }

    /// Whether the dependency folder is out of step with what is being installed.
    ///
    /// Any inconsistency - missing lockfile, an unfinished install, version drift, a file that
    /// vanished, a changed `files` declaration - means "yes". Every failure path is also "yes",
    /// matching the reference's blanket `catch`.
    pub(crate) async fn needs_update(
        &self,
        dependency: &Dependency,
        lockfile_path: &Path,
        folder: &Path,
        new_version: &str,
    ) -> bool {
        let Ok(lockfile) = read_lockfile(lockfile_path).await else {
            return true;
        };
        let Some(locked) = lockfile
            .get(&dependency.name)
            .filter(|locked| !locked.pending)
        else {
            return true;
        };
        if locked.version != new_version {
            return true;
        }
        // A changed `url` downloads from somewhere else, even at the same version.
        if dependency.is_url() && locked.url.as_deref() != Some(dependency.repository.as_str()) {
            return true;
        }
        // Only `singleLockfile` records a location; there it is what tells a moved dependency
        // from an unchanged one.
        if self.workspace.config.single_lockfile.is_some() {
            let lock_dir = lockfile_path.parent().unwrap_or_else(|| Path::new("."));
            if saved_folder(lock_dir, locked).as_deref() != Some(folder) {
                return true;
            }
        }
        // An installer keeps no files. It is in place while the program it installed is, so one
        // removed through Settings is installed again.
        if dependency.installer.is_some() {
            return !locked.files.is_empty()
                || !locked.installed.as_ref().is_some_and(installer::is_present);
        }
        // Folders and patterns are checked against what they matched at install, re-rendered through
        // the current destinations. A changed destination renders differently and reinstalls.
        let Ok(selections) = dependency.selections(new_version) else {
            return true;
        };
        let Some(matched) = locked_matches(&selections, locked) else {
            return true;
        };
        let Ok(files) = expanded_lock(
            &dependency.name,
            &dependency.files,
            &selections,
            &matched,
            new_version,
        ) else {
            return true;
        };
        let links = dependency.links();
        files != locked.files
            || flat_files(&files)
                .iter()
                .any(|file| !anchor(folder, file.as_str()).exists())
            || links.iter().map(|link| &link.file).ne(locked.bin.iter())
            || links
                .iter()
                .any(|link| !crate::bin_links::is_in_place(folder, link))
    }
}

#[cfg(test)]
mod tests {
    use super::means_no_release;
    use crate::error::VendorError;

    #[test]
    fn only_a_real_answer_counts_as_the_repository_having_no_release() {
        // What GitHub actually says when there is nothing to find.
        assert!(means_no_release(&VendorError::RequestFailed {
            status: 404,
            message: "Not Found".to_owned(),
        }));
        assert!(means_no_release(&VendorError::NoMatchingRelease {
            regex: "^v".to_owned(),
            owner: "o".to_owned(),
            repo: "r".to_owned(),
        }));

        // Failures, every one of which used to come back as "could not find a version" and
        // bury its own reason. This is the guard that stopped a bad GITHUB_TOKEN doing that.
        for failure in [
            VendorError::BadCredentials,
            VendorError::RateLimited(String::new()),
            VendorError::RequestFailed {
                status: 401,
                message: String::new(),
            },
            VendorError::RequestFailed {
                status: 403,
                message: "Resource protected by organization SAML enforcement".to_owned(),
            },
            VendorError::RequestFailed {
                status: 503,
                message: String::new(),
            },
            VendorError::Http("connection reset".to_owned()),
        ] {
            assert!(
                !means_no_release(&failure),
                "{failure} would be reported as a missing release"
            );
        }
    }
}
