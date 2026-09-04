//! `install` - resolve a version, refresh the dependency folder, update config and lockfile.
//!
//! An install is split into three stages so `sync` can overlap the slow one across
//! dependencies:
//!
//! 1. [`Session::prepare`] - decide the version and whether anything is stale. Read-only.
//! 2. [`download`] - fetch and extract into the dependency folder, reporting to the
//!    dependency's own progress line as it goes. Owns everything it needs, so it can run on
//!    its own task.
//! 3. [`Session::commit`] - write the lockfile, update the config, settle the line. Ordered.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::archive;
use crate::error::{Result, VendorError};
use crate::fsx::{self, anchor, delete_file_and_empty_folders, join_normalized, stream_to_file};
use crate::github::GitHubClient;
use crate::lockfile::{
    VendorLock, config_files_to_lock_files, files_from_lockfile, write_lockfile,
};
use crate::model::{Dependency, FileSpec, FileTarget, RawDependency, flatten_files};
use crate::ops::{OpResult, Session};
use crate::progress;
use crate::remote_zip;
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
    /// This dependency's line in the display. Shared because the download stage runs on its
    /// own task while `commit` waits to settle the same line.
    pub progress: Arc<progress::Dependency>,
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
        let progress = Arc::new(self.progress.dependency(&dependency.name));
        progress.status("resolving version");
        let version = match self.decide_version(&dependency, &options).await {
            Ok(version) => version,
            // Reported as this dependency's failure rather than handed straight out by `?`. The
            // version lookup is the first request an install makes and the one a bad token trips
            // first, and left to `?` it settled no row and wrote no line - so a piped run ended
            // with an error naming nothing.
            Err(error) => {
                let error = blame(&progress, error);
                self.progress.end();
                return Err(error);
            }
        };
        let prepared = self
            .prepare(dependency, options, version, Arc::clone(&progress))
            .await;
        // The two stages below report their own failures, so there is nothing to add here.
        let outcome = match download(Arc::clone(&self.github), prepared).await {
            Ok(prepared) => self.commit(prepared).await,
            Err(error) => Err(error),
        };
        // Closed here rather than left to `Reporter`'s `Drop`, which drains the held lines only
        // once the session goes - by which point `main` has printed `ERROR:` and the outcome row
        // would land underneath the error it belongs above.
        self.progress.end();
        outcome
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
        progress: Arc<progress::Dependency>,
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
            progress,
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
    pub async fn commit(&mut self, prepared: Prepared) -> OpResult {
        let Prepared {
            dependency,
            options,
            lockfile_path,
            version,
            needs_update,
            progress,
            folder: _,
        } = prepared;

        if options.show_outdated_only {
            progress.finish_quietly();
            if needs_update {
                report_outdated(&dependency, &version);
            }
            return Ok(None);
        }

        if !needs_update {
            progress.up_to_date();
            return Ok(None);
        }

        progress.committing("writing lockfile");
        write_lockfile(
            &dependency.name,
            VendorLock {
                repository: dependency.repository.clone(),
                version: version.clone(),
                files: config_files_to_lock_files(&dependency.files, &version),
            },
            &lockfile_path,
        )
        .await
        .map_err(|error| blame(&progress, error))?;

        let old_version = dependency.version.clone();
        if old_version.as_deref() != Some(version.as_str()) {
            progress.committing("updating config");
            self.record_version(&dependency, &version)
                .await
                .map_err(|error| blame(&progress, error))?;
        }

        if options.should_update {
            match old_version.as_deref() {
                // A dependency with no previous version was installed, not updated.
                Some(old) => progress.updated(old, &version),
                None => progress.installed(&version),
            }
            return Ok(Some(version));
        }

        progress.installed(&version);
        Ok(None)
    }

    /// Writes the new version back to the config file, adding the dependency if it is new.
    ///
    /// Registering a new dependency is a deliberate divergence: the reference crashes here
    /// because it never inserts the entry the `install` command was asked to create.
    ///
    /// The in-memory `dependencies` map is left alone for entries that already existed. The
    /// reference keeps a deep copy of them, so a version written mid-run is never visible to
    /// the staleness checks of later dependencies - and neither is it here.
    async fn record_version(&mut self, dependency: &Dependency, new_version: &str) -> Result<()> {
        let name = dependency.name.clone();
        if self.workspace.dependencies.contains_key(&name) {
            self.workspace
                .file
                .document
                .set_dependency_version(&name, new_version);
        } else {
            let mut entry = RawDependency {
                version: Some(new_version.to_owned()),
                repository: Some(dependency.repository.clone()),
                files: Some(dependency.files.clone()),
                hash_version_file: dependency.hash_version_file.clone(),
                vendor_folder: dependency.vendor_folder.clone(),
                release_regex: dependency.release_regex.clone(),
                locked: dependency.locked.then_some(true),
                name: None,
            };
            // The resolved dependency has the `default` block folded into it. Writing those
            // values back would restate the defaults in every entry, so drop them again -
            // `load` will fold them in next time just the same.
            let written = {
                let mut written = entry.clone();
                written.strip_defaults(&self.workspace.defaults);
                written
            };
            self.workspace
                .file
                .document
                .upsert_dependency(&name, &written)?;
            entry.version = Some(new_version.to_owned());
            self.workspace.dependencies.insert(name, entry);
        }
        self.workspace.file.write().await
    }
}

