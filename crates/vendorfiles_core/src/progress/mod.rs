//! Live progress display.
//!
//! The display is a fixed region at the bottom of the terminal, redrawn from a snapshot of
//! [`state::RunState`] on a tick. It has a summary bar, a bounded set of worker rows showing the
//! work in flight, and a footer accounting for whatever did not fit. A dependency reports its
//! outcome on its own row and holds that row until something with work to do needs it; the lines
//! themselves are printed together as the region comes down, so the scrollback still gets the
//! whole run.
//!
//! Two rules keep it honest:
//!
//! * **Nothing writes to the terminal behind its back.** A raw `println!` lands wherever the
//!   cursor happens to be, which is inside the region as often as not. Everything goes through
//!   [`print_out`] or [`print_err`], which hand the line to the render thread.
//! * **The height is fixed for the whole run.** It is chosen once, when the run knows how much
//!   work there is, so nothing shifts under the reader's eye.
//!
//! The display animates only when **stdout is a terminal** and `--pr` is not asking for
//! machine-readable output. That is not a preference: crossterm's cursor query is hard-wired to
//! stdout, so an inline viewport anchored on stderr fails outright and leaks its query into the
//! data stream. When stdout is redirected there is no display, and the reporter falls back to the
//! plain `INFO:`/`SUCCESS:` lines - buffered per dependency and flushed as it settles - so piped
//! output keeps the bytes and the ordering it has always had.

pub mod ansi;
mod driver;
pub mod state;
pub mod view;

use std::borrow::Cow;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use driver::{Command, Driver};
use state::{Bytes, DepId, Outcome, RunState, Stage};

use crate::ui;

/// The render thread of the run in progress, for code that only needs to print a line.
///
/// A `Mutex` rather than a `OnceLock` because a run ends: once it has, printing goes straight to
/// the stream again.
static ACTIVE: Mutex<Option<Sender<Command>>> = Mutex::new(None);

/// Whether stderr is a terminal, worked out once.
static STDERR_IS_TTY: OnceLock<bool> = OnceLock::new();

/// Hands a command to the render thread, if one is running.
fn dispatch(command: Command) -> bool {
    let Ok(active) = ACTIVE.lock() else {
        return false;
    };
    let Some(sender) = active.as_ref() else {
        return false;
    };
    sender.send(command).is_ok()
}

/// Prints a line destined for stdout, above the region when one is live.
pub fn print_out(text: &str) {
    if !dispatch(Command::Print(ansi::to_lines(text))) {
        println!("{text}");
    }
}

/// Prints a line destined for stderr.
///
/// When stderr is a terminal it is the same screen as the display, so the line goes through the
/// region to keep the frame intact. When it is redirected it cannot disturb anything, so it is
/// written where it was addressed.
pub fn print_err(text: &str) {
    let tty = *STDERR_IS_TTY.get_or_init(|| std::io::stderr().is_terminal());
    if tty && dispatch(Command::Print(ansi::to_lines(text))) {
        return;
    }
    eprintln!("{text}");
}

