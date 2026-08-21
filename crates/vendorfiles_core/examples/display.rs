//! Drives the progress display through a simulated sync, for looking at it.
//!
//! `cargo run --example display [dependencies]`
//!
//! The display is the one part of the tool that cannot be judged from a unit test alone — a frame
//! can be asserted, but not whether the region tears, re-anchors, or leaves the cursor behind. So
//! this runs a plausible sync without touching the network: many dependencies, eight downloading
//! at a time, an ordered commit pass that makes the rest wait, and log lines printed above.
//!
//! Everything is deterministic, so two runs can be compared.

use std::time::Duration;

use vendorfiles_core::progress::{Dependency, Reporter, Transfer};
use vendorfiles_core::ui;

/// How many may download at once, matching `ops::sync`.
const CONCURRENCY: usize = 8;

/// What one simulated dependency is doing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Queued,
    Downloading,
    Waiting,
    Installed,
}

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(18);

    let names: Vec<String> = (0..count).map(name).collect();
    let sizes: Vec<u64> = (0..count)
        .map(|index| 200 * 1024 + noise(index as u64) % (6 * 1024 * 1024))
        .collect();

    let reporter = Reporter::new(true);
    reporter.begin(count);

    // Version resolution: the summary bar alone, before the run knows how much work there is.
    std::thread::sleep(Duration::from_millis(600));
    reporter.summary("installing");

    let deps: Vec<Dependency> = names.iter().map(|name| reporter.dependency(name)).collect();
    reporter.reserve_rows(CONCURRENCY.min(count));

    let mut transfers: Vec<Option<Transfer<'_>>> = deps.iter().map(|_| None).collect();
    let mut phases = vec![Phase::Queued; count];
    let mut received = vec![0_u64; count];
    let mut next = 0_usize;
    let mut committed = 0_usize;
    let mut step = 0_u64;

    while committed < count {
        // Start downloads up to the concurrency limit.
        let running = phases.iter().filter(|p| **p == Phase::Downloading).count();
        for _ in running..CONCURRENCY {
            if next >= count {
                break;
            }
            deps[next].status("downloading");
            transfers[next] = Some(deps[next].transfer(Some(sizes[next])));
            phases[next] = Phase::Downloading;
            next += 1;
        }

        // Move bytes.
        for index in 0..count {
            if phases[index] != Phase::Downloading {
                continue;
            }
            let chunk = (64 * 1024 + noise(step * 31 + index as u64) % (512 * 1024))
                .min(sizes[index] - received[index]);
            received[index] += chunk;
            if let Some(transfer) = transfers[index].as_ref() {
                transfer.advance(chunk);
            }
            if received[index] >= sizes[index] {
                transfers[index] = None;
                deps[index].saved(std::path::Path::new("vendor/example/file.bin"));
                deps[index].waiting();
                phases[index] = Phase::Waiting;
            }
        }

        // Commit in config order, which is what makes later dependencies wait.
        while committed < count && phases[committed] == Phase::Waiting {
            deps[committed].committing("writing lockfile");
            std::thread::sleep(Duration::from_millis(120));
            if committed % 5 == 3 {
                deps[committed].up_to_date();
            } else {
                deps[committed].installed(&format!("v1.{committed}.0"));
            }
            phases[committed] = Phase::Installed;
            committed += 1;
        }

        // A warning mid-run: it has to appear above the region, not inside it.
        if step == 12 {
            ui::warning("a mid-run warning that must land above the region");
        }
        step += 1;
        std::thread::sleep(Duration::from_millis(40));
    }

    reporter.end();
    println!("done: {count} dependencies");
}

/// A plausible dependency name for a given index.
fn name(index: usize) -> String {
    const NAMES: [&str; 9] = [
        "fzf",
        "micro",
        "yamlfmt",
        "shfmt",
        "Coloris",
        "ls-interactive",
        "wsl-notify-send",
        "bitwarden-secrets-cli",
        "ripgrep",
    ];
    let base = NAMES[index % NAMES.len()];
    if index < NAMES.len() {
        base.to_owned()
    } else {
        format!("{base}-{}", index / NAMES.len())
    }
}

/// A deterministic stand-in for randomness, so runs can be compared.
const fn noise(seed: u64) -> u64 {
    seed.wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
        >> 16
}
