//! Terminal output.
//!
//! The reference implementation writes raw SGR escapes unconditionally — no tty probing, no
//! `NO_COLOR` handling — so this module does the same. Anything smarter would silently break
//! parity the moment output is piped, which is exactly how the tool is used in CI.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};

/// `--pr` mode: suppresses INFO/SUCCESS/WARNING so only the pull-request body reaches stdout.
static PR_MODE: AtomicBool = AtomicBool::new(false);

/// Enables or disables `--pr` mode for the remainder of the process.
pub fn set_pr_mode(enabled: bool) {
    PR_MODE.store(enabled, Ordering::Relaxed);
}

/// Whether `--pr` mode is active.
#[must_use]
pub fn pr_mode() -> bool {
    PR_MODE.load(Ordering::Relaxed)
}

macro_rules! color_fn {
    ($(#[$meta:meta])* $name:ident, $code:literal) => {
        $(#[$meta])*
        #[must_use]
        pub fn $name(message: &str) -> String {
            format!(concat!("\x1b[", $code, "m{}\x1b[0m"), message)
        }
    };
}

color_fn!(/// Wraps `message` in green.
    green, "32");
color_fn!(/// Wraps `message` in red.
    red, "31");
color_fn!(/// Wraps `message` in yellow.
    yellow, "33");
color_fn!(/// Wraps `message` in cyan.
    cyan, "36");

/// Prints `WARNING: {message}` in yellow to stderr, unless `--pr` mode is active.
pub fn warning(message: impl Display) {
    if pr_mode() {
        return;
    }
    crate::progress::print_err(&yellow(&format!("WARNING: {message}")));
}

/// Prints `SUCCESS: {message}` in green to stdout, unless `--pr` mode is active.
pub fn success(message: impl Display) {
    if pr_mode() {
        return;
    }
    crate::progress::print_out(&green(&format!("SUCCESS: {message}")));
}

/// Prints `INFO: {message}` in cyan to stdout, unless `--pr` mode is active.
pub fn info(message: impl Display) {
    if pr_mode() {
        return;
    }
    crate::progress::print_out(&cyan(&format!("INFO: {message}")));
}

/// Prints `ERROR: {message}` in red to stderr. Never suppressed.
pub fn error(message: impl Display) {
    crate::progress::print_err(&red(&format!("ERROR: {message}")));
}

#[cfg(test)]
mod tests {
    use super::{cyan, green, red, yellow};

    #[test]
    fn colors_match_the_reference_escapes() {
        assert_eq!(green("x"), "\u{1b}[32mx\u{1b}[0m");
        assert_eq!(red("x"), "\u{1b}[31mx\u{1b}[0m");
        assert_eq!(yellow("x"), "\u{1b}[33mx\u{1b}[0m");
        assert_eq!(cyan("x"), "\u{1b}[36mx\u{1b}[0m");
    }
}
