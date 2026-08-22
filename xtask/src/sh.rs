//! Running child processes on behalf of a task.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// The workspace root, which is this crate's parent.
pub fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .context("locating the workspace root")
}

/// Runs a command under a spinner, so a slow step (`cargo check`) never looks like a hang.
///
/// The child's output is captured rather than inherited - otherwise it would fight the spinner
/// for the same lines - and is replayed only when the command fails.
pub fn run(root: &Path, label: &str, program: &str, args: &[&str]) -> Result<()> {
    let spinner = Spinner::start(label);

    let started = Instant::now();
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("running {program} {}", args.join(" ")));
    spinner.stop();

    let output = output?;
    if !output.status.success() {
        bail!(
            "{program} {} failed:\n{}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("✔ {label} ({:.1?})", started.elapsed());
    Ok(())
}

/// A one-line spinner on stderr, kept turning by its own thread.
///
/// Small enough to own outright: tasks show one at a time, on a line of its own, so none of the
/// machinery the tool's own display needs applies here.
struct Spinner {
    running: Arc<AtomicBool>,
    /// `None` when there is nothing to animate to, so a redirected run stays readable.
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    /// Starts spinning until [`Spinner::stop`].
    fn start(label: &str) -> Self {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let running = Arc::new(AtomicBool::new(true));
        // Redirected: a spinner would fill the log with one line per frame.
        if !std::io::stderr().is_terminal() {
            return Self {
                running,
                thread: None,
            };
        }
        let flag = Arc::clone(&running);
        let label = label.to_owned();
        let started = Instant::now();
        let thread = std::thread::spawn(move || {
            let mut frame = 0_usize;
            while flag.load(Ordering::Relaxed) {
                let mut err = std::io::stderr();
                let _ = write!(
                    err,
                    "\r\x1b[2K\x1b[36m{}\x1b[0m {label} {:.1?}",
                    FRAMES[frame % FRAMES.len()],
                    started.elapsed()
                );
                let _ = err.flush();
                frame += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        Self {
            running,
            thread: Some(thread),
        }
    }

    /// Stops the thread and wipes the line.
    fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        let Some(thread) = self.thread.take() else {
            return;
        };
        let _ = thread.join();
        let mut err = std::io::stderr();
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    }
}