/// Puts the terminal back the way it was found, as far as is possible in a hurry.
///
/// A signal runs no destructor: not [`Reporter::end`], not `Drop`, not the panic hook. Without
/// this, interrupting a sync leaves the cursor hidden - [`driver`] hides it on every frame - and
/// the shell that follows has no visible caret for the rest of the session.
///
/// The cursor comes first and the region second, because only the second one can be given up:
/// whatever cuts this short - a second Ctrl-C, a caller's deadline - has already had the cursor
/// back. Showing it is what stops the render thread from drawing, so nothing can hide it again;
/// until drawing could be stopped this had to be the other way round, and the wait was the risk.
pub fn restore_terminal() {
    driver::show_cursor();
    let sender = ACTIVE.lock().ok().and_then(|mut active| active.take());
    if let Some(sender) = sender {
        let _ = sender.send(Command::Stop);
        // Long enough for the thread to notice and wipe the region, short enough that nobody
        // remembers the wait.
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
}

/// Writes raw bytes to stdout, for output that is not line-shaped.
///
/// Only the `--pr` body, which never animates - it is the whole output of the command.
pub fn print_raw(bytes: &[u8]) {
    debug_assert!(
        ACTIVE.lock().map_or(true, |active| active.is_none()),
        "raw output would land inside a live region"
    );
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// The display for one run.
#[derive(Debug)]
pub struct Reporter {
    /// Cleared if the terminal turns out not to support the region.
    animated: AtomicBool,
    state: Arc<Mutex<RunState>>,
    driver: Mutex<Option<Driver>>,
    /// Outcome lines held until the run ends.
    ///
    /// Emitting them as they happen would push the region down the screen once per dependency.
    /// They are reported in place instead, on the dependency's own row, and printed together
    /// when the region comes down - so the scrollback still ends up with the whole run.
    results: Arc<Mutex<Vec<String>>>,
}

impl Default for Reporter {
    fn default() -> Self {
        Self::detect()
    }
}

impl Reporter {
    /// Builds a reporter that animates only if stdout is a terminal and `--pr` is not asking for
    /// machine-readable output.
    #[must_use]
    pub fn detect() -> Self {
        Self::new(std::io::stdout().is_terminal() && !ui::pr_mode())
    }

    /// Builds a reporter, choosing explicitly whether to animate.
    #[must_use]
    pub fn new(animated: bool) -> Self {
        Self {
            animated: AtomicBool::new(animated),
            state: Arc::new(Mutex::new(RunState::default())),
            driver: Mutex::new(None),
            results: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Whether anything is being animated.
    #[must_use]
    pub fn animated(&self) -> bool {
        self.animated.load(Ordering::Relaxed)
    }

    /// Opens the display for a run over `total` dependencies.
    ///
    /// The region starts with no worker rows: how many are wanted is not known until staleness
    /// has been checked, and a project already up to date should not be shown a column of blanks.
    pub fn begin(&self, total: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.total = total;
        }
        if !self.animated() || total == 0 {
            return;
        }
        let Some(driver) = Driver::start(Arc::clone(&self.state)) else {
            // No usable terminal after all. Fall back to the plain path rather than animating
            // into a void.
            self.animated.store(false, Ordering::Relaxed);
            return;
        };
        if let Ok(mut active) = ACTIVE.lock() {
            *active = Some(driver.commands());
        }
        if let Ok(mut slot) = self.driver.lock() {
            *slot = Some(driver);
        }
    }

    /// Fixes how many worker rows the region has, for the rest of the run.
    ///
    /// Capped by what the terminal can hold: a region taller than the screen cannot be
    /// repainted in place.
    pub fn reserve_rows(&self, count: usize) {
        let count = driver::fit(count);
        if let Ok(mut state) = self.state.lock() {
            state.resize(count);
        }
        if let Ok(driver) = self.driver.lock()
            && let Some(driver) = driver.as_ref()
        {
            driver.send(Command::Resize(count));
        }
    }

    /// Replaces the summary message.
    pub fn summary(&self, message: impl Into<Cow<'static, str>>) {
        if let Ok(mut state) = self.state.lock() {
            state.phase = message.into();
        }
    }

    /// Closes the display, wiping the region and restoring the cursor.
    ///
    /// The held outcome lines go out first, so they land above the region and are what remains on
    /// screen once it is wiped.
    pub fn end(&self) {
        let held: Vec<String> = self
            .results
            .lock()
            .map(|mut results| results.drain(..).collect())
            .unwrap_or_default();
        if !held.is_empty() {
            let lines = held.iter().flat_map(|line| ansi::to_lines(line)).collect();
            if !dispatch(Command::Print(lines)) {
                for line in held {
                    println!("{line}");
                }
            }
        }
        if let Ok(mut active) = ACTIVE.lock() {
            *active = None;
        }
        if let Ok(mut driver) = self.driver.lock()
            && let Some(mut driver) = driver.take()
        {
            driver.stop();
        }
    }

    /// A handle for one dependency, in config order.
    #[must_use]
    pub fn dependency(&self, name: &str) -> Dependency {
        let id = self
            .state
            .lock()
            .map_or(usize::MAX, |mut state| state.register(name));
        Dependency {
            id,
            name: name.to_owned(),
            animated: self.animated(),
            state: Arc::clone(&self.state),
            pending: Mutex::new(Vec::new()),
            results: Arc::clone(&self.results),
        }
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        self.end();
    }
}

/// The display for one dependency, plus the plain lines to emit if nothing is animating.
#[derive(Debug)]
pub struct Dependency {
    id: DepId,
    name: String,
    animated: bool,
    state: Arc<Mutex<RunState>>,
    /// `INFO:` lines withheld until this dependency finishes, so piped output stays ordered.
    pending: Mutex<Vec<String>>,
    /// The run's outcome lines, printed together when the region comes down.
    results: Arc<Mutex<Vec<String>>>,
}

impl Dependency {
    /// Replaces this dependency's stage.
    fn set(&self, stage: Stage) {
        if let Ok(mut state) = self.state.lock()
            && let Some(slot) = state.stage_mut(self.id)
        {
            *slot = stage;
        }
    }

    /// Says what this dependency is doing, keeping any transfer already in flight.
    pub fn status(&self, message: impl Into<Cow<'static, str>>) {
        let label = message.into();
        if let Ok(mut state) = self.state.lock()
            && let Some(slot) = state.stage_mut(self.id)
        {
            let bytes = match slot {
                Stage::Active { bytes, .. } => *bytes,
                _ => Bytes::default(),
            };
            *slot = Stage::Active { label, bytes };
        }
    }

    /// Says which step of the install is running. Only one dependency commits at a time.
    pub fn committing(&self, message: impl Into<Cow<'static, str>>) {
        self.set(Stage::Committing {
            label: message.into(),
        });
    }

    /// Marks this dependency as downloaded and waiting its turn to install.
    ///
    /// Commits run in config order, so a dependency that finishes early has nothing to do for a
    /// while. It keeps its place in the accounting rather than disappearing.
    pub fn waiting(&self) {
        self.set(Stage::Waiting);
    }

    /// Registers one file transfer, folded into this dependency's totals.
    #[must_use]
    pub fn transfer(&self, total: Option<u64>) -> Transfer<'_> {
        if let Ok(mut state) = self.state.lock()
            && let Some(Stage::Active { bytes, .. }) = state.stage_mut(self.id)
        {
            bytes.active += 1;
            match total {
                Some(total) => bytes.expected += total,
                None => bytes.unmeasured += 1,
            }
        }
        Transfer { dependency: self }
    }

    /// Counts bytes that just arrived, against this dependency and the run.
    fn advance(&self, amount: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.bytes += amount;
            if let Some(Stage::Active { bytes, .. }) = state.stage_mut(self.id) {
                bytes.done += amount;
            }
        }
    }

    /// Notes that one transfer ended, clearing the totals once they all have.
    fn transfer_done(&self) {
        if let Ok(mut state) = self.state.lock()
            && let Some(Stage::Active { bytes, .. }) = state.stage_mut(self.id)
        {
            bytes.active = bytes.active.saturating_sub(1);
            if bytes.active == 0 {
                *bytes = Bytes::default();
            }
        }
    }

    /// Records that a file reached its destination.
    pub fn saved(&self, path: &Path) {
        if self.animated {
            // Just the file name: the row is narrow, and every file of a dependency lands in the
            // same folder anyway.
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            self.status(format!("saved {name}"));
        } else if let Ok(mut pending) = self.pending.lock() {
            pending.push(format!("Saved {}", path.display()));
        }
    }

    /// Ends with a note that nothing needed doing.
    pub fn up_to_date(&self) {
        let name = self.name.clone();
        self.finish(Outcome::Unchanged, "up to date".into(), || {
            ui::info(format!("{name} is up to date"));
        });
    }

    /// Ends with the version now installed.
    pub fn installed(&self, version: &str) {
        let name = self.name.clone();
        self.finish(
            Outcome::Changed,
            format!("installed {version}").into(),
            || {
                ui::success(format!("Installed {name} {version}"));
            },
        );
    }

    /// Ends with the version change.
    pub fn updated(&self, old: &str, new: &str) {
        let name = self.name.clone();
        self.finish(Outcome::Changed, format!("{old} → {new}").into(), || {
            ui::success(format!("Updated {name} from {old} to {new}"));
        });
    }

    /// Ends because the dependency was removed.
    pub fn uninstalled(&self) {
        let name = self.name.clone();
        self.finish(Outcome::Changed, "uninstalled".into(), || {
            ui::success(format!("Uninstalled {name}"));
        });
    }

    /// Ends because the dependency failed, saying briefly why.
    ///
    /// `reason` is a single line - [`VendorError::brief`](crate::VendorError::brief) produces
    /// one. The full error still reaches the user as the command's `ERROR:` line; this is the
    /// row, which used to read only "failed" and so said nothing the mark had not already said.
    pub fn failed(&self, reason: &str) {
        self.finish(Outcome::Failed, reason.to_owned().into(), || {});
    }

    /// Ends without saying anything, for commands whose real output is elsewhere.
    pub fn finish_quietly(&self) {
        self.flush();
        self.set(Stage::Gone);
        self.count();
    }

    /// Flushes withheld lines, in the order they happened.
    fn flush(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            for line in pending.drain(..) {
                ui::info(line);
            }
        }
    }

    /// Counts this dependency towards the summary.
    fn count(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.done += 1;
        }
    }

    /// Records the outcome on this dependency's own row, and keeps the line for the end.
    fn finish(&self, outcome: Outcome, detail: Cow<'static, str>, plain: impl FnOnce()) {
        self.flush();
        if self.animated {
            // Held rather than printed: emitting it now would push the region one row down the
            // screen, once per dependency. `Reporter::end` prints them all as the region comes
            // down, so the scrollback still gets the whole run.
            let (glyph, pad) = outcome.mark();
            // No escape around the failure mark: it is an emoji and paints itself, so a
            // foreground colour would be dead bytes in the scrollback.
            let mark = match outcome {
                Outcome::Changed => ui::green(glyph),
                Outcome::Unchanged => ui::cyan(glyph),
                Outcome::Failed => glyph.to_owned(),
            };
            let line = format!("{mark}{pad} {} {detail}", column(&self.name));
            if let Ok(mut results) = self.results.lock() {
                results.push(line);
            }
            self.set(Stage::Done { outcome, detail });
        } else {
            plain();
            self.set(Stage::Gone);
        }
        self.count();
    }
}

