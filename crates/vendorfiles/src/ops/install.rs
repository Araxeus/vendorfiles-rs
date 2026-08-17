//! `install` — resolve a version, refresh the dependency folder, update config and lockfile.
//!
//! An install is split into three stages so `sync` can overlap the slow one across
//! dependencies without disturbing log order:
//!
//! 1. [`Session::prepare`] — decide the version and whether anything is stale. Read-only.
//! 2. [`download`] — fetch and extract into the dependency folder, collecting the log lines it
//!    *would* have printed. Owns everything it needs, so it can run on its own task.
//! 3. [`Session::commit`] — print those lines, write the lockfile, update the config. Ordered.

use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// A dependency with its version decided and its staleness known.
///
/// Fully owned, so the download stage can be moved onto a task.
#[derive(Debug)]
pub struct Prepared {
    pub dependency: Dependency,
    pub options: InstallOptions,
    pub folder: PathBuf,
    pub lockfile_path: PathBuf,
    pub version: String,
    pub needs_update: bool,
}

impl Prepared {
    /// Whether the download stage has any work to do.
    #[must_use]
    pub const fn has_work(&self) -> bool {
        self.needs_update && !self.options.show_outdated_only
    }
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
        let version = self.decide_version(&dependency, &options).await?;
        let prepared = self.prepare(dependency, options, version).await;
        let (prepared, logs) = download(Arc::clone(&self.github), prepared).await?;
        self.commit(prepared, logs).await
    }

    /// Picks the version to install: the explicit one, the configured one, or a fresh lookup.
    ///
    /// # Errors
    ///
    /// Returns whatever the release or commit lookup produced when one was needed.
    pub async fn decide_version(
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

    /// Works out where the dependency lives and whether it is stale. Touches nothing.
    pub async fn prepare(
        &self,
        dependency: Dependency,
        options: InstallOptions,
        version: String,
    ) -> Prepared {
        let folder = self
            .workspace
            .dependency_folder(dependency.vendor_folder.as_deref(), &dependency.name);
        let lockfile_path = Self::lockfile_path(&folder);
        let needs_update = options.force
            || self
                .needs_update(&dependency.name, &lockfile_path, &version)
                .await;

        Prepared {
            dependency,
            options,
            folder,
            lockfile_path,
            version,
            needs_update,
        }
    }

    /// Reports the outcome and makes it durable: log lines, lockfile, config.
    ///
    /// Everything here is ordered and sequential, which is what keeps `sync`'s output identical
    /// to the reference's even though the downloads overlapped.
    ///
    /// # Errors
    ///
    /// Returns a write error if the lockfile or the config file cannot be updated.
    pub async fn commit(&mut self, prepared: Prepared, logs: Vec<String>) -> OpResult {
        let Prepared {
            dependency,
            options,
            lockfile_path,
            version,
            needs_update,
            folder: _,
        } = prepared;

        if options.show_outdated_only {
            if needs_update {
                report_outdated(&dependency, &version);
            }
            return Ok(None);
        }

        if !needs_update {
            ui::info(format!("{} is up to date", dependency.name));
            return Ok(None);
        }

        for line in logs {
            ui::info(line);
        }

        write_lockfile(
            &dependency.name,
            VendorLock {
                repository: dependency.repository.clone(),
                version: version.clone(),
                files: config_files_to_lock_files(&dependency.files, &version),
            },
            &lockfile_path,
        )
        .await?;

        let old_version = dependency.version.clone();
        if old_version.as_deref() != Some(version.as_str()) {
            self.record_version(&dependency, &version).await?;
        }

        if options.should_update {
            ui::success(format!(
                "Updated {} from {} to {}",
                dependency.name,
                display_version(old_version.as_deref()),
                version
            ));
            return Ok(Some(version));
        }

        ui::success(format!("Installed {} {}", dependency.name, version));
        Ok(None)
    }

    /// Writes the new version back to the config file, adding the dependency if it is new.
    ///
    /// Registering a new dependency is a deliberate divergence: the reference crashes here
    /// because it never inserts the entry the `install` command was asked to create.
    ///
    /// The in-memory `dependencies` map is left alone for entries that already existed. The
    /// reference keeps a deep copy of them, so a version written mid-run is never visible to
    /// the staleness checks of later dependencies — and neither is it here.
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
            self.workspace.dependencies.insert(name, entry);
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

/// Fetches everything a prepared dependency needs, returning it with its pending log lines.
///
/// Deliberately a free function taking an `Arc`: it borrows nothing from the session, so `sync`
/// can run one of these per dependency on its own task while committing earlier ones.
///
/// # Errors
///
/// Returns whatever the downloads, the archive extraction or the pruning of the previous
/// install produced.
pub async fn download(
    github: Arc<GitHubClient>,
    prepared: Prepared,
) -> Result<(Prepared, Vec<String>)> {
    if !prepared.has_work() {
        return Ok((prepared, Vec::new()));
    }

    tokio::fs::create_dir_all(&prepared.folder).await?;
    remove_previously_installed(
        &prepared.dependency.name,
        &prepared.folder,
        &prepared.lockfile_path,
    )
    .await?;

    let logs = download_all(
        &github,
        &prepared.dependency,
        &prepared.folder,
        &prepared.version,
    )
    .await?;
    Ok((prepared, logs))
}

/// Deletes whatever the previous install left behind, per the lockfile.
async fn remove_previously_installed(
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

/// Downloads every declared file: repository files first, then release assets.
///
/// Each batch runs concurrently, matching the reference's two `Promise.all` phases. The log
/// lines come back in declaration order rather than completion order, which the reference
/// leaves to chance.
async fn download_all(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
) -> Result<Vec<String>> {
    let (release_files, repo_files): (Vec<FileSpec>, Vec<FileSpec>) =
        flatten_files(&dependency.files)
            .into_iter()
            .partition(|spec| is_release_path(&spec.input));

    let mut logs = futures_util::future::try_join_all(
        repo_files
            .iter()
            .map(|spec| download_repo_file(github, dependency, folder, version, spec)),
    )
    .await?;

    let release_logs = futures_util::future::try_join_all(
        release_files
            .iter()
            .map(|spec| download_release_file(github, dependency, folder, version, spec)),
    )
    .await?;

    logs.extend(release_logs.into_iter().flatten());
    Ok(logs)
}

/// Downloads one file from the repository tree at `version`.
async fn download_repo_file(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    spec: &FileSpec,
) -> Result<String> {
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
    stream_to_file(response, &save_path, true).await?;
    Ok(format!("Saved {}", save_path.display()))
}

/// Downloads one release asset, extracting it when the target names archive members.
async fn download_release_file(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    spec: &FileSpec,
) -> Result<Vec<String>> {
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
        stream_to_file(response, &save_path, true).await?;
        return Ok(vec![format!("Saved {}", save_path.display())]);
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

    let mut logs = Vec::with_capacity(pairs.len());
    for (from, to) in pairs {
        let source = join_normalized(&extracted, &[replace_version(&from, version).as_str()]);
        let destination = join_normalized(folder, &[replace_version(&to, version).as_str()]);
        move_extracted(&source, &destination).await?;
        logs.push(format!("Saved {}", destination.display()));
    }
    Ok(logs)
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
