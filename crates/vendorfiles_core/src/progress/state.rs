//! What the run is doing, as plain data.
//!
//! Nothing here knows about a terminal. The renderer reads a snapshot of this and draws it; the
//! rest of the crate only ever mutates it. Keeping the two apart is what makes the display
//! testable - a frame is a function of a state, and both halves can be checked on their own.

use std::borrow::Cow;
use std::time::Instant;

/// A dependency's position in the config file, which is also its identity here.
pub type DepId = usize;

/// Bytes in flight for one dependency, aggregated over however many files it is fetching.
///
/// One dependency can be downloading several files at once, and a row is one line, so the files
/// are summed rather than shown apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bytes {
    /// Transfers still running.
    pub active: usize,
    /// Bytes that have arrived.
    pub done: u64,
    /// Bytes promised across the running transfers.
    pub expected: u64,
    /// Transfers that never advertised a size.
    pub unmeasured: usize,
}

impl Bytes {
    /// How far along, when that can be known at all.
    ///
    /// A single transfer of unknown length makes the total unknowable, so the whole row falls
    /// back to counting bytes rather than showing a percentage that would be a guess.
    #[must_use]
    pub fn ratio(&self) -> Option<f64> {
        if self.unmeasured > 0 || self.expected == 0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a ratio to two significant figures; f64 holds byte counts exactly to 2^53"
        )]
        Some((self.done as f64 / self.expected as f64).clamp(0.0, 1.0))
    }

    /// Whether a transfer is running, and so whether the row should show a bar.
    #[must_use]
    pub const fn transferring(&self) -> bool {
        self.active > 0
    }
}

/// What one dependency is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Not started: no permit yet, or nothing to do.
    Queued,
    /// Working. `bytes` is non-empty while files are arriving.
    Active {
        /// What it is doing, shown when no transfer is running.
        label: Cow<'static, str>,
        /// Transfer totals, summed over this dependency's files.
        bytes: Bytes,
    },
    /// Downloaded, waiting its turn to install.
    ///
    /// Commits run in config order, so a dependency that finishes early waits for every earlier
    /// one. It keeps a place in the display rather than vanishing, which is what the old row
    /// model got wrong.
    Waiting,
    /// Writing files, lockfile and config. Only ever one at a time.
    Committing {
        /// The step in progress.
        label: Cow<'static, str>,
    },
    /// Settled, showing what happened to it.
    ///
    /// It keeps its row rather than leaving, so a completed dependency is reported in place
    /// instead of being pushed above the region - which moved the region down the screen once
    /// per dependency.
    Done {
        /// How it ended, which decides the mark and the colour.
        outcome: Outcome,
        /// The version installed, the change made, or why not.
        detail: Cow<'static, str>,
    },
    /// Settled with nothing to say, for commands whose real output is elsewhere.
    Gone,
}

/// How a dependency ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Installed, updated or removed.
    Changed,
    /// Nothing needed doing.
    Unchanged,
    /// It failed; the error itself is reported by the caller.
    Failed,
}

impl Stage {
    /// How badly this dependency needs one of the limited rows, lowest first.
    ///
    /// `None` means it has nothing to show. Committing outranks downloading because it is the
    /// head of the ordered commit loop - the one thing everything else is waiting for - and
    /// downloading outranks waiting so that a backlog of finished downloads can never hide the
    /// transfers that are actually moving.
    #[must_use]
    pub const fn priority(&self) -> Option<u8> {
        match self {
            Self::Committing { .. } => Some(0),
            Self::Active { .. } => Some(1),
            Self::Waiting => Some(2),
            // Lowest of all: a finished dependency holds its row only until something with work
            // to do needs it, so recent results stay readable without ever hiding live work.
            Self::Done { .. } => Some(3),
            Self::Queued | Self::Gone => None,
        }
    }
}

/// One dependency's name and stage.
#[derive(Debug, Clone)]
pub struct DepState {
    /// As written in the config file.
    pub name: String,
    /// What it is doing now.
    pub stage: Stage,
}

/// How many dependencies are in each stage, for the footer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Downloaded, waiting their turn to install.
    pub waiting: usize,
    /// Not started yet.
    pub queued: usize,
}

