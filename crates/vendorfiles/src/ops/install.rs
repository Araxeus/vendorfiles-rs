//! `install` — resolve a version, refresh the dependency folder, update config and lockfile.

use std::path::Path;

use crate::archive;
use crate::error::{Result, VendorError};
use crate::fsx::{delete_file_and_empty_folders, join_normalized, stream_to_file};
use crate::github::GitHubClient;
use crate::lockfile::{
    config_files_to_lock_files, files_from_lockfile, write_lockfile, VendorLock,
};
use crate::model::{flatten_files, Dependency, FileSpec, FileTarget, RawDependency};
use crate::ops::{display_version, OpResult, Session};
use crate::template::{is_release_path, replace_version, strip_release_prefix};
use crate::ui;

/// How an install should behave.
#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Resolve the latest version rather than trusting the configured one.
    pub should_update: bool,
    /// Reinstall even when the lockfile says everything is in place.
    pub force: bool,
    /// Install this exact version.
    pub new_version: Option<String>,
    /// Report what would change and stop.
    pub show_outdated_only: bool,
}

impl Session {
    /// Installs (or refreshes) a single dependency.
    ///
    /// Returns the version installed when `should_update` is set, so `sync` can build its
    /// pull-request summary; `None` otherwise or when nothing was done.
    ///
    /// # Errors
    ///
    /// Returns whichever [`VendorError`](crate::VendorError) the version lookup, the downloads,
    /// the archive extraction or the config/lockfile write produced.
    pub async fn install(&mut self, dependency: Dependency, options: InstallOptions) -> OpResult {
        let folder = self
            .workspace
            .dependency_folder(dependency.vendor_folder.as_deref(), &dependency.name);
        let lockfile_path = Self::lockfile_path(&folder);

        let new_version = self.decide_version(&dependency, &options).await?;
        let needs_update = options.force
            || self
                .needs_update(&dependency.name, &lockfile_path, &new_version)
                .await;

        if options.show_outdated_only {
            if needs_update {
                report_outdated(&dependency, &new_version);
            }
            return Ok(None);
        }

        if !needs_update {
            ui::info(format!("{} is up to date", dependency.name));
            return Ok(None);
        }

        tokio::fs::create_dir_all(&folder).await?;
        self.remove_previously_installed(&dependency.name, &folder, &lockfile_path)
            .await?;
        download_all(&self.github, &dependency, &folder, &new_version).await?;

        write_lockfile(
            &dependency.name,
            VendorLock {
                repository: dependency.repository.clone(),
                version: new_version.clone(),
                files: config_files_to_lock_files(&dependency.files, &new_version),
            },
            &lockfile_path,
        )
        .await?;

        let old_version = dependency.version.clone();
        if old_version.as_deref() != Some(new_version.as_str()) {
            self.record_version(&dependency, &new_version).await?;
        }

        if options.should_update {
            ui::success(format!(
                "Updated {} from {} to {}",
                dependency.name,
                display_version(old_version.as_deref()),
                new_version
            ));
            return Ok(Some(new_version));
        }

        ui::success(format!("Installed {} {}", dependency.name, new_version));
        Ok(None)
    }

    /// Picks the version to install: the explicit one, the configured one, or a fresh lookup.
    async fn decide_version(
        &self,
        dependency: &Dependency,
        options: &InstallOptions,
    ) -> Result<String> {
        let mut candidate = options.new_version.clone().filter(|v| !v.is_empty());
        if !(options.should_update || candidate.is_some()) {
            candidate.clone_from(&dependency.version);
        }
        match candidate.filter(|v| !v.is_empty()) {
            Some(version) => Ok(version),
            None => {
                self.resolve_new_version(dependency, options.show_outdated_only)
                    .await
            }
        }
    }

    /// Deletes whatever the previous install left behind, per the lockfile.
    async fn remove_previously_installed(
        &self,
        name: &str,
        folder: &Path,
        lockfile_path: &Path,
    ) -> Result<()> {
        for file in files_from_lockfile(lockfile_path, name).await {
            if join_normalized(folder, &[file.as_str()]).exists() {
                delete_file_and_empty_folders(folder, &file).await?;
            }
        }
        Ok(())
    }

    /// Writes the new version back to the config file, adding the dependency if it is new.
    ///
    /// Registering a new dependency is a deliberate divergence: the reference crashes here
    /// because it never inserts the entry the `install` command was asked to create.
    async fn record_version(&mut self, dependency: &Dependency, new_version: &str) -> Result<()> {
        let name = dependency.name.clone();
        if self.workspace.dependencies.contains_key(&name) {
            self.workspace
                .file
                .document
                .set_dependency_version(&name, new_version);
        } else {
            let entry = RawDependency {
                version: Some(new_version.to_owned()),
                repository: Some(dependency.repository.clone()),
                files: Some(dependency.files.clone()),
                hash_version_file: dependency.hash_version_file.clone(),
                vendor_folder: dependency.vendor_folder.clone(),
                release_regex: dependency.release_regex.clone(),
                locked: dependency.locked.then_some(true),
                name: None,
            };
            self.workspace
                .file
                .document
                .upsert_dependency(&name, &entry)?;
            self.workspace.dependencies.insert(name.clone(), entry);
        }
        if let Some(entry) = self.workspace.dependencies.get_mut(&name) {
            entry.version = Some(new_version.to_owned());
        }
        self.workspace.file.write().await
    }
}

