//! Drawing the live region.
//!
//! [`view`] is a pure function of a [`RunState`]: one snapshot in, one frame out. That is the
//! point of it. The old display computed its output while also moving the cursor, so the only way
//! to check it was to run it under a pty and count escape sequences; this can be asserted
//! cell-by-cell in a unit test.
//!
//! The region's height is fixed for the whole run by [`height`], and every line is built to the
//! width it was given. Nothing here can wrap, so nothing here can push the frame out of shape.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph, Widget};

use super::state::{MARK_WIDTH, Outcome, RunState, Stage};

/// The frames a spinner cycles through.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Column the status text starts at, so names do not shuffle between rows.
pub const NAME_WIDTH: usize = 20;

/// Rows of chrome around the worker rows: two borders, the summary, the rule and the footer.
const CHROME: u16 = 5;

/// Width below which the byte counts are dropped.
const NARROW: usize = 52;

/// Width below which the percentage is dropped too.
const CRAMPED: usize = 38;

/// The narrowest bar worth drawing.
const MIN_BAR: usize = 4;

/// The widest a worker row's bar grows.
///
/// Capped, not stretched: across a wide terminal a full-width bar leaves the percentage and the
/// byte counts a screen away from the name they belong to.
const BAR_MAX: usize = 28;

/// The widest the summary bar grows, for the same reason.
const SUMMARY_BAR: usize = 24;

/// A row that is only waiting gets a static mark, not a spinner - eight spinners for work that
/// is not moving reads as far busier than the run actually is.
const IDLE_MARK: &str = "·";

/// The widest the region grows, however wide the terminal is.
///
/// Enough for a name, a capped bar, a percentage and the byte counts. Stretching the frame across
/// a very wide terminal leaves its content huddled in the left third of an otherwise empty box.
const REGION_WIDTH: u16 = 84;

/// How tall the region is for a given number of worker rows.
///
/// A run with no rows to show - `outdated`, or a project already up to date - has no bytes to
/// report either, so it loses the rule and the footer and sits in three lines.
#[must_use]
pub const fn height(rows: usize) -> u16 {
    if rows == 0 {
        3
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "rows is capped at MAX_CONCURRENT_DOWNLOADS and by the terminal height"
        )]
        let rows = rows as u16;
        CHROME.saturating_add(rows)
    }
}

/// The most worker rows whose region still fits a terminal `lines` tall.
///
/// Two lines are left spare: one for the line the region sits under, one to scroll into. A
/// region taller than the screen cannot be repainted in place, which is how the previous
/// display came apart, so fewer rows is always the better trade.
#[must_use]
pub const fn rows_that_fit(lines: u16) -> usize {
    // `height` is CHROME plus one line per row.
    lines.saturating_sub(CHROME + 2) as usize
}

/// Draws the whole region.
pub fn view(state: &RunState, tick: usize, area: Rect, buf: &mut Buffer) {
    if area.height < 3 || area.width < 10 {
        return;
    }
    let area = Rect {
        width: area.width.min(REGION_WIDTH),
        ..area
    };
    let border = Style::new().dim();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Span::styled("─ vendorfiles ", Style::new().fg(Color::Cyan)));
    let inner = block.inner(area);
    block.render(area, buf);

    let width = inner.width as usize;
    // A footer needs the summary, at least one worker row, the rule and itself.
    let footer = inner.height >= 4;
    let capacity = if footer { inner.height as usize - 3 } else { 0 };

    let mut lines = Vec::with_capacity(inner.height as usize);
    lines.push(summary_line(state, width));
    for slot in state.selection().iter().take(capacity) {
        // An empty row stays empty rather than closing up: the row below it must not move
        // because this one's dependency finished.
        let line = slot
            .and_then(|id| state.deps.get(id))
            .map_or_else(Line::default, |dep| {
                worker_line(&dep.name, &dep.stage, tick, width)
            });
        lines.push(line);
    }
    // Blank rows hold the height once the work drains, rather than letting the region shrink.
    while lines.len() < 1 + capacity {
        lines.push(Line::default());
    }
    if footer {
        lines.push(Line::styled("─".repeat(width), border));
        lines.push(stats_line(state, width));
    }
    Paragraph::new(lines).render(inner, buf);

    if footer {
        // Join the rule to the frame, which `Paragraph` cannot do since it draws inside it.
        let y = inner.y + 1 + u16::try_from(capacity).unwrap_or(u16::MAX);
        for (x, symbol) in [(area.left(), "├"), (area.right().saturating_sub(1), "┤")] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(symbol);
            }
        }
    }
}

