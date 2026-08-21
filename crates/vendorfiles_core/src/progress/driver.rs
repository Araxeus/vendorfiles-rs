//! The render thread.
//!
//! One thread owns the terminal and nothing else touches it. Everything else mutates the shared
//! [`RunState`] and moves on, so a download reporting bytes thousands of times a second costs a
//! lock rather than a repaint. The thread redraws on a tick, which is what makes the frame rate
//! independent of how fast work arrives.
//!
//! It is a plain OS thread rather than a tokio task so the spinner keeps turning while the
//! runtime is saturated, and so any thread can report progress without an executor to hand.

use std::io::{self, IsTerminal, Stdout, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{MoveTo, Show};
use ratatui::crossterm::execute;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{TerminalOptions, Viewport};

use super::state::RunState;
use super::view;

/// How often the display redraws, and so how fast a spinner turns.
const TICK: Duration = Duration::from_millis(80);

/// The terminal we draw to.
///
/// Stdout, not stderr: crossterm's cursor query is hard-wired to stdout, so an inline viewport
/// backed by stderr fails to anchor and leaks its query into the data stream. See the design
/// note in `docs/superpowers/specs`.
type Term = Terminal<CrosstermBackend<Stdout>>;

/// What the render thread can be asked to do.
pub enum Command {
    /// Put these lines above the region, permanently.
    Print(Vec<Line<'static>>),
    /// Change how many worker rows the region has, re-anchoring it.
    Resize(usize),
    /// Wipe the region and let the terminal go.
    Stop,
}

/// A handle to the render thread.
#[derive(Debug)]
pub struct Driver {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Driver {
    /// Opens the terminal and starts drawing, or gives up and returns `None`.
    ///
    /// Failure is not an error: a terminal that will not report its cursor position simply gets
    /// the plain output instead of a live region.
    pub fn start(state: Arc<Mutex<RunState>>) -> Option<Self> {
        install_panic_hook();
        let terminal = open(0)?;
        let (commands, inbox) = channel();
        let thread = thread::Builder::new()
            .name("vendorfiles-display".to_owned())
            .spawn(move || run(terminal, &state, &inbox))
            .ok()?;
        Some(Self {
            commands,
            thread: Some(thread),
        })
    }

    /// A sender for code that only needs to print, not to own the display.
    pub fn commands(&self) -> Sender<Command> {
        self.commands.clone()
    }

    /// Asks the thread to do something, ignoring a thread that has already gone.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Stops drawing and restores the terminal.
    ///
    /// `Stop` queues behind every pending `Print`, so joining the thread is also the flush.
    pub fn stop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Builds a terminal whose viewport is the region, and nothing else.
///
/// No raw mode, no alternate screen, no input: `vendor` is not interactive, and its output has to
/// survive in scrollback and in CI logs.
fn open(rows: usize) -> Option<Term> {
    // Anchoring an inline viewport asks the terminal where the cursor is, and crossterm sends
    // that query to stdout. If stdout is not a terminal, nothing answers and the query itself
    // ends up in whatever is reading the pipe.
    if !io::stdout().is_terminal() {
        return None;
    }
    Terminal::with_options(
        CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Inline(view::height(rows)),
        },
    )
    .ok()
}

/// The most worker rows that will fit, leaving the terminal room to scroll.
///
/// A region taller than the screen cannot be repainted in place, which is how the previous
/// display came apart. Better to show fewer rows than to show a broken frame.
#[must_use]
pub fn fit(rows: usize) -> usize {
    if rows == 0 {
        return 0;
    }
    let Ok((_, lines)) = ratatui::crossterm::terminal::size() else {
        return rows;
    };
    rows.min(view::rows_that_fit(lines).max(1))
}

/// The thread body: draw on a tick, print on request, tear down on `Stop`.
fn run(mut terminal: Term, state: &Mutex<RunState>, inbox: &Receiver<Command>) {
    let mut tick = 0_usize;
    let mut rows = 0_usize;
    loop {
        match inbox.recv_timeout(TICK) {
            Ok(Command::Print(lines)) => {
                let height = u16::try_from(lines.len()).unwrap_or(1).max(1);
                let _ = terminal.insert_before(height, move |buf| {
                    Paragraph::new(lines).render(buf.area, buf);
                });
            }
            Ok(Command::Resize(wanted)) => {
                if wanted != rows {
                    rows = wanted;
                    if let Some(replacement) = reopen(&mut terminal, rows) {
                        terminal = replacement;
                    }
                }
            }
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => tick = tick.wrapping_add(1),
        }
        draw(&mut terminal, state, tick);
    }
    close(&mut terminal);
}

/// Re-anchors the region at a new height.
///
/// The viewport is anchored to a stored rectangle, so it cannot simply be made taller: the old
/// one is wiped and a new terminal re-queries the cursor. Rebuilding is the only mechanism that
/// was found to re-anchor reliably.
fn reopen(terminal: &mut Term, rows: usize) -> Option<Term> {
    wipe(terminal);
    open(rows)
}

/// Erases the region and puts the cursor back at its first row.
///
/// `Terminal::clear` erases the viewport but leaves the cursor at the *bottom* of it. Anything
/// written next therefore starts below a band of rows that have just been blanked — which is
/// where the gap above the region on resize, and the gap below it after teardown, both came from.
fn wipe(terminal: &mut Term) {
    // The viewport's own position on screen, which is what the cursor has to go back to.
    let top = terminal.get_frame().area().y;
    let _ = terminal.clear();
    let _ = execute!(io::stdout(), MoveTo(0, top));
}

/// Paints one frame from the current state.
fn draw(terminal: &mut Term, state: &Mutex<RunState>, tick: usize) {
    // A poisoned lock means some other thread panicked mid-update. Skipping the frame is better
    // than joining it.
    let Ok(mut state) = state.lock() else {
        return;
    };
    // Hand out rows before drawing, so a dependency keeps the row it already had.
    state.assign();
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        view::view(&state, tick, area, frame.buffer_mut());
    });
}

/// Wipes the region and gives the cursor back, so the next prompt starts where it began.
fn close(terminal: &mut Term) {
    wipe(terminal);
    let mut out = io::stdout();
    // `draw` hides the cursor, so leaving without showing it again would cost the user their
    // prompt cursor for the rest of the session.
    let _ = execute!(out, Show);
    let _ = out.flush();
}

/// Guards against installing the panic hook more than once.
static HOOK: OnceLock<()> = OnceLock::new();

/// Makes sure a panic leaves a usable terminal behind.
///
/// `ratatui::init` installs a hook of its own, but it also enables raw mode and so is not what
/// this uses; nothing else will do it for us.
fn install_panic_hook() {
    HOOK.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut out = io::stdout();
            let _ = execute!(out, Show);
            let _ = out.flush();
            previous(info);
        }));
    });
}