/// Prints one line of `vendor outdated` output.
fn report_outdated(dependency: &Dependency, new_version: &str) {
    match dependency.version.as_deref() {
        Some(old) if old != new_version => {
            println!(
                "{} {} -> {}",
                dependency.name,
                ui::red(old),
                ui::green(new_version)
            );
        }
        _ => println!("{} {new_version}", dependency.name),
    }
}

/// Downloads every declared file: repository files first, then release assets.
///
/// Each batch runs concurrently, matching the reference's two `Promise.all` phases.
async fn download_all(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
) -> Result<()> {
    let (release_files, repo_files): (Vec<FileSpec>, Vec<FileSpec>) =
        flatten_files(&dependency.files)
            .into_iter()
            .partition(|spec| is_release_path(&spec.input));

    futures_util::future::try_join_all(
        repo_files
            .iter()
            .map(|spec| download_repo_file(github, dependency, folder, version, spec)),
    )
    .await?;

    futures_util::future::try_join_all(
        release_files
            .iter()
            .map(|spec| download_release_file(github, dependency, folder, version, spec)),
    )
    .await?;

    Ok(())
}

/// Downloads one file from the repository tree at `version`.
async fn download_repo_file(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    spec: &FileSpec,
) -> Result<()> {
    let FileTarget::Rename(output) = &spec.output else {
        let rendered = serde_json::to_string(&(&spec.input, &spec.output))
            .unwrap_or_else(|_| spec.input.clone());
        return Err(VendorError::NonStringOutputWithoutRelease(rendered));
    };

    let response = github
        .download_file(&dependency.repo, &spec.input, Some(version))
        .await
        .map_err(|source| VendorError::FileDownloadFailed {
            file: spec.input.clone(),
            repository: dependency.repository.clone(),
            version: version.to_owned(),
            source: Box::new(source),
        })?;

    let save_path = join_normalized(folder, &[output.as_str()]);
    stream_to_file(response, &save_path, true).await
}

/// Downloads one release asset, extracting it when the target names archive members.
async fn download_release_file(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    spec: &FileSpec,
) -> Result<()> {
    let asset = strip_release_prefix(&replace_version(&spec.input, version));
    let response = github
        .download_release_asset(
            &dependency.repo,
            &asset,
            version,
            dependency.release_regex.as_deref(),
        )
        .await?;

    let Some(pairs) = spec.output.extraction_pairs() else {
        let FileTarget::Rename(output) = &spec.output else {
            unreachable!("extraction_pairs returns None only for Rename");
        };
        let save_path = join_normalized(folder, &[replace_version(output, version).as_str()]);
        return stream_to_file(response, &save_path, true).await;
    };

    let temp = tempfile::Builder::new()
        .prefix("vendorfiles-")
        .tempdir()
        .map_err(VendorError::from)?;
    let archive_path = temp.path().join(&asset);
    stream_to_file(response, &archive_path, false).await?;

    let extracted = temp.path().join("extracted");
    let extract_target = extracted.clone();
    tokio::task::spawn_blocking(move || archive::extract(&archive_path, &extract_target))
        .await
        .map_err(|e| VendorError::Http(e.to_string()))?
        .map_err(|_| VendorError::CannotExtract(asset.clone()))?;

    for (from, to) in pairs {
        let source = join_normalized(&extracted, &[replace_version(&from, version).as_str()]);
        let destination = join_normalized(folder, &[replace_version(&to, version).as_str()]);
        move_extracted(&source, &destination).await?;
        ui::info(format!("Saved {}", destination.display()));
    }
    Ok(())
}

/// Moves an extracted member into the dependency folder.
///
/// Falls back to copy-then-delete when the temp directory is on another filesystem, which
/// `rename` cannot cross — the reference fails outright in that case.
async fn move_extracted(source: &Path, destination: &Path) -> Result<()> {
    let fail = |source_error: std::io::Error| VendorError::MoveFailed {
        from: source.display().to_string(),
        to: destination.display().to_string(),
        source: source_error,
    };

    tokio::fs::metadata(source).await.map_err(&fail)?;
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(&fail)?;
    }
    if tokio::fs::rename(source, destination).await.is_err() {
        tokio::fs::copy(source, destination).await.map_err(&fail)?;
        let _ = tokio::fs::remove_file(source).await;
    }
    Ok(())
}