/// Pads a name so outcome lines line up with the rows above them.
fn column(name: &str) -> String {
    let width = view::NAME_WIDTH;
    if name.chars().count() > width {
        let kept: String = name.chars().take(width - 1).collect();
        format!("{kept}…")
    } else {
        format!("{name:<width$}")
    }
}

/// One file's transfer, counted into its dependency's totals.
///
/// Dropping it is what reports that one fewer transfer is running, so the count cannot drift if a
/// download returns early with an error.
#[derive(Debug)]
pub struct Transfer<'a> {
    dependency: &'a Dependency,
}

impl Transfer<'_> {
    /// Counts bytes that just arrived.
    pub fn advance(&self, bytes: u64) {
        self.dependency.advance(bytes);
    }
}

impl Drop for Transfer<'_> {
    fn drop(&mut self) {
        self.dependency.transfer_done();
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTIVE, Command, Reporter, column, print_out, restore_terminal};
    use crate::progress::state::{Bytes, Outcome, RunState, Stage};
    use crate::progress::view::NAME_WIDTH;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Serialises the tests that reach into [`ACTIVE`] directly.
    ///
    /// It is process-wide state, and `Reporter::end` clears it unconditionally: two of these
    /// running at once would steal each other's sender.
    static ACTIVE_GUARD: Mutex<()> = Mutex::new(());

    /// Reads from the run state without holding the lock past the read.
    fn peek<T>(reporter: &Reporter, read: impl FnOnce(&RunState) -> T) -> T {
        read(&reporter.state.lock().expect("state lock"))
    }

    /// The stage of one dependency.
    fn stage_of(reporter: &Reporter, id: usize) -> Stage {
        peek(reporter, |state| state.deps[id].stage.clone())
    }

    /// The transfer totals of a dependency that should be mid-download.
    fn bytes_of(reporter: &Reporter, id: usize) -> Bytes {
        match stage_of(reporter, id) {
            Stage::Active { bytes, .. } => bytes,
            other => panic!("expected an active stage, found {other:?}"),
        }
    }

    #[test]
    fn a_hidden_reporter_animates_nothing() {
        assert!(!Reporter::new(false).animated());
    }

    #[test]
    fn outcome_lines_line_up_with_the_rows_above_them() {
        assert_eq!(column("fzf").chars().count(), NAME_WIDTH);
        assert_eq!(
            column("bitwarden-secrets-cli-x").chars().count(),
            NAME_WIDTH
        );
        assert!(column("bitwarden-secrets-cli-x").ends_with('…'));
    }

    #[test]
    fn a_hidden_dependency_withholds_its_lines_until_it_finishes() {
        let reporter = Reporter::new(false);
        reporter.begin(1);
        let dependency = reporter.dependency("fzf");
        dependency.saved(Path::new("vendor/fzf/LICENSE"));
        dependency.saved(Path::new("vendor/fzf/fzf.exe"));
        dependency.installed("0.38.0");
        reporter.end();
    }

    #[test]
    fn dependencies_are_registered_in_config_order() {
        let reporter = Reporter::new(false);
        reporter.begin(3);
        let first = reporter.dependency("aaa");
        let second = reporter.dependency("bbb");
        let third = reporter.dependency("ccc");
        assert_eq!((first.id, second.id, third.id), (0, 1, 2));
        let names = peek(&reporter, |state| {
            state
                .deps
                .iter()
                .map(|dep| dep.name.clone())
                .collect::<Vec<_>>()
        });
        assert_eq!(names, ["aaa", "bbb", "ccc"]);
    }

    #[test]
    fn concurrent_transfers_share_one_row_and_clear_together() {
        let reporter = Reporter::new(false);
        reporter.begin(1);
        let dependency = reporter.dependency("Coloris");
        dependency.status("downloading");
        let first = dependency.transfer(Some(2048));
        let second = dependency.transfer(Some(1024));
        first.advance(512);
        second.advance(256);
        let bytes = bytes_of(&reporter, 0);
        assert_eq!(bytes.active, 2);
        assert_eq!(bytes.done, 768);
        assert_eq!(bytes.expected, 3072);
        assert_eq!(
            peek(&reporter, |state| state.bytes),
            768,
            "the run total counts every transfer"
        );
        drop(first);
        drop(second);
        assert_eq!(
            bytes_of(&reporter, 0),
            Bytes::default(),
            "totals reset together"
        );
    }

    #[test]
    fn an_unmeasured_transfer_drops_the_percentage() {
        let reporter = Reporter::new(false);
        reporter.begin(1);
        let dependency = reporter.dependency("fzf");
        dependency.status("downloading");
        let measured = dependency.transfer(Some(4096));
        let open_ended = dependency.transfer(None);
        measured.advance(128);
        open_ended.advance(128);
        assert_eq!(bytes_of(&reporter, 0).ratio(), None);
        drop(measured);
        drop(open_ended);
        dependency.up_to_date();
    }

    #[test]
    fn a_dependency_waiting_its_turn_keeps_its_place() {
        let reporter = Reporter::new(false);
        reporter.begin(1);
        let dependency = reporter.dependency("yamlfmt");
        dependency.status("downloading");
        dependency.waiting();
        let stage = stage_of(&reporter, 0);
        assert_eq!(stage, Stage::Waiting);
        assert_eq!(
            stage.priority(),
            Some(2),
            "it still wants a row, behind anything actually working"
        );
    }

    #[test]
    fn finishing_counts_towards_the_summary_and_frees_the_row() {
        // Nothing is animating, so a settled dependency has printed its line already and wants
        // no row at all.
        let reporter = Reporter::new(false);
        reporter.begin(2);
        let first = reporter.dependency("aaa");
        let second = reporter.dependency("bbb");
        first.installed("1.0.0");
        second.finish_quietly();
        let (done, all_settled, rows_free) = peek(&reporter, |state| {
            (
                state.done,
                state.deps.iter().all(|dep| dep.stage == Stage::Gone),
                state.selection().is_empty(),
            )
        });
        assert_eq!(done, 2);
        assert!(all_settled);
        assert!(rows_free);
    }

    #[test]
    fn an_animated_run_reports_in_place_and_keeps_the_lines_for_the_end() {
        // The region must not be pushed down the screen once per dependency, so outcome lines
        // are held rather than printed as they happen - and the dependency reports on its own
        // row instead.
        // No `begin` here on purpose: there is no terminal in a test run, so opening the
        // display would fail and correctly fall back to the plain path.
        let reporter = Reporter::new(true);
        let first = reporter.dependency("aaa");
        let second = reporter.dependency("bbb");
        first.installed("1.0.0");
        second.up_to_date();
        let installed = stage_of(&reporter, 0);
        assert!(
            matches!(
                installed,
                Stage::Done {
                    outcome: Outcome::Changed,
                    ..
                }
            ),
            "{installed:?}"
        );
        let unchanged = stage_of(&reporter, 1);
        assert!(
            matches!(
                unchanged,
                Stage::Done {
                    outcome: Outcome::Unchanged,
                    ..
                }
            ),
            "{unchanged:?}"
        );
        let held = reporter.results.lock().expect("results lock").clone();
        assert_eq!(held.len(), 2, "both lines are waiting for the end");
        assert!(held[0].contains("aaa") && held[0].contains("installed 1.0.0"));
        assert!(held[1].contains("bbb") && held[1].contains("up to date"));
    }

    #[test]
    fn a_failed_dependency_still_releases_what_it_had_done() {
        let reporter = Reporter::new(false);
        reporter.begin(1);
        let dependency = reporter.dependency("micro");
        dependency.saved(Path::new("vendor/micro/LICENSE"));
        dependency.failed("GitHub rejected the credentials (401 Bad credentials).");
    }

    #[test]
    fn a_settled_row_says_why_it_failed_and_lines_up_with_the_rest() {
        let reporter = Reporter::new(true);
        let failed = reporter.dependency("ls-interactive(lsi)");
        failed.failed("GitHub rejected the credentials (401 Bad credentials).");
        let installed = reporter.dependency("fd");
        installed.installed("v10.5.0");

        let held = reporter.results.lock().expect("results lock").clone();
        // The reason, rather than the bare word the row used to carry.
        assert!(
            held[0].contains("GitHub rejected the credentials"),
            "{:?}",
            held[0]
        );
        assert!(!held[0].contains("failed"), "{:?}", held[0]);
        // The failure mark paints itself, so no escape is spent colouring it; the others get one.
        assert!(held[0].contains('\u{274c}'), "{:?}", held[0]);
        assert!(!held[0].contains('\u{1b}'), "{:?}", held[0]);
        assert!(held[1].contains('\u{1b}'), "{:?}", held[1]);
    }

    #[test]
    fn restoring_the_terminal_is_safe_with_nothing_to_restore() {
        let _guard = ACTIVE_GUARD.lock().expect("active guard");
        // The interrupt handler runs whatever the run was doing, including before a display
        // exists and after one has already been closed.
        restore_terminal();
        let reporter = Reporter::new(false);
        reporter.begin(1);
        reporter.end();
        restore_terminal();
    }

    #[test]
    fn restoring_the_terminal_stops_an_active_display() {
        // A display cannot be started here - it needs a terminal that answers a cursor query -
        // so the render thread is stood in for by a channel of its own.
        let _guard = ACTIVE_GUARD.lock().expect("active guard");
        let (sender, inbox) = std::sync::mpsc::channel();
        *ACTIVE.lock().expect("active") = Some(sender);

        restore_terminal();

        // A `Print` from work still in flight may arrive first; `Stop` is what has to arrive.
        let stopped = std::iter::from_fn(|| inbox.recv_timeout(Duration::from_secs(1)).ok())
            .any(|command| matches!(command, Command::Stop));
        assert!(stopped, "the display was never asked to stop");
        assert!(
            ACTIVE.lock().expect("active").is_none(),
            "a stopped display is still taking print traffic"
        );
    }

    #[test]
    fn printing_without_an_active_display_still_prints() {
        print_out("a line that has nowhere else to go");
    }
}