/// `  4/18  ━━━━━━━━╴──────  installing`
fn summary_line(state: &RunState, width: usize) -> Line<'static> {
    let counter = format!("{:>3}/{:<3} ", state.done, state.total);
    let phase = format!("  {}", state.phase);
    let used = counter.chars().count();
    let room = width.saturating_sub(used + phase.chars().count());
    let bar_width = SUMMARY_BAR.min(room).max(MIN_BAR);
    let (filled, rest) = bar(bar_width, state.ratio());
    Line::from(vec![
        Span::styled(counter, Style::new().bold()),
        Span::styled(filled, Style::new().fg(Color::Green)),
        Span::styled(rest, Style::new().dim()),
        Span::raw(clip(&phase, width.saturating_sub(used + bar_width))),
    ])
}

/// `⠹ fzf                ━━━━━━━━╴───   61%  1.2/2.0 MiB`
fn worker_line(name: &str, stage: &Stage, tick: usize, width: usize) -> Line<'static> {
    let mark = match stage {
        Stage::Waiting => Span::styled(IDLE_MARK, Style::new().dim()),
        Stage::Done { outcome, .. } => {
            let (glyph, _) = outcome.mark();
            // The failure mark is an emoji: it has its own colour and ignores a foreground one.
            colour_of(*outcome).map_or_else(
                || Span::raw(glyph),
                |colour| Span::styled(glyph, Style::new().fg(colour)),
            )
        }
        // Only work that is actually moving gets a spinner.
        _ => Span::styled(SPINNER[tick % SPINNER.len()], Style::new().fg(Color::Cyan)),
    };
    // Padded to a fixed width because the marks are not all one cell wide, and a name that
    // shifted a column when its dependency failed would be worse than either.
    let pad = match stage {
        Stage::Done { outcome, .. } => outcome.mark().1,
        _ => " ",
    };
    let mut spans = vec![
        mark,
        Span::raw(pad),
        Span::raw(" "),
        Span::styled(column(name), Style::new().bold()),
        Span::raw(" "),
    ];
    let used = MARK_WIDTH + 1 + NAME_WIDTH + 1;
    let rest = width.saturating_sub(used);

    match stage {
        Stage::Active { bytes, .. } if bytes.transferring() && bytes.ratio().is_some() => {
            let show_bytes = width >= NARROW;
            let show_percent = width >= CRAMPED;
            let counts = if show_bytes {
                format!("  {}", transferred(bytes.done, bytes.expected))
            } else {
                String::new()
            };
            let ratio = bytes.ratio().unwrap_or(0.0);
            let percent = if show_percent {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "a clamped ratio times 100 is between 0 and 100"
                )]
                let whole = (ratio * 100.0).round() as u16;
                format!(" {whole:>3}%")
            } else {
                String::new()
            };
            let bar_width = BAR_MAX
                .min(rest.saturating_sub(counts.chars().count() + percent.chars().count()))
                .max(MIN_BAR);
            let (filled, unfilled) = bar(bar_width, ratio);
            spans.push(Span::styled(filled, Style::new().fg(Color::Cyan)));
            spans.push(Span::styled(unfilled, Style::new().dim()));
            spans.push(Span::raw(percent));
            spans.push(Span::styled(counts, Style::new().fg(Color::DarkGray)));
        }
        Stage::Active { label, bytes } => {
            // No total to divide by: report what has arrived instead of a percentage.
            let text = if bytes.done > 0 {
                format!("{label} {}", human(bytes.done))
            } else {
                label.to_string()
            };
            spans.push(Span::raw(clip(&text, rest)));
        }
        Stage::Committing { label } => {
            spans.push(Span::styled(
                clip(label, rest),
                Style::new().fg(Color::Green),
            ));
        }
        Stage::Waiting => {
            spans.push(Span::styled(
                clip("waiting to install", rest),
                Style::new().dim(),
            ));
        }
        Stage::Done { outcome, detail } => {
            // Red on the text even where the mark supplies its own, so a failed row reads as one
            // at a glance and a long reason still gets clipped to the room available.
            let colour = colour_of(*outcome).unwrap_or(Color::Red);
            spans.push(Span::styled(clip(detail, rest), Style::new().fg(colour)));
        }
        Stage::Queued | Stage::Gone => {}
    }
    Line::from(spans)
}

