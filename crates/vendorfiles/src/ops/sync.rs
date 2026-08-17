//! `sync`, `update` and `outdated` — all three are one traversal with different flags.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::error::{Result, VendorError};
use crate::model::Dependency;
use crate::ops::install::{download, Prepared};
use crate::ops::{InstallOptions, Session};
use crate::ui;

/// How many dependencies may be downloading at once.
///
/// Enough to hide latency on a large config, low enough that GitHub does not see a burst of
/// requests from one client.
const MAX_CONCURRENT_DOWNLOADS: usize = 8;

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

/// A dependency queued for installation, with the decision about whether to look for a
/// newer version already made.
#[derive(Debug)]
struct Plan {
    dependency: Dependency,
    options: InstallOptions,
}

/// What `commit` needs to know about a dependency after its `Prepared` has been consumed.
#[derive(Debug)]
struct BumpCandidate {
    name: String,
    url: String,
    old_version: Option<String>,
    tracked: bool,
}

/// One dependency that moved to a new version.
#[derive(Debug, Clone)]
struct Bump {
    name: String,
    url: String,
    old_version: String,
    new_version: String,
}

type DownloadTask = JoinHandle<Result<(Prepared, Vec<String>)>>;

impl Session {
    /// Walks every dependency, installing or reporting as configured.
    ///
    /// Three passes, arranged so the slow parts overlap and the visible parts do not:
    ///
    /// 1. **Resolve** every target version concurrently. One API round trip per dependency,
    ///    and the cost that dominates `update` and `outdated` on a large config.
    /// 2. **Download** each dependency on its own task, up to
    ///    [`MAX_CONCURRENT_DOWNLOADS`] at a time, collecting the log lines instead of
    ///    printing them.
    /// 3. **Commit** in config order: print each dependency's lines, write its lockfile, update
    ///    the config. Because this awaits the tasks in order, output still streams out
    ///    dependency by dependency, in exactly the reference's sequence, while later
    ///    dependencies are still downloading.
    ///
    /// Errors from an earlier pass are held until the ordered pass reaches them, so the first
    /// failure reported is always the first failure in the file. When one is reached, the
    /// remaining downloads are cancelled.
    ///
    /// # Errors
    ///
    /// Returns the first error any dependency produces; earlier dependencies keep their effects.
    pub async fn sync(&mut self, options: SyncOptions) -> Result<()> {
        let plans = self.plan(&options)?;

        let versions = futures_util::future::join_all(
            plans
                .iter()
                .map(|plan| self.decide_version(&plan.dependency, &plan.options)),
        )
        .await;

        let mut prepared = Vec::with_capacity(plans.len());
        for (plan, version) in plans.into_iter().zip(versions) {
            prepared.push(self.prepare(plan.dependency, plan.options, version?).await);
        }

        let mut tasks = self.spawn_downloads(prepared);
        let mut bumps: Vec<Bump> = Vec::new();

        while let Some(task) = tasks.pop_front() {
            let outcome = task.await.map_err(|e| VendorError::Http(e.to_string()));
            let (prepared, logs) = match outcome.and_then(|inner| inner) {
                Ok(pair) => pair,
                Err(error) => {
                    cancel(&tasks);
                    return Err(error);
                }
            };

            let candidate = BumpCandidate {
                name: prepared.dependency.name.clone(),
                url: prepared.dependency.repository.clone(),
                old_version: prepared.dependency.version.clone(),
                tracked: prepared.options.should_update && !prepared.options.show_outdated_only,
            };

            match self.commit(prepared, logs).await {
                Ok(new_version) => {
                    if let Some(bump) = candidate.into_bump(new_version) {
                        bumps.push(bump);
                    }
                }
                Err(error) => {
                    cancel(&tasks);
                    return Err(error);
                }
            }
        }

        if ui::pr_mode() && !bumps.is_empty() {
            print_pull_request_body(&bumps);
        }
        Ok(())
    }

    /// Validates every dependency and resolves it, in config order.
    fn plan(&self, options: &SyncOptions) -> Result<Vec<Plan>> {
        let mut plans = Vec::with_capacity(self.workspace.dependencies.len());
        for (name, raw) in &self.workspace.dependencies {
            raw.validate(name)?;
            let dependency = raw.resolve(name)?;
            plans.push(Plan {
                options: InstallOptions {
                    should_update: !dependency.locked && options.should_update,
                    force: options.force,
                    new_version: None,
                    show_outdated_only: options.show_outdated_only,
                },
                dependency,
            });
        }
        Ok(plans)
    }

    /// Starts one download task per dependency, bounded by a shared permit pool.
    fn spawn_downloads(&self, prepared: Vec<Prepared>) -> VecDeque<DownloadTask> {
        let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
        prepared
            .into_iter()
            .map(|item| {
                let github = Arc::clone(&self.github);
                let permits = Arc::clone(&permits);
                tokio::spawn(async move {
                    // A closed semaphore is impossible here; treat it as "no limit" anyway.
                    let _permit = permits.acquire().await.ok();
                    download(github, item).await
                })
            })
            .collect()
    }
}

impl BumpCandidate {
    /// The bump line for this dependency, if it actually moved.
    fn into_bump(self, new_version: Option<String>) -> Option<Bump> {
        if !self.tracked {
            return None;
        }
        let old = self.old_version.filter(|v| !v.is_empty())?;
        let new = new_version.filter(|v| !v.is_empty())?;
        if old == new {
            return None;
        }
        Some(Bump {
            name: self.name,
            url: self.url,
            old_version: old,
            new_version: new,
        })
    }
}

/// Cancels download work that will never be committed.
fn cancel(tasks: &VecDeque<DownloadTask>) {
    for task in tasks {
        task.abort();
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
