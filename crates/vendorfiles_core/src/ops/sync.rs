//! `sync`, `update` and `outdated` - all three are one traversal with different flags.

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use crate::error::{Result, VendorError};
use crate::model::Dependency;
use crate::ops::install::{Prepared, clear_previous, download};
use crate::ops::{InstallOptions, Session};
use crate::progress;
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
    progress: Arc<progress::Dependency>,
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

type DownloadTask = JoinHandle<Result<Prepared>>;

impl Session {
    /// Walks every dependency, installing or reporting as configured.
    ///
    /// Three passes, arranged so the slow parts overlap and the visible parts do not:
    ///
    /// 1. **Resolve** every target version concurrently. One API round trip per dependency,
    ///    and the cost that dominates `update` and `outdated` on a large config.
    /// 2. **Download** each dependency on its own task, up to
    ///    [`MAX_CONCURRENT_DOWNLOADS`] at a time, collecting the log lines instead of
    ///    printing them. Before the first one starts, what the previous installs left behind
    ///    is cleared for every dependency, since they may swap folders.
    /// 3. **Commit** in config order: update the config, write each lockfile, and settle that
    ///    dependency's line. Awaiting the tasks in order is what keeps piped output ordered
    ///    while later dependencies are still downloading.
    ///
    /// Then, unless only reporting, lock entries the config no longer lists are dropped. Their
    /// files stay on disk.
    ///
    /// Errors from an earlier pass are held until the ordered pass reaches them, so the first
    /// failure reported is always the first failure in the file. When one is reached, the
    /// remaining downloads are cancelled.
    ///
    /// # Errors
    ///
    /// Returns the first error any dependency produces; earlier dependencies keep their effects.
    /// A config entry that cannot be edited stops the run before any file is touched.
    pub async fn sync(&mut self, options: SyncOptions) -> Result<()> {
        self.progress.begin(self.workspace.dependencies.len());
        let plans = self.plan(&options).inspect_err(|_| self.progress.end())?;
        if plans.is_empty() {
            self.progress.end();
            ui::info(self.workspace.no_dependencies_notice());
            return Ok(());
        }

        let locked = plans.iter().filter(|plan| plan.dependency.locked).count();

        // Closed before any error leaves, so the outcome rows print above it.
        let outcome = self.run_plans(plans, &options).await;
        self.progress.end();
        let (bumps, stale) = outcome?;

        if !options.show_outdated_only {
            self.prune_lockfiles().await?;
        }

        if !stale {
            ui::info(nothing_changed(&options, locked));
        }

        if ui::pr_mode() && !bumps.is_empty() {
            print_pull_request_body(&bumps);
        }
        Ok(())
    }

    /// Runs the three passes, returning the version bumps they made and whether anything was stale.
    async fn run_plans(
        &mut self,
        plans: Vec<Plan>,
        options: &SyncOptions,
    ) -> Result<(Vec<Bump>, bool)> {
        let versions = futures_util::future::join_all(
            plans
                .iter()
                .map(|plan| self.decide_version(&plan.dependency, &plan.options)),
        )
        .await;
        self.progress.summary(if options.show_outdated_only {
            "checking for updates"
        } else {
            "installing"
        });

        let mut prepared = Vec::with_capacity(plans.len());
        for (plan, version) in plans.into_iter().zip(versions) {
            let version = version.inspect_err(|error| plan.progress.failed(&error.brief()))?;
            prepared.push(
                self.prepare(plan.dependency, plan.options, version, plan.progress)
                    .await,
            );
        }

        // Now that staleness is known, reserve exactly the rows the downloads can use: a
        // project already up to date - or `outdated`, which never downloads - gets none, and
        // the height is settled before the first byte moves.
        let working = prepared.iter().filter(|item| item.has_work()).count();
        let stale = prepared.iter().any(|item| item.needs_update);
        self.progress
            .reserve_rows(MAX_CONCURRENT_DOWNLOADS.min(working));

        for item in &prepared {
            self.check_recordable(item)?;
        }
        for item in &prepared {
            clear_previous(item).await?;
        }

        let mut tasks = self.spawn_downloads(prepared);
        let bumps = self.commit_in_order(&mut tasks).await;
        if bumps.is_err() {
            cancel(&tasks);
        }
        bumps.map(|bumps| (bumps, stale))
    }

    /// Awaits each download in config order and commits it, stopping at the first error.
    ///
    /// Tasks not yet reached stay in `tasks`, so the caller can cancel them.
    async fn commit_in_order(&mut self, tasks: &mut VecDeque<DownloadTask>) -> Result<Vec<Bump>> {
        let mut bumps = Vec::new();
        while let Some(task) = tasks.pop_front() {
            let prepared = task.await.map_err(|e| VendorError::Http(e.to_string()))??;

            // A traversal reports only what it affects. `vendor list` shows the rest.
            if !prepared.needs_update {
                prepared.progress.finish_quietly();
                continue;
            }

            let candidate = BumpCandidate::of(&prepared);
            let new_version = self.commit(prepared).await?;
            bumps.extend(candidate.into_bump(new_version));
        }
        Ok(bumps)
    }

    /// Validates every dependency and resolves it, in config order.
    fn plan(&self, options: &SyncOptions) -> Result<Vec<Plan>> {
        let mut plans = Vec::with_capacity(self.workspace.dependencies.len());
        for (name, raw) in &self.workspace.dependencies {
            raw.validate(name)?;
            let dependency = raw.resolve(name)?;
            let progress = Arc::new(
                self.progress
                    .dependency(name)
                    .linked_to(&dependency.repository),
            );
            plans.push(Plan {
                options: InstallOptions {
                    should_update: !dependency.locked && options.should_update,
                    force: options.force,
                    new_version: None,
                    show_outdated_only: options.show_outdated_only,
                },
                dependency,
                progress,
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
    fn of(prepared: &Prepared) -> Self {
        Self {
            name: prepared.dependency.name.clone(),
            url: prepared.dependency.repository.clone(),
            old_version: prepared.dependency.version.clone(),
            tracked: prepared.options.should_update && !prepared.options.show_outdated_only,
        }
    }

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

/// What a traversal says when no dependency was stale.
///
/// `sync` compares files against the config and never looks for newer versions, so it cannot
/// claim anything is up to date. `update` and `outdated` skip locked dependencies, so they count
/// them.
fn nothing_changed(options: &SyncOptions, locked: usize) -> String {
    match (options.should_update, locked) {
        (false, _) => "Already in sync".to_owned(),
        (true, 0) => "Everything is up to date".to_owned(),
        (true, locked) => format!("Everything is up to date ({locked} locked)"),
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
    crate::progress::print_raw(body.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::{SyncOptions, nothing_changed};

    #[test]
    fn only_a_version_check_counts_locked_dependencies() {
        let sync = SyncOptions::default();
        let update = SyncOptions {
            should_update: true,
            ..SyncOptions::default()
        };
        assert_eq!(nothing_changed(&sync, 2), "Already in sync");
        assert_eq!(nothing_changed(&update, 0), "Everything is up to date");
        assert_eq!(
            nothing_changed(&update, 2),
            "Everything is up to date (2 locked)"
        );
    }
}
