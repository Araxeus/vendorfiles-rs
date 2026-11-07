//! `uninstall` - remove a dependency's files, its lock entry, and its config entry.

use crate::error::{Result, VendorError};
use crate::fsx::delete_file_and_empty_folders;
use crate::lockfile::{
    Lockfile, config_files_to_lock_files, flat_files, previous_entry, read_lockfile,
    retire_folder_entry, saved_folder, to_json, write_string,
};
use crate::model::RawDependency;
use crate::ops::Session;
use std::path::{Path, PathBuf};

impl Session {
    /// Removes a dependency entirely.
    ///
    /// A file the user already removed must not stop the config entry from going away, so a
    /// missing file counts as deleted. A file that *refuses* to be deleted is the opposite case
    /// and stops the run: the config entry and the lockfile are what record that the file is
    /// there, and dropping them while the file survives leaves something on disk that nothing
    /// knows about. Windows produces this whenever the file is an executable that is running.
    /// Nothing has been removed from the config at that point, so closing the program and
    /// running the command again finishes the job.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::DependencyNotFound`] when the config has no such entry,
    /// [`VendorError::DeleteFailed`] for a file that could not be removed, or a write error if
    /// the lockfile or config cannot be updated.
    pub async fn uninstall(&mut self, name: &str) -> Result<()> {
        let progress = self.progress.dependency(name);
        progress.status("removing files");
        let dependency = self
            .workspace
            .dependencies
            .get(name)
            .cloned()
            .ok_or_else(|| VendorError::DependencyNotFound {
                name: name.to_owned(),
                path: self.workspace.file.display_path(),
            })?;

        let folder = self
            .workspace
            .dependency_folder(dependency.vendor_folder.as_deref(), name);
        let lockfile_path = self.workspace.lockfile_path(&folder);

        let single = self.workspace.config.single_lockfile.is_some();
        let lockfile = read_lockfile(&lockfile_path).await.ok();
        let owned = Owned::find(name, &dependency, &folder, &lockfile_path, single).await;
        if let Err(error) = owned.delete().await {
            progress.failed(&error.brief());
            self.progress.end();
            return Err(error);
        }

        if single {
            retire_folder_entry(name, &folder, &lockfile_path).await;
        }
        if let Some(lockfile) = lockfile.filter(|l| l.contains_key(name)) {
            drop_lock_entry(name, lockfile, &lockfile_path, &folder).await?;
        }

        self.workspace.dependencies.shift_remove(name);
        self.workspace.file.document.remove_dependency(name)?;
        self.workspace.file.write().await?;

        progress.uninstalled();
        Ok(())
    }
}

/// The files a dependency owns, by the lockfile and by the config.
struct Owned {
    /// Where the lock entry says the files were installed.
    locked_folder: PathBuf,
    locked_files: Vec<String>,
    /// Where the config puts the dependency today.
    folder: PathBuf,
    declared_files: Vec<String>,
}

impl Owned {
    async fn find(
        name: &str,
        dependency: &RawDependency,
        folder: &Path,
        lockfile_path: &Path,
        single: bool,
    ) -> Self {
        let locked = previous_entry(name, folder, lockfile_path, single).await;

        // With `singleLockfile` the files may sit where the entry was installed, not where the
        // config points today.
        let lock_dir = lockfile_path.parent().unwrap_or_else(|| Path::new("."));
        let locked_folder = locked
            .as_ref()
            .and_then(|entry| saved_folder(lock_dir, entry))
            .unwrap_or_else(|| folder.to_path_buf());
        let locked_files = locked
            .as_ref()
            .map(|entry| flat_files(&entry.files))
            .unwrap_or_default();

        let declared = config_files_to_lock_files(
            dependency.files.as_deref().unwrap_or_default(),
            dependency.version.as_deref().unwrap_or_default(),
        );
        Self {
            locked_folder,
            locked_files,
            folder: folder.to_path_buf(),
            declared_files: flat_files(&declared),
        }
    }

    /// Deletes every owned file, stopping at the first one that refuses to go.
    async fn delete(&self) -> Result<()> {
        let locked = self
            .locked_files
            .iter()
            .map(|file| (&self.locked_folder, file));
        let declared = self.declared_files.iter().map(|file| (&self.folder, file));
        for (root, file) in locked.chain(declared) {
            delete_file_and_empty_folders(root, file).await?;
        }
        if self.locked_folder != self.folder {
            let _ = tokio::fs::remove_dir(&self.locked_folder).await;
        }
        Ok(())
    }
}

/// Removes `name` from a lockfile, deleting the file when it was the only entry.
async fn drop_lock_entry(
    name: &str,
    mut lockfile: Lockfile,
    lockfile_path: &Path,
    folder: &Path,
) -> Result<()> {
    if lockfile.len() == 1 {
        // Still best-effort: by here every file this dependency owned is gone, and a stray
        // lockfile or an empty folder records nothing that is not true.
        let _ = tokio::fs::remove_file(lockfile_path).await;
        if is_empty_dir(folder).await {
            let _ = tokio::fs::remove_dir_all(folder).await;
        }
        return Ok(());
    }
    lockfile.shift_remove(name);

    // The reference writes this one without a trailing newline.
    write_string(lockfile_path, &to_json(&lockfile)?).await
}

async fn is_empty_dir(path: &std::path::Path) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => entries.next_entry().await.ok().flatten().is_none(),
        Err(_) => false,
    }
}
