//! `sync`, `update` and `outdated` — all three are one traversal with different flags.

use std::io::Write;

use crate::error::Result;
use crate::ops::{InstallOptions, Session};
use crate::ui;

/// How a traversal of every configured dependency should behave.
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// Look for newer versions instead of honouring the configured one.
    pub should_update: bool,
    /// Reinstall everything regardless of the lockfile.
    pub force: bool,
    /// Only report what is outdated.
    pub show_outdated_only: bool,
}

/// One dependency that moved to a new version.
#[derive(Debug, Clone)]
struct Bump {
    name: String,
    url: String,
    old_version: String,
    new_version: String,
}

impl Session {
    /// Walks every dependency in config order, installing or reporting as configured.
    ///
    /// Order is deliberate: dependencies are processed one at a time so log output is
    /// deterministic and identical to the reference, while the files *within* a dependency
    /// download concurrently.
    ///
    /// # Errors
    ///
    /// Returns the first error any dependency produces; earlier dependencies keep their effects.
    pub async fn sync(&mut self, options: SyncOptions) -> Result<()> {
        let names: Vec<String> = self.workspace.dependencies.keys().cloned().collect();
        let mut bumps: Vec<Bump> = Vec::new();

        for name in names {
            let raw = self.workspace.dependencies[&name].clone();
            raw.validate(&name)?;
            let dependency = raw.resolve(&name)?;

            let old_version = dependency.version.clone();
            let should_update = !dependency.locked && options.should_update;

            let new_version = self
                .install(
                    dependency.clone(),
                    InstallOptions {
                        should_update,
                        force: options.force,
                        new_version: None,
                        show_outdated_only: options.show_outdated_only,
                    },
                )
                .await?;

            if should_update && !options.show_outdated_only {
                if let (Some(old), Some(new)) = (old_version, new_version) {
                    if !old.is_empty() && !new.is_empty() && old != new {
                        bumps.push(Bump {
                            name,
                            url: dependency.repository,
                            old_version: old,
                            new_version: new,
                        });
                    }
                }
            }
        }

        if ui::pr_mode() && !bumps.is_empty() {
            print_pull_request_body(&bumps);
        }
        Ok(())
    }
}

/// Writes the `--pr` summary to stdout, without a trailing newline.
fn print_pull_request_body(bumps: &[Bump]) {
    let body = bumps
        .iter()
        .map(|bump| {
            format!(
                "* Bump [{}]({}) from ❌ {} to ✅ {}",
                bump.name, bump.url, bump.old_version, bump.new_version
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(body.as_bytes());
    let _ = stdout.flush();
}
