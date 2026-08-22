//! Deciding which version a dependency should be at, and whether anything must change.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::error::{Result, VendorError};
use crate::lockfile::{config_files_to_lock_files, flat_files, read_lockfile};
use crate::model::Dependency;
use crate::ops::Session;

impl Session {
    /// Resolves the version to install.
    ///
    /// With `hashVersionFile` this is the tracked file's latest commit SHA; otherwise it is
    /// the latest release tag. A repository with no usable release yields an empty string -
    /// except under `outdated`, where the reference treats it as fatal.
    pub(crate) async fn resolve_new_version(
        &self,
        dependency: &Dependency,
        show_outdated_only: bool,
    ) -> Result<String> {
        if let Some(path) = dependency.hash_version_target()? {
            return self
                .github
                .file_commit_sha(&dependency.repo, &path)
                .await
                .map_err(|source| VendorError::CommitShaLookup {
                    path,
                    source: Box::new(source),
                });
        }

        match self
            .github
            .latest_release(&dependency.repo, dependency.release_regex.as_deref())
            .await
        {
            Ok(release) => Ok(release.tag_name.clone()),
            Err(_) if show_outdated_only => {
                Err(VendorError::NoVersionFound(dependency.name.clone()))
            }
            Err(_) => Ok(String::new()),
        }
    }

    /// Whether the dependency folder is out of step with the config.
    ///
    /// Any inconsistency - missing lockfile, version drift, a file that vanished, a changed
    /// `files` declaration - means "yes". Every failure path is also "yes", matching the
    /// reference's blanket `catch`.
    pub(crate) async fn needs_update(
        &self,
        name: &str,
        lockfile_path: &Path,
        new_version: &str,
    ) -> bool {
        let Ok(lockfile) = read_lockfile(lockfile_path).await else {
            return true;
        };
        let Some(locked) = lockfile.get(name) else {
            return true;
        };
        if locked.version != new_version {
            return true;
        }
        if self
            .files_from_config()
            .iter()
            .any(|(path, owner)| owner == name && !path.exists())
        {
            return true;
        }
        let Some(configured) = self.workspace.dependencies.get(name) else {
            return true;
        };
        let Some(files) = configured.files.as_deref() else {
            return true;
        };
        let expected =
            config_files_to_lock_files(files, configured.version.as_deref().unwrap_or_default());
        expected != locked.files
    }

    /// Every path the config expects on disk, mapped to the dependency that owns it.
    pub(crate) fn files_from_config(&self) -> IndexMap<PathBuf, String> {
        let mut files = IndexMap::new();
        for (name, dependency) in &self.workspace.dependencies {
            let Some(declared) = dependency.files.as_deref() else {
                continue;
            };
            let folder = self.workspace.dependency_folder(
                dependency.vendor_folder.as_deref(),
                dependency
                    .name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .unwrap_or(name),
            );
            let lock_files = config_files_to_lock_files(
                declared,
                dependency.version.as_deref().unwrap_or_default(),
            );
            for file in flat_files(&lock_files) {
                files.insert(
                    crate::fsx::join_normalized(&folder, &[file.as_str()]),
                    name.clone(),
                );
            }
        }
        files
    }
}