/// Everything the display draws.
#[derive(Debug)]
pub struct RunState {
    /// Dependencies in config order.
    pub deps: Vec<DepState>,
    /// Dependencies in the config, which is the denominator of the summary.
    pub total: usize,
    /// Dependencies that have settled.
    pub done: usize,
    /// What the run as a whole is doing.
    pub phase: Cow<'static, str>,
    /// Worker rows the run reserved.
    pub rows: usize,
    /// Which dependency is shown on each worker row, `None` for an empty one.
    ///
    /// A dependency keeps the row it was given until it has nothing left to show. Re-deriving
    /// the list every frame instead - even sorted into config order - slides every row up
    /// whenever one of them finishes, which on a large config is continuous motion.
    pub slots: Vec<Option<DepId>>,
    /// Bytes received across every dependency, for the footer.
    pub bytes: u64,
    /// When the run started, for elapsed time and rate.
    pub started: Instant,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            deps: Vec::new(),
            total: 0,
            done: 0,
            phase: Cow::Borrowed("resolving versions"),
            rows: 0,
            slots: Vec::new(),
            bytes: 0,
            started: Instant::now(),
        }
    }
}

impl RunState {
    /// Registers a dependency and hands back its identity.
    pub fn register(&mut self, name: &str) -> DepId {
        self.deps.push(DepState {
            name: name.to_owned(),
            stage: Stage::Queued,
        });
        self.deps.len() - 1
    }

    /// The stage of one dependency, if it is registered.
    pub fn stage_mut(&mut self, id: DepId) -> Option<&mut Stage> {
        self.deps.get_mut(id).map(|dep| &mut dep.stage)
    }

    /// Sets how many worker rows there are, discarding any assignment.
    pub fn resize(&mut self, rows: usize) {
        self.rows = rows;
        self.slots = vec![None; rows];
    }

    /// Decides which dependency sits on which row.
    ///
    /// A dependency **keeps its row** until it has nothing left to show; a row that frees up is
    /// refilled in place. That is the whole point: rows are a set of places, not a list that
    /// closes up when one of its members leaves. Re-deriving the list every frame - even sorted
    /// into config order - moves every row below whichever one finished, and with a large config
    /// finishing something every few hundred milliseconds, the display never stops moving.
    ///
    /// Empty rows are filled by [`Stage::priority`], then config order, so:
    ///
    /// * the first rows to fill do so in config order, and read like the file;
    /// * a committing dependency - the one everything else is queued behind - always has a row;
    /// * a backlog of finished downloads can never hide a transfer that is still moving, because
    ///   an active dependency will evict a merely waiting one when nothing is free.
    pub fn assign(&mut self) {
        if self.slots.len() != self.rows {
            self.slots.resize(self.rows, None);
        }
        if self.rows == 0 {
            return;
        }

        // Let go of rows whose dependency has finished.
        for index in 0..self.slots.len() {
            if let Some(id) = self.slots[index]
                && self.rank(id).is_none()
            {
                self.slots[index] = None;
            }
        }

        // Everything that wants a row and does not have one, best first.
        let mut waiting: Vec<(u8, DepId)> = self
            .deps
            .iter()
            .enumerate()
            .filter(|(id, _)| !self.slots.contains(&Some(*id)))
            .filter_map(|(id, dep)| dep.stage.priority().map(|rank| (rank, id)))
            .collect();
        waiting.sort_unstable();
        let mut queue = waiting.into_iter().peekable();

        // Fill what is free, in row order, so the first pass reads like the config.
        for index in 0..self.slots.len() {
            if self.slots[index].is_some() {
                continue;
            }
            match queue.next() {
                Some((_, id)) => self.slots[index] = Some(id),
                None => break,
            }
        }

        // Nothing free: let more important work take the least important row.
        while let Some(&(rank, id)) = queue.peek() {
            let Some((index, worst)) = self.worst_row() else {
                break;
            };
            if rank >= worst {
                break;
            }
            self.slots[index] = Some(id);
            queue.next();
        }
    }

    /// How badly the dependency on a row needs it, or `None` if it has finished.
    fn rank(&self, id: DepId) -> Option<u8> {
        self.deps.get(id).and_then(|dep| dep.stage.priority())
    }

