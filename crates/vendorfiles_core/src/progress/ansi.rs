//! Turns our own coloured strings into ratatui text.
//!
//! Lines printed above the region - `INFO:`, `SUCCESS:`, the `outdated` listing - are built with
//! the escape sequences in [`crate::ui`], because a piped run has to emit those bytes exactly.
//! Rather than build every line twice, the animated path parses them back.
//!
//! Only the escapes we generate ourselves are understood: SGR reset, bold, dim, and the eight
//! basic foreground colours. Anything else is dropped rather than shown, so a stray sequence
//! cannot leak into the frame as literal text.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Parses one line of possibly-coloured text.
#[must_use]
pub fn to_line(text: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style = Style::new();
    let mut plain = String::new();
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            plain.push(character);
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        let mut params = String::new();
        let mut terminator = None;
        for character in chars.by_ref() {
            if character.is_ascii_digit() || character == ';' {
                params.push(character);
            } else {
                terminator = Some(character);
                break;
            }
        }
        // Only SGR ends in `m`. Anything else is a sequence we did not write; drop it.
        if terminator != Some('m') {
            continue;
        }
        if !plain.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut plain), style));
        }
        style = apply(style, &params);
    }
    if !plain.is_empty() {
        spans.push(Span::styled(plain, style));
    }
    Line::from(spans)
}

/// Splits text into lines and parses each, since inserting above the region needs a height.
#[must_use]
pub fn to_lines(text: &str) -> Vec<Line<'static>> {
    let lines: Vec<Line<'static>> = text.lines().map(to_line).collect();
    if lines.is_empty() {
        return vec![Line::default()];
    }
    lines
}

/// Applies one SGR parameter list to the style in effect.
fn apply(style: Style, params: &str) -> Style {
    if params.is_empty() {
        // A bare `ESC[m` is a reset.
        return Style::new();
    }
    let mut style = style;
    for part in params.split(';') {
        match part.parse::<u16>() {
            Ok(0) => style = Style::new(),
            Ok(1) => style = style.bold(),
            Ok(2) => style = style.dim(),
            Ok(code @ 30..=37) => style = style.fg(basic(code - 30)),
            Ok(code @ 90..=97) => style = style.fg(bright(code - 90)),
            _ => {}
        }
    }
    style
}

/// The eight basic foreground colours, in SGR order.
const fn basic(index: u16) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        _ => Color::Gray,
    }
}

/// Their bright counterparts.
const fn bright(index: u16) -> Color {
    match index {
        0 => Color::DarkGray,
        1 => Color::LightRed,
        2 => Color::LightGreen,
        3 => Color::LightYellow,
        4 => Color::LightBlue,
        5 => Color::LightMagenta,
        6 => Color::LightCyan,
        _ => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::{to_line, to_lines};
    use crate::ui;
    use ratatui::style::{Color, Style};

    /// The visible text, with every escape removed.
    fn text(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn every_colour_helper_round_trips() {
        for (paint, colour) in [
            (ui::green as fn(&str) -> String, Color::Green),
            (ui::red, Color::Red),
            (ui::yellow, Color::Yellow),
            (ui::cyan, Color::Cyan),
        ] {
            let line = to_line(&paint("hello"));
            assert_eq!(text(&line), "hello");
            assert_eq!(line.spans[0].style.fg, Some(colour));
        }
    }

    #[test]
    fn a_line_with_several_colours_keeps_each_one() {
        let text_in = format!("{} → {}", ui::red("v1"), ui::green("v2"));
        let line = to_line(&text_in);
        assert_eq!(text(&line), "v1 → v2");
        let coloured: Vec<Option<Color>> = line.spans.iter().map(|span| span.style.fg).collect();
        assert!(coloured.contains(&Some(Color::Red)));
        assert!(coloured.contains(&Some(Color::Green)));
    }

    #[test]
    fn plain_text_survives_untouched() {
        let line = to_line("INFO: Saved vendor/fzf/fzf.exe");
        assert_eq!(text(&line), "INFO: Saved vendor/fzf/fzf.exe");
        assert_eq!(line.spans[0].style, Style::new());
    }

    #[test]
    fn a_reset_ends_the_colour_it_was_given() {
        let line = to_line("\u{1b}[32mgreen\u{1b}[0mplain");
        assert_eq!(line.spans[0].style.fg, Some(Color::Green));
        assert_eq!(line.spans[1].style.fg, None);
    }

    #[test]
    fn a_sequence_we_never_write_is_dropped_rather_than_shown() {
        // A cursor move, not a colour: it must not appear as literal text in the frame.
        let line = to_line("before\u{1b}[2Aafter");
        assert_eq!(text(&line), "beforeafter");
    }

    #[test]
    fn multiple_lines_become_multiple_rows() {
        assert_eq!(to_lines("one\ntwo\nthree").len(), 3);
        assert_eq!(to_lines("").len(), 1);
    }
}
