//! `uninstall` - remove a dependency's files, its lock entry, and its config entry.

use crate::error::{Result, VendorError};
use crate::fsx::delete_file_and_empty_folders;
use crate::lockfile::{
    config_files_to_lock_files, flat_files, read_lockfile, to_json, write_string,
};
use crate::ops::Session;

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
        let lockfile_path = Self::lockfile_path(&folder);

        let lockfile = read_lockfile(&lockfile_path).await.ok();
        let locked_files = lockfile
            .as_ref()
            .and_then(|locked| locked.get(name))
            .map(|locked| flat_files(&locked.files))
            .unwrap_or_default();
        let declared = config_files_to_lock_files(
            dependency.files.as_deref().unwrap_or_default(),
            dependency.version.as_deref().unwrap_or_default(),
        );
        let declared_files = flat_files(&declared);

        for file in locked_files.iter().chain(declared_files.iter()) {
            if let Err(error) = delete_file_and_empty_folders(&folder, file).await {
                progress.failed(&error.brief());
                self.progress.end();
                return Err(error);
            }
        }

        if let Some(mut lockfile) = lockfile.filter(|l| l.contains_key(name)) {
            if lockfile.len() == 1 {
                // Still best-effort: by here every file this dependency owned is gone, and a
                // stray lockfile or an empty folder records nothing that is not true.
                let _ = tokio::fs::remove_file(&lockfile_path).await;
                if is_empty_dir(&folder).await {
                    let _ = tokio::fs::remove_dir_all(&folder).await;
                }
            } else {
                lockfile.shift_remove(name);
                // The reference writes this one without a trailing newline.
                write_string(&lockfile_path, &to_json(&lockfile)?).await?;
            }
        }

        self.workspace.dependencies.shift_remove(name);
        self.workspace.file.document.remove_dependency(name);
        self.workspace.file.write().await?;

        progress.uninstalled();
        Ok(())
    }
}

async fn is_empty_dir(path: &std::path::Path) -> bool {
    match tokio::fs::read_dir(path).await {
        Ok(mut entries) => entries.next_entry().await.ok().flatten().is_none(),
        Err(_) => false,
    }
}