    /// The row whose occupant needs it least, and how much it needs it.
    ///
    /// Ties go to the dependency earliest in the config, which - because commits are ordered - is
    /// the one that finished longest ago. That keeps the most recent results on screen.
    fn worst_row(&self) -> Option<(usize, u8)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.and_then(|id| self.rank(id).map(|rank| (index, rank, id)))
            })
            .max_by_key(|&(_, rank, id)| (rank, std::cmp::Reverse(id)))
            .map(|(index, rank, _)| (index, rank))
    }

    /// The dependencies on the worker rows, in row order, with gaps for empty rows.
    #[must_use]
    pub fn selection(&self) -> &[Option<DepId>] {
        &self.slots
    }

    /// How many dependencies are waiting or queued, whether or not they got a row.
    #[must_use]
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for dep in &self.deps {
            match dep.stage {
                Stage::Waiting => counts.waiting += 1,
                Stage::Queued => counts.queued += 1,
                Stage::Active { .. }
                | Stage::Committing { .. }
                | Stage::Done { .. }
                | Stage::Gone => {}
            }
        }
        counts
    }

    /// Bytes per second so far, or `None` before there is enough to divide by.
    #[must_use]
    pub fn rate(&self) -> Option<f64> {
        let elapsed = self.started.elapsed().as_secs_f64();
        if elapsed <= 0.05 || self.bytes == 0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a throughput figure shown to one decimal place"
        )]
        Some(self.bytes as f64 / elapsed)
    }

    /// How far through the run, for the summary bar.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "dependency counts are far below f64's exact integer range"
        )]
        let ratio = self.done as f64 / self.total as f64;
        ratio.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Bytes, Outcome, RunState, Stage};
    use std::borrow::Cow;

    fn done() -> Stage {
        Stage::Done {
            outcome: Outcome::Changed,
            detail: Cow::Borrowed("installed v1.0.0"),
        }
    }

    fn downloading(bytes: Bytes) -> Stage {
        Stage::Active {
            label: Cow::Borrowed("downloading"),
            bytes,
        }
    }

    fn state(stages: Vec<Stage>) -> RunState {
        rows(stages, 3)
    }

    fn rows(stages: Vec<Stage>, rows: usize) -> RunState {
        let mut state = RunState {
            total: stages.len(),
            ..RunState::default()
        };
        state.resize(rows);
        for (index, stage) in stages.into_iter().enumerate() {
            let id = state.register(&format!("dep-{index}"));
            *state.stage_mut(id).unwrap() = stage;
        }
        state.assign();
        state
    }

    /// The dependencies on the rows, ignoring which row each is on.
    fn occupants(state: &RunState) -> Vec<usize> {
        let mut ids: Vec<usize> = state.selection().iter().flatten().copied().collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn a_ratio_needs_every_transfer_to_have_advertised_a_size() {
        assert_eq!(
            Bytes {
                active: 1,
                done: 512,
                expected: 1024,
                unmeasured: 0
            }
            .ratio(),
            Some(0.5)
        );
        assert_eq!(
            Bytes {
                active: 2,
                done: 512,
                expected: 1024,
                unmeasured: 1
            }
            .ratio(),
            None
        );
        assert_eq!(Bytes::default().ratio(), None);
    }

    #[test]
    fn a_ratio_cannot_exceed_one_even_if_a_server_lied_about_the_length() {
        let bytes = Bytes {
            active: 1,
            done: 4096,
            expected: 1024,
            unmeasured: 0,
        };
        assert_eq!(bytes.ratio(), Some(1.0));
    }

    #[test]
    fn rows_go_to_the_work_that_matters() {
        // Five dependencies want a row and only three exist. The committing one and the two
        // downloading ones win; the two merely waiting do not.
        let state = state(vec![
            Stage::Waiting,
            downloading(Bytes::default()),
            Stage::Waiting,
            Stage::Committing {
                label: Cow::Borrowed("writing lockfile"),
            },
            downloading(Bytes::default()),
        ]);
        assert_eq!(occupants(&state), vec![1, 3, 4]);
    }

    #[test]
    fn the_first_rows_to_fill_do_so_in_config_order() {
        let state = rows(
            vec![
                downloading(Bytes::default()),
                downloading(Bytes::default()),
                downloading(Bytes::default()),
            ],
            3,
        );
        assert_eq!(state.selection(), [Some(0), Some(1), Some(2)]);
    }

    #[test]
    fn a_dependency_keeps_its_row_while_it_still_has_something_to_show() {
        // The heart of it: the row a dependency sits on must not move because some *other*
        // dependency finished. That movement is the bug this scheduler exists to prevent.
        let mut state = rows(
            vec![
                downloading(Bytes::default()),
                downloading(Bytes::default()),
                downloading(Bytes::default()),
            ],
            3,
        );
        assert_eq!(state.selection(), [Some(0), Some(1), Some(2)]);

        // The first one finishes. The other two must stay exactly where they are.
        *state.stage_mut(0).unwrap() = Stage::Gone;
        state.assign();
        assert_eq!(state.selection(), [None, Some(1), Some(2)]);

        // A newcomer takes the freed row rather than everything sliding up.
        let id = state.register("latecomer");
        *state.stage_mut(id).unwrap() = downloading(Bytes::default());
        state.assign();
        assert_eq!(state.selection(), [Some(id), Some(1), Some(2)]);
    }

    #[test]
    fn assigning_again_changes_nothing_on_its_own() {
        let mut state = state(vec![
            downloading(Bytes::default()),
            Stage::Waiting,
            Stage::Waiting,
        ]);
        let before = state.selection().to_vec();
        state.assign();
        state.assign();
        assert_eq!(state.selection(), before.as_slice());
    }

    #[test]
    fn a_backlog_of_finished_downloads_never_hides_a_running_one() {
        // Everything early in the file has downloaded and is waiting its turn, and every row is
        // taken. The only active transfer is last, and it must still take a row from a waiter.
        let mut stages = vec![Stage::Waiting; 8];
        stages.push(downloading(Bytes::default()));
        let state = state(stages);
        assert!(
            occupants(&state).contains(&8),
            "the running download must take a row from a merely waiting one"
        );
    }

    #[test]
    fn a_committing_dependency_outranks_a_downloading_one_for_the_last_row() {
        let mut stages = vec![downloading(Bytes::default()); 3];
        stages.push(Stage::Committing {
            label: Cow::Borrowed("writing lockfile"),
        });
        let state = state(stages);
        assert!(
            occupants(&state).contains(&3),
            "the head of the commit queue must be visible"
        );
    }

    #[test]
    fn settled_and_unstarted_dependencies_take_no_rows() {
        let state = state(vec![Stage::Gone, Stage::Queued, Stage::Waiting]);
        assert_eq!(occupants(&state), vec![2]);
    }

    #[test]
    fn a_finished_dependency_keeps_its_row_until_something_needs_it() {
        // Reporting in place is the point: a completed dependency stays on its own row rather
        // than being pushed above the region, which moved the region down the screen.
        let mut state = rows(vec![downloading(Bytes::default()); 2], 2);
        assert_eq!(state.selection(), [Some(0), Some(1)]);

        *state.stage_mut(0).unwrap() = done();
        state.assign();
        assert_eq!(
            state.selection(),
            [Some(0), Some(1)],
            "a finished dependency holds its row"
        );

        // Now real work arrives and both rows are taken. The finished one yields, not the live
        // one.
        let id = state.register("newcomer");
        *state.stage_mut(id).unwrap() = downloading(Bytes::default());
        state.assign();
        assert_eq!(state.selection(), [Some(id), Some(1)]);
    }

    #[test]
    fn the_oldest_result_is_the_one_that_makes_way() {
        // Commits are ordered, so the earliest dependency in the config finished longest ago.
        // Evicting that one keeps the most recent results on screen.
        let mut state = rows(vec![done(), done(), done()], 3);
        assert_eq!(state.selection(), [Some(0), Some(1), Some(2)]);
        let id = state.register("newcomer");
        *state.stage_mut(id).unwrap() = downloading(Bytes::default());
        state.assign();
        assert_eq!(state.selection(), [Some(id), Some(1), Some(2)]);
    }

    #[test]
    fn a_run_with_no_rows_selects_nothing() {
        // `outdated` never downloads, so it reserves no rows and shows the summary alone.
        let mut state = state(vec![Stage::Waiting, Stage::Waiting]);
        state.resize(0);
        state.assign();
        assert!(state.selection().is_empty());
    }

    #[test]
    fn no_dependency_ever_occupies_two_rows() {
        let mut state = rows(vec![downloading(Bytes::default()); 4], 8);
        for _ in 0..5 {
            state.assign();
        }
        let ids = occupants(&state);
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(ids, unique, "a dependency was shown twice: {ids:?}");
    }

    #[test]
    fn the_footer_counts_everything_in_flight_whether_or_not_it_got_a_row() {
        let state = state(vec![
            Stage::Waiting,
            Stage::Waiting,
            Stage::Queued,
            Stage::Gone,
            downloading(Bytes::default()),
        ]);
        let counts = state.counts();
        assert_eq!(counts.waiting, 2);
        assert_eq!(counts.queued, 1);
    }

    #[test]
    fn the_summary_ratio_survives_an_empty_run() {
        let state = RunState::default();
        assert!((state.ratio() - 0.0).abs() < f64::EPSILON);
    }
}