/// Prints one line of `vendor outdated` output.
fn report_outdated(dependency: &Dependency, new_version: &str) {
    let line = match dependency.version.as_deref() {
        Some(old) if old != new_version => format!(
            "{} {} -> {}",
            dependency.name,
            ui::red(old),
            ui::green(new_version)
        ),
        _ => format!("{} {new_version}", dependency.name),
    };
    // Through the display, not around it: the listing is the command's real output, but it
    // still has to wait for the bars to step aside.
    progress::print_out(&line);
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
pub async fn download(github: Arc<GitHubClient>, prepared: Prepared) -> Result<Prepared> {
    if !prepared.has_work() {
        return Ok(prepared);
    }
    if let Err(error) = fetch_all(&github, &prepared).await {
        return Err(blame(&prepared.progress, error));
    }
    // Committing waits its turn behind earlier dependencies; there is nothing to animate in
    // the meantime, and the writes themselves are local and immediate.
    prepared.progress.waiting();
    Ok(prepared)
}

/// Clears the folder and fetches every file into it.
///
/// Split out of [`download`] only so that a failure of any part of it is reported once, in one
/// place, against the right dependency.
async fn fetch_all(github: &GitHubClient, prepared: &Prepared) -> Result<()> {
    tokio::fs::create_dir_all(&prepared.folder).await?;
    prepared.progress.status("clearing previous install");
    remove_previously_installed(
        &prepared.dependency.name,
        &prepared.folder,
        &prepared.lockfile_path,
    )
    .await?;

    download_all(
        github,
        &prepared.dependency,
        &prepared.folder,
        &prepared.version,
        &prepared.progress,
    )
    .await
}

/// Marks a dependency's row failed and hands the error straight back.
///
/// For the stages that have to report their own failures. `download` runs on its own task and
/// `commit` is handed the dependency by value, so in both cases the caller is left holding an
/// error with nothing in it to say which dependency it belonged to - `sync` awaits a
/// `Result<Prepared>` and, on the `Err` side, never learns the name. Reported here, the row is
/// settled and the plain line written while the name is still in scope.
fn blame(progress: &progress::Dependency, error: VendorError) -> VendorError {
    progress.failed(&error.brief());
    error
}

/// Deletes whatever the previous install left behind, per the lockfile.
async fn remove_previously_installed(
    name: &str,
    folder: &Path,
    lockfile_path: &Path,
) -> Result<()> {
    for file in files_from_lockfile(lockfile_path, name).await {
        let path = anchor(folder, file.as_str());
        if !path.exists() {
            continue;
        }
        // The running binary cannot be deleted while it runs - on Windows the attempt fails
        // outright - and it does not need to be: the download that follows replaces it in place.
        if fsx::is_running_executable(&path) {
            continue;
        }
        delete_file_and_empty_folders(folder, &file).await?;
    }
    Ok(())
}

/// Downloads every declared file: repository files first, then release assets.
///
/// Each batch runs concurrently, and every file in it gets its own bar for as long as it is in
/// flight.
async fn download_all(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    progress: &progress::Dependency,
) -> Result<()> {
    let (release_files, repo_files): (Vec<FileSpec>, Vec<FileSpec>) =
        flatten_files(&dependency.files)
            .into_iter()
            .partition(|spec| is_release_path(&spec.input));

    if !repo_files.is_empty() {
        progress.status(format!("downloading {} file(s)", repo_files.len()));
        let saved =
            futures_util::future::try_join_all(repo_files.iter().map(|spec| {
                download_repo_file(github, dependency, folder, version, spec, progress)
            }))
            .await?;
        record(progress, saved.iter());
    }

    if !release_files.is_empty() {
        progress.status(format!(
            "downloading {} release asset(s)",
            release_files.len()
        ));
        let saved = futures_util::future::try_join_all(release_files.iter().map(|spec| {
            download_release_file(github, dependency, folder, version, spec, progress)
        }))
        .await?;
        record(progress, saved.iter().flatten());
    }

    Ok(())
}

/// Notes every destination a batch produced, in declaration order.
///
/// The transfers finish in whatever order the network decides; ordering the record here is what
/// keeps piped output identical from run to run.
fn record<'a>(progress: &progress::Dependency, saved: impl Iterator<Item = &'a PathBuf>) {
    for path in saved {
        progress.saved(path);
    }
}