/// The colour a settled outcome is drawn in, or `None` when its mark supplies its own.
const fn colour_of(outcome: Outcome) -> Option<Color> {
    match outcome {
        Outcome::Changed => Some(Color::Green),
        Outcome::Unchanged => Some(Color::Cyan),
        Outcome::Failed => None,
    }
}

/// `12.4 MiB · 3.1 MiB/s · 4.2s · 3 waiting · 11 queued`
fn stats_line(state: &RunState, width: usize) -> Line<'static> {
    let mut parts = Vec::new();
    if state.bytes > 0 {
        parts.push(human(state.bytes));
    }
    if let Some(rate) = state.rate() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a throughput in bytes per second, floored for display"
        )]
        let rate = rate as u64;
        parts.push(format!("{}/s", human(rate)));
    }
    parts.push(format!("{:.1}s", state.started.elapsed().as_secs_f64()));
    let counts = state.counts();
    if counts.waiting > 0 {
        parts.push(format!("{} waiting", counts.waiting));
    }
    if counts.queued > 0 {
        parts.push(format!("{} queued", counts.queued));
    }
    Line::styled(clip(&parts.join(" · "), width), Style::new().dim())
}

/// The filled and unfilled halves of a bar, so each can be styled on its own.
fn bar(width: usize, ratio: f64) -> (String, String) {
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a cell count for a bar at most a terminal wide"
    )]
    let filled = ((ratio * width as f64).round() as usize).min(width);
    let mut remaining = width - filled;
    let mut unfilled = String::new();
    // A head on the unfilled side reads as "in progress" rather than "stopped here".
    if remaining > 0 && filled > 0 {
        unfilled.push('╴');
        remaining -= 1;
    }
    unfilled.push_str(&"─".repeat(remaining));
    ("━".repeat(filled), unfilled)
}

/// Pads a name so statuses line up, and truncates one long enough to threaten the width.
fn column(name: &str) -> String {
    if name.chars().count() > NAME_WIDTH {
        let kept: String = name.chars().take(NAME_WIDTH - 1).collect();
        format!("{kept}…")
    } else {
        format!("{name:<NAME_WIDTH$}")
    }
}

/// Cuts text to fit, since a line that overflows would wrap and break the frame.
fn clip(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Binary units, the same ones the reference tool's progress output used.
const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

/// The divisor and unit that keep a byte count under four digits.
fn unit_for(bytes: u64) -> (f64, &'static str) {
    let mut divisor = 1.0_f64;
    let mut index = 0;
    #[expect(
        clippy::cast_precision_loss,
        reason = "choosing a display unit; a rounding error cannot pick the wrong one"
    )]
    let value = bytes as f64;
    while value / divisor >= 1024.0 && index + 1 < UNITS.len() {
        divisor *= 1024.0;
        index += 1;
    }
    (divisor, UNITS[index])
}

/// `4.2 MiB`, or `512 B` where a decimal would be noise.
fn human(bytes: u64) -> String {
    let (divisor, unit) = unit_for(bytes);
    if unit == "B" {
        return format!("{bytes} B");
    }
    #[expect(clippy::cast_precision_loss, reason = "displayed to one decimal place")]
    let value = bytes as f64 / divisor;
    format!("{value:.1} {unit}")
}

