//! Lock entries for dependencies the config no longer lists.
//!
//! Installs only ever add to a lockfile, and `uninstall` is the only command that takes an entry
//! out. A dependency deleted or renamed by editing the config by hand leaves its entry behind.
//! `sync` calls [`Session::prune_lockfiles`] to drop those entries. The files they recorded stay
//! on disk, because deleting them is what `uninstall` is for.

use std::path::Path;

use indexmap::IndexSet;

use crate::error::{Result, VendorError};
use crate::lockfile::{Lockfile, read_lockfile, to_json, write_string};
use crate::ops::Session;

impl Session {
    /// Drops every lock entry the config does not name, leaving its files in place.
    ///
    /// Only lockfiles a configured dependency writes to are visited.
    ///
    /// # Errors
    ///
    /// Returns a write error if a lockfile cannot be rewritten or removed.
    pub(crate) async fn prune_lockfiles(&self) -> Result<()> {
        let lockfiles: IndexSet<_> = self
            .workspace
            .dependencies
            .iter()
            .map(|(name, raw)| {
                let folder = self
                    .workspace
                    .dependency_folder(raw.vendor_folder.as_deref(), name);
                self.workspace.lockfile_path(&folder)
            })
            .collect();

        for lockfile_path in lockfiles {
            self.prune_lockfile(&lockfile_path).await?;
        }
        Ok(())
    }

    /// Prunes one lockfile, rewriting it only when an entry actually went.
    async fn prune_lockfile(&self, lockfile_path: &Path) -> Result<()> {
        let Ok(mut lockfile) = read_lockfile(lockfile_path).await else {
            return Ok(());
        };
        let before = lockfile.len();
        lockfile.retain(|name, _| self.workspace.dependencies.contains_key(name));
        if lockfile.len() == before {
            return Ok(());
        }
        write(lockfile_path, &lockfile).await
    }
}

/// Writes a pruned lockfile back, or removes it once it holds nothing.
async fn write(lockfile_path: &Path, lockfile: &Lockfile) -> Result<()> {
    if lockfile.is_empty() {
        return tokio::fs::remove_file(lockfile_path)
            .await
            .map_err(|source| VendorError::WriteFile {
                path: lockfile_path.to_path_buf(),
                source,
            });
    }
    write_string(lockfile_path, &format!("{}\n", to_json(lockfile)?)).await
}