/// Downloads one file from the repository tree at `version`.
async fn download_repo_file(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    spec: &FileSpec,
    progress: &progress::Dependency,
) -> Result<PathBuf> {
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

    let save_path = anchor(folder, output.as_str());
    let transfer = progress.transfer(response.content_length());
    stream_to_file(response, &save_path, Some(&transfer)).await?;
    drop(transfer);
    Ok(save_path)
}

/// Downloads one release asset, extracting it when the target names archive members.
async fn download_release_file(
    github: &GitHubClient,
    dependency: &Dependency,
    folder: &Path,
    version: &str,
    spec: &FileSpec,
    progress: &progress::Dependency,
) -> Result<Vec<PathBuf>> {
    let asset = strip_release_prefix(&replace_version(&spec.input, version));

    let Some(pairs) = spec.output.extraction_pairs() else {
        let FileTarget::Rename(output) = &spec.output else {
            unreachable!("extraction_pairs returns None only for Rename");
        };
        // The asset itself is the file, so all of it is wanted.
        let response = fetch_asset(github, dependency, &asset, version).await?;
        let save_path = anchor(folder, replace_version(output, version).as_str());
        let transfer = progress.transfer(response.content_length());
        stream_to_file(response, &save_path, Some(&transfer)).await?;
        drop(transfer);
        return Ok(vec![save_path]);
    };

    // Only the declared members come out. An asset that ships every platform's binary, or a
    // manual nobody asked for, costs neither the disk to unpack it nor - for a tar stream - the
    // CPU to decompress past the last file wanted.
    let wanted: Vec<String> = pairs
        .iter()
        .map(|(from, _)| replace_version(from, version))
        .collect();

    let temp = tempfile::Builder::new()
        .prefix("vendorfiles-")
        .tempdir()
        .map_err(VendorError::from)?;
    let extracted = temp.path().join("extracted");

    if !read_remotely(
        github, dependency, &asset, version, &wanted, &extracted, progress,
    )
    .await
    {
        download_and_extract(
            github,
            dependency,
            &asset,
            version,
            &wanted,
            temp.path(),
            progress,
        )
        .await?;
    }

    let mut saved = Vec::with_capacity(pairs.len());
    for (from, to) in pairs {
        let source = join_normalized(&extracted, &[replace_version(&from, version).as_str()]);
        let destination = anchor(folder, replace_version(&to, version).as_str());
        move_extracted(&source, &destination).await?;
        saved.push(destination);
    }
    Ok(saved)
}

/// Starts a streaming download of one of a dependency's release assets.
async fn fetch_asset(
    github: &GitHubClient,
    dependency: &Dependency,
    asset: &str,
    version: &str,
) -> Result<reqwest::Response> {
    github
        .download_release_asset(
            &dependency.repo,
            asset,
            version,
            dependency.release_regex.as_deref(),
        )
        .await
}