/// `1.2/2.0 MiB` - both figures in the larger one's unit, so they can be compared at a glance.
fn transferred(done: u64, expected: u64) -> String {
    let (divisor, unit) = unit_for(expected.max(done));
    if unit == "B" {
        return format!("{done}/{expected} B");
    }
    #[expect(clippy::cast_precision_loss, reason = "displayed to one decimal place")]
    let (done, expected) = (done as f64 / divisor, expected as f64 / divisor);
    format!("{done:.1}/{expected:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::{
        BAR_MAX, NAME_WIDTH, SPINNER, bar, clip, height, human, rows_that_fit, transferred, view,
    };
    use crate::progress::state::{Bytes, Outcome, RunState, Stage};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::borrow::Cow;

    /// The rendered text of each row, which is what a reader of the terminal actually sees.
    fn rows(buf: &Buffer) -> Vec<String> {
        (buf.area.top()..buf.area.bottom())
            .map(|y| {
                (buf.area.left()..buf.area.right())
                    .filter_map(|x| buf.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn render(state: &RunState, width: u16) -> Vec<String> {
        let area = Rect::new(0, 0, width, height(state.rows));
        let mut buf = Buffer::empty(area);
        view(state, 2, area, &mut buf);
        rows(&buf)
    }

    fn with(stages: Vec<Stage>, rows: usize) -> RunState {
        let mut state = RunState {
            total: stages.len(),
            phase: Cow::Borrowed("installing"),
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

    #[test]
    fn the_height_is_three_when_there_are_no_worker_rows() {
        assert_eq!(height(0), 3);
        assert_eq!(height(1), 6);
        assert_eq!(height(8), 13);
    }

    #[test]
    fn the_region_never_asks_for_more_lines_than_the_terminal_has() {
        for lines in 3_u16..40 {
            let rows = rows_that_fit(lines);
            if rows > 0 {
                assert!(
                    height(rows) + 2 <= lines,
                    "{rows} rows need {} of {lines} lines",
                    height(rows)
                );
            }
        }
        // A terminal with no room at all asks for no rows, and the caller floors it at one.
        assert_eq!(rows_that_fit(6), 0);
        assert_eq!(rows_that_fit(24), 17);
    }

    #[test]
    fn a_run_with_no_downloads_draws_a_three_line_box() {
        let mut state = with(vec![Stage::Queued, Stage::Queued], 0);
        state.done = 2;
        state.phase = Cow::Borrowed("checking for updates");
        let frame = render(&state, 60);
        assert_eq!(frame.len(), 3);
        assert!(frame[0].starts_with("╭─ vendorfiles ─"), "{:?}", frame[0]);
        assert!(frame[1].contains("2/2"), "{:?}", frame[1]);
        assert!(frame[1].contains("checking for updates"), "{:?}", frame[1]);
        assert!(frame[2].starts_with('╰'), "{:?}", frame[2]);
        // No bytes moved, so no rule and no footer.
        assert!(!frame.iter().any(|line| line.contains('├')));
    }

    #[test]
    fn a_transferring_dependency_shows_a_bar_a_percentage_and_its_bytes() {
        let state = with(
            vec![Stage::Active {
                label: Cow::Borrowed("downloading"),
                bytes: Bytes {
                    active: 1,
                    done: 1_258_291,
                    expected: 2_097_152,
                    unmeasured: 0,
                },
            }],
            1,
        );
        let frame = render(&state, 72);
        let row = &frame[2];
        assert!(row.contains("dep-0"), "{row:?}");
        assert!(row.contains('━') && row.contains('─'), "{row:?}");
        assert!(row.contains("60%"), "{row:?}");
        assert!(row.contains("1.2/2.0 MiB"), "{row:?}");
    }

    #[test]
    fn a_transfer_of_unknown_length_reports_bytes_instead_of_a_percentage() {
        let state = with(
            vec![Stage::Active {
                label: Cow::Borrowed("downloading"),
                bytes: Bytes {
                    active: 1,
                    done: 4096,
                    expected: 0,
                    unmeasured: 1,
                },
            }],
            1,
        );
        let frame = render(&state, 72);
        assert!(!frame[2].contains('%'), "{:?}", frame[2]);
        assert!(frame[2].contains("4.0 KiB"), "{:?}", frame[2]);
    }

    #[test]
    fn every_line_fits_the_width_exactly_so_nothing_can_wrap() {
        let state = with(
            vec![
                Stage::Active {
                    label: Cow::Borrowed("downloading"),
                    bytes: Bytes {
                        active: 2,
                        done: 900_000_000,
                        expected: 1_000_000_000,
                        unmeasured: 0,
                    },
                },
                Stage::Waiting,
                Stage::Committing {
                    label: Cow::Borrowed("writing lockfile"),
                },
            ],
            3,
        );
        for width in [30_u16, 38, 52, 60, 80, 120, 200] {
            let area = Rect::new(0, 0, width, height(state.rows));
            let mut buf = Buffer::empty(area);
            view(&state, 0, area, &mut buf);
            for (index, line) in rows(&buf).iter().enumerate() {
                assert!(
                    line.chars().count() <= width as usize,
                    "width {width}, line {index}: {line:?}"
                );
            }
        }
    }

    #[test]
    fn the_rule_joins_the_frame() {
        let state = with(vec![Stage::Waiting], 1);
        let frame = render(&state, 40);
        let rule = frame.iter().find(|line| line.starts_with('├')).unwrap();
        assert!(rule.ends_with('┤'), "{rule:?}");
    }

    #[test]
    fn the_footer_reports_what_did_not_fit() {
        let mut state = with(
            vec![Stage::Waiting, Stage::Waiting, Stage::Queued, Stage::Queued],
            1,
        );
        state.bytes = 12_000_000;
        let frame = render(&state, 70);
        let footer = frame.last().map(String::as_str).unwrap_or_default();
        let counts = &frame[frame.len() - 2];
        assert!(counts.contains("2 waiting"), "{counts:?}");
        assert!(counts.contains("2 queued"), "{counts:?}");
        assert!(counts.contains("11.4 MiB"), "{counts:?}");
        assert!(footer.starts_with('╰'), "{footer:?}");
    }

    #[test]
    fn a_vacated_row_stays_blank_instead_of_the_rows_below_moving_up() {
        // The renderer must not compact. If the middle dependency finishes, the one under it
        // has to stay where it is - that sliding is what made the display look like it was
        // jumping around.
        let mut state = with(vec![Stage::Waiting, Stage::Waiting, Stage::Waiting], 3);
        assert_eq!(state.selection(), [Some(0), Some(1), Some(2)]);
        *state.stage_mut(1).unwrap() = Stage::Gone;
        state.assign();

        let frame = render(&state, 60);
        assert!(frame[2].contains("dep-0"), "{:?}", frame[2]);
        // Only the frame's own borders survive on a vacated row.
        assert_eq!(
            frame[3].replace([' ', '│'], ""),
            "",
            "the vacated row must be blank: {:?}",
            frame[3]
        );
        assert!(
            frame[4].contains("dep-2"),
            "dep-2 must not have moved up: {:?}",
            frame[4]
        );
    }

    #[test]
    fn a_finished_dependency_reports_its_outcome_on_its_own_row() {
        let state = with(
            vec![
                Stage::Done {
                    outcome: Outcome::Changed,
                    detail: Cow::Borrowed("installed v1.7.0"),
                },
                Stage::Done {
                    outcome: Outcome::Unchanged,
                    detail: Cow::Borrowed("up to date"),
                },
                Stage::Done {
                    outcome: Outcome::Failed,
                    detail: Cow::Borrowed("GitHub rejected the credentials (401)"),
                },
            ],
            3,
        );
        let frame = render(&state, 60);
        assert!(
            frame[2].contains("✔") && frame[2].contains("installed v1.7.0"),
            "{:?}",
            frame[2]
        );
        assert!(
            frame[3].contains("·") && frame[3].contains("up to date"),
            "{:?}",
            frame[3]
        );
        assert!(
            frame[4].contains("❌") && frame[4].contains("GitHub rejected the credentials"),
            "{:?}",
            frame[4]
        );
        // The failure mark is two cells wide and the other two are one, so the mark field is
        // padded: without that, a failed row's name would sit a column right of every other.
        let column_of = |line: &str, name: &str| line.find(name).expect("the name is on the row");
        assert_eq!(
            column_of(&frame[2], "dep-0"),
            column_of(&frame[4], "dep-2"),
            "names must line up whatever the mark:\n{:?}\n{:?}",
            frame[2],
            frame[4]
        );
        // Nothing here is working, so nothing here spins.
        assert!(
            !frame[2..5]
                .iter()
                .any(|row| SPINNER.iter().any(|glyph| row.contains(glyph))),
            "{:?}",
            &frame[2..5]
        );
    }

    #[test]
    fn a_waiting_row_gets_a_static_mark_rather_than_a_spinner() {
        let state = with(vec![Stage::Waiting], 1);
        let frame = render(&state, 60);
        assert!(frame[2].contains('·'), "{:?}", frame[2]);
        assert!(
            !SPINNER
                .iter()
                .any(|frame_char| frame[2].contains(frame_char)),
            "nothing is moving, so nothing should spin: {:?}",
            frame[2]
        );
    }

    #[test]
    fn bars_are_capped_so_a_wide_terminal_keeps_its_columns_together() {
        let state = with(
            vec![Stage::Active {
                label: Cow::Borrowed("downloading"),
                bytes: Bytes {
                    active: 1,
                    done: 512,
                    expected: 1024,
                    unmeasured: 0,
                },
            }],
            1,
        );
        // On a very wide terminal the bar must not stretch away from its own numbers.
        let frame = render(&state, 200);
        let row = &frame[2];
        let bar_end = row.rfind('─').or_else(|| row.rfind('━')).unwrap();
        let percent = row.find('%').unwrap();
        assert!(
            percent - bar_end < 8,
            "the percentage drifted away from the bar: {row:?}"
        );
        assert!(
            row.matches('━').count() + row.matches('─').count() + row.matches('╴').count()
                <= BAR_MAX,
            "the bar exceeded its cap: {row:?}"
        );
    }

    #[test]
    fn blank_rows_hold_the_height_when_the_work_drains() {
        // Four rows were reserved and only one dependency is left in flight.
        let state = with(vec![Stage::Waiting, Stage::Gone, Stage::Gone], 4);
        let frame = render(&state, 60);
        assert_eq!(frame.len(), usize::from(height(4)));
    }

    #[test]
    fn a_name_too_long_for_the_column_is_truncated_not_wrapped() {
        let mut state = with(vec![Stage::Waiting], 1);
        state.deps[0].name = "a-really-quite-long-dependency-name".to_owned();
        let frame = render(&state, 60);
        assert!(frame[2].contains('…'), "{:?}", frame[2]);
        assert!(frame[2].contains("waiting to install"), "{:?}", frame[2]);
    }

    #[test]
    fn a_bar_is_exactly_the_width_it_was_given() {
        for ratio in [0.0, 0.01, 0.5, 0.999, 1.0] {
            let (filled, unfilled) = bar(20, ratio);
            assert_eq!(
                filled.chars().count() + unfilled.chars().count(),
                20,
                "ratio {ratio}"
            );
        }
    }

    #[test]
    fn byte_counts_read_the_way_a_person_would_write_them() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(4096), "4.0 KiB");
        assert_eq!(human(4_404_019), "4.2 MiB");
        assert_eq!(transferred(1_258_291, 2_097_152), "1.2/2.0 MiB");
        // Both figures in the larger unit, so they compare at a glance.
        assert_eq!(transferred(512, 1_048_576), "0.0/1.0 MiB");
    }

    #[test]
    fn clipping_never_exceeds_the_room_it_was_given() {
        assert_eq!(clip("abc", 10), "abc");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
        assert_eq!(clip("abc", 0), "");
        assert_eq!(clip("abc", 1), "…");
    }

    #[test]
    fn the_name_column_is_a_fixed_width() {
        assert_eq!(super::column("fzf").chars().count(), NAME_WIDTH);
        assert_eq!(
            super::column("bitwarden-secrets-cli-x").chars().count(),
            NAME_WIDTH
        );
    }

    #[test]
    fn a_region_too_small_to_draw_is_left_alone() {
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        view(&with(vec![Stage::Waiting], 1), 0, area, &mut buf);
        assert!(rows(&buf).iter().all(String::is_empty));
    }
}
