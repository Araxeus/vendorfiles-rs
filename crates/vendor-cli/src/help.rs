//! Commander-compatible help and version routing.
//!
//! Help must be answered before validation — `vendor install -h` prints help rather than
//! complaining about the missing `<url/name>` — so it is resolved by scanning argv up front.

use crate::cli::{first_operand, locate_command};
use crate::spec;

/// What a scan of argv decided before real parsing begins.
#[derive(Debug, PartialEq, Eq)]
pub enum Intercept {
    /// Print `text` to stdout and exit 0.
    Print(&'static str),
    /// Print the version to stdout and exit 0.
    Version,
    /// Print the root help to stderr and exit 1 — no command, or `help <unknown>`.
    UsageError,
    /// Nothing to intercept; hand over to the parser.
    Parse,
}

const HELP_FLAGS: [&str; 2] = ["-h", "--help"];
const VERSION_FLAGS: [&str; 2] = ["-v", "--version"];

/// Decides how to respond to `argv` (excluding the program name) before parsing it.
#[must_use]
pub fn intercept(args: &[String]) -> Intercept {
    let command = locate_command(args);
    let command_index = first_operand(args);

    // A help or version flag before the subcommand belongs to the root command.
    let root_slice = &args[..command_index.unwrap_or(args.len())];
    if root_slice.iter().any(|a| HELP_FLAGS.contains(&a.as_str())) {
        return Intercept::Print(spec::ROOT_HELP);
    }
    if root_slice
        .iter()
        .any(|a| VERSION_FLAGS.contains(&a.as_str()))
    {
        return Intercept::Version;
    }

    // `help [command]` is a command, not a flag.
    if let Some(index) = args.iter().position(|a| a == "help")
        && command_index.is_none_or(|c| index <= c)
    {
        return args
            .get(index + 1)
            .map_or(Intercept::Print(spec::ROOT_HELP), |topic| {
                spec::find(topic)
                    .map_or(Intercept::UsageError, |found| Intercept::Print(found.help))
            });
    }

    match (command_index, command) {
        // Nothing that could be a command: Commander prints usage and fails.
        (None, _) => Intercept::UsageError,
        // A token is there but names no command: let the parser report it.
        (Some(_), None) => Intercept::Parse,
        (Some(index), Some((_, found))) => {
            if args[index + 1..]
                .iter()
                .any(|a| HELP_FLAGS.contains(&a.as_str()))
            {
                Intercept::Print(found.help)
            } else {
                Intercept::Parse
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Intercept, intercept};
    use crate::spec;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_arguments_is_a_usage_error() {
        assert_eq!(intercept(&args(&[])), Intercept::UsageError);
        assert_eq!(intercept(&args(&["-c"])), Intercept::UsageError);
    }

    #[test]
    fn root_flags_are_answered_before_any_command() {
        assert_eq!(
            intercept(&args(&["--help"])),
            Intercept::Print(spec::ROOT_HELP)
        );
        assert_eq!(intercept(&args(&["-v"])), Intercept::Version);
    }

    #[test]
    fn subcommand_help_wins_over_missing_arguments() {
        let install = spec::find("install").unwrap();
        assert_eq!(
            intercept(&args(&["install", "-h"])),
            Intercept::Print(install.help)
        );
        assert_eq!(
            intercept(&args(&["i", "--help"])),
            Intercept::Print(install.help)
        );
    }

    #[test]
    fn help_subcommand_resolves_its_topic() {
        let sync = spec::find("sync").unwrap();
        assert_eq!(
            intercept(&args(&["help"])),
            Intercept::Print(spec::ROOT_HELP)
        );
        assert_eq!(
            intercept(&args(&["help", "sync"])),
            Intercept::Print(sync.help)
        );
        assert_eq!(intercept(&args(&["help", "nope"])), Intercept::UsageError);
    }

    #[test]
    fn real_invocations_are_handed_to_the_parser() {
        assert_eq!(intercept(&args(&["sync", "-f"])), Intercept::Parse);
        assert_eq!(
            intercept(&args(&["-c", "dir", "outdated"])),
            Intercept::Parse
        );
        assert_eq!(intercept(&args(&["frobnicate"])), Intercept::Parse);
    }
}