/// Pulls the wanted members straight out of the asset over HTTP range requests.
///
/// Answers whether it worked, and is never an error: all of this is an optimisation over
/// downloading the asset, so every reason it cannot happen - a container that is not a ZIP,
/// storage that will not serve ranges, an index that does not hold one of the members - simply
/// hands the work back to [`download_and_extract`]. A user should never see a failure that only
/// this route could produce.
async fn read_remotely(
    github: &GitHubClient,
    dependency: &Dependency,
    asset: &str,
    version: &str,
    wanted: &[String],
    extracted: &Path,
    progress: &progress::Dependency,
) -> bool {
    // ZIP is the only container here whose index is at the end, which is the whole basis for
    // reading one out of order. Checked by name so a tarball never costs even the one request it
    // would take to find that out.
    if !asset.to_ascii_lowercase().ends_with(".zip") {
        return false;
    }
    // Under `WORTH_RANGING` the round trips cost more than the bytes they save, and the release
    // JSON already knows how big the asset is - so a small one is ruled out without a request.
    let Ok(Some(source)) = github
        .asset_range_source(
            &dependency.repo,
            asset,
            version,
            dependency.release_regex.as_deref(),
            remote_zip::WORTH_RANGING,
        )
        .await
    else {
        return false;
    };

    progress.status(format!("reading {} member(s) from {asset}", wanted.len()));
    // The transfer is unmeasured: how much of the asset is about to be read is only known once
    // the index has been, and by then most of the reading is done. The bytes are still counted
    // as they land, which is what the display actually shows.
    let transfer = progress.transfer(None);
    let (arrivals, mut arrived) = tokio::sync::mpsc::unbounded_channel();
    let source = github.range_source(source, Some(arrivals));

    let destination = extracted.to_path_buf();
    let wanted = wanted.to_vec();
    let reading = tokio::task::spawn_blocking(move || {
        remote_zip::extract_members(source, &destination, &wanted)
    });
    // Ends when the blocking task drops the source, and with it the sender.
    let counting = async {
        while let Some(bytes) = arrived.recv().await {
            transfer.advance(bytes);
        }
    };
    let (outcome, ()) = tokio::join!(reading, counting);
    matches!(outcome, Ok(Ok(true)))
}

/// Downloads the whole asset into `temp` and extracts the wanted members from it.
async fn download_and_extract(
    github: &GitHubClient,
    dependency: &Dependency,
    asset: &str,
    version: &str,
    wanted: &[String],
    temp: &Path,
    progress: &progress::Dependency,
) -> Result<()> {
    // Said again because the range route may have set its own status before handing the work
    // back: what happens next is a download, and the line has to say so.
    progress.status(format!("downloading {asset}"));
    let response = fetch_asset(github, dependency, asset, version).await?;
    // Saved under the asset's own name: a lone `.gz` or `.xz` extracts to its name minus the
    // suffix, so a random temporary name would produce the wrong member.
    let archive_path = temp.join(asset);
    let transfer = progress.transfer(response.content_length());
    // Reported as the extraction failure the reference prints for it, because that is what a
    // half-written archive amounts to - but carrying the reason, which used to be dropped here
    // so that extraction could rediscover the symptom and not the cause.
    stream_to_file(response, &archive_path, Some(&transfer))
        .await
        .map_err(|source| cannot_extract(asset, source))?;
    drop(transfer);

    let extract_target = temp.join("extracted");
    let wanted = wanted.to_vec();
    progress.status(format!("extracting {asset}"));
    let name = asset.to_owned();
    tokio::task::spawn_blocking(move || {
        archive::extract_members(&archive_path, &extract_target, &wanted)
    })
    .await
    .map_err(|e| VendorError::Http(e.to_string()))?
    .map_err(|source| cannot_extract(&name, source))
}

/// The reference's wording for an archive that would not open, over the reason it would not.
fn cannot_extract(asset: &str, source: VendorError) -> VendorError {
    VendorError::CannotExtract {
        file: asset.to_owned(),
        source: Box::new(source),
    }
}

/// Moves an extracted member into the dependency folder.
///
/// Falls back to copy-then-delete when the temp directory is on another filesystem, which
/// `rename` cannot cross - the reference fails outright in that case.
async fn move_extracted(source: &Path, destination: &Path) -> Result<()> {
    let fail = |source_error: std::io::Error| VendorError::MoveFailed {
        from: source.display().to_string(),
        to: destination.display().to_string(),
        source: source_error,
    };

    tokio::fs::metadata(source).await.map_err(&fail)?;
    if fsx::is_running_executable(destination) {
        // The archive member is a new copy of this very binary: hand the swap to `self-replace`
        // rather than trying to move onto an image the operating system has open.
        return fsx::replace_running_executable(source).await;
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(&fail)?;
    }
    if tokio::fs::rename(source, destination).await.is_err() {
        tokio::fs::copy(source, destination).await.map_err(&fail)?;
        let _ = tokio::fs::remove_file(source).await;
    }
    Ok(())
}
