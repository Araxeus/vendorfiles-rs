//! Argument parsing.
//!
//! `clap` builds the typed command, but the observable contract belongs to Commander: help is
//! served from captured text, and parse failures are re-worded and always exit 1.

// `Option<Option<T>>` is how clap models an option whose value is itself optional
// (`-n` alone versus `-n name`), which Commander's `[name]` syntax requires.
#![expect(clippy::option_option)]

use clap::{Parser, Subcommand};

use crate::spec;

/// Parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "vendor",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Config file path, or a folder containing the config file.
    ///
    /// The value is required, unlike the reference's `[file/folder path]`: naming the option is
    /// only ever a request for a specific config, and an optional value would let `-c` swallow
    /// the next word after a command that takes names.
    #[arg(
        short = 'c',
        long = "config",
        num_args = 1,
        value_name = "file/folder path",
        global = true
    )]
    pub config: Option<String>,

    /// Print plain lines instead of animating a live display.
    ///
    /// Global, so it reads naturally either side of the subcommand. `-p` is why `--pr` no longer
    /// has a short form: a flag that means one thing everywhere is worth more than the one
    /// letter the reference spent on `update`'s only option.
    #[arg(short = 'p', long = "plain", global = true)]
    pub plain: bool,

    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands, with the reference's aliases.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Sync all dependencies in the config file.
    #[command(alias = "s", disable_help_flag = true)]
    Sync {
        #[arg(short = 'f', long = "force")]
        force: bool,
    },

    /// Update all/selected dependencies to their latest version.
    #[command(
        aliases = ["upgrade", "bump", "up", "u"],
        disable_help_flag = true
    )]
    Update {
        names: Vec<String>,
        #[arg(long = "pr")]
        pr: bool,
    },

    /// List outdated dependencies.
    #[command(alias = "o", disable_help_flag = true)]
    Outdated,

    /// Install a dependency.
    #[command(aliases = ["add", "i", "a"], disable_help_flag = true)]
    Install {
        #[arg(value_name = "url/name")]
        source: String,
        #[arg(value_name = "version")]
        version: Option<String>,
        #[arg(short = 'n', long = "name", num_args = 0..=1, value_name = "name")]
        name: Option<Option<String>>,
        #[arg(short = 'f', long = "files", num_args = 1.., value_name = "files")]
        files: Option<Vec<String>>,
        /// Re-check the program registry rather than trusting the cached copy.
        #[arg(long = "refresh")]
        refresh: bool,
    },

    /// Uninstall all/selected dependencies.
    #[command(
        aliases = ["remove", "delete", "del", "rm", "un", "r"],
        disable_help_flag = true
    )]
    Uninstall { names: Vec<String> },

    /// Login to GitHub to increase rate limit.
    #[command(alias = "auth", disable_help_flag = true)]
    Login { token: Option<String> },
}

/// Index of the first token that is not a root option or one of its values.
///
/// That token is where the subcommand goes, whether or not it names a real one.
#[must_use]
pub fn first_operand(args: &[String]) -> Option<usize> {
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            index += 1;
            continue;
        }
        if spec::is_option_like(token) {
            let takes_value = !token.contains('=')
                && spec::ROOT_OPTIONS.iter().any(|option| {
                    (option.long == token
                        || option
                            .short
                            .is_some_and(|s| token.len() == 2 && token.ends_with(s)))
                        && option.arity != spec::Arity::None
                });
            index += 1;
            if takes_value
                && args
                    .get(index)
                    .is_some_and(|next| !spec::is_option_like(next))
            {
                index += 1;
            }
            continue;
        }
        return Some(index);
    }
    None
}

/// Resolves the subcommand named in argv, if it names a known one.
#[must_use]
pub fn locate_command(args: &[String]) -> Option<(usize, &'static spec::CommandSpec)> {
    let index = first_operand(args)?;
    spec::find(&args[index]).map(|command| (index, command))
}

/// Rewrites a `clap` failure into Commander's wording.
#[must_use]
pub fn commander_message(error: &clap::Error, args: &[String]) -> String {
    use clap::error::{ContextKind, ErrorKind};

    let context = |kind: ContextKind| {
        error.get(kind).map(|value| {
            value
                .to_string()
                .trim_matches(|c| matches!(c, '<' | '>' | '[' | ']'))
                .to_owned()
        })
    };

    match error.kind() {
        ErrorKind::InvalidSubcommand => context(ContextKind::InvalidSubcommand)
            .map_or_else(fallback_usage, |name| format!("unknown command '{name}'")),

        ErrorKind::UnknownArgument | ErrorKind::TooManyValues => {
            let Some(token) = context(ContextKind::InvalidArg) else {
                return fallback_usage();
            };
            if token.starts_with('-') {
                return format!("unknown option '{token}'");
            }
            match locate_command(args) {
                Some((index, command)) => {
                    let provided = command.count_operands(&args[index + 1..]);
                    let expected = command.max_operands.unwrap_or(provided);
                    format!(
                        "too many arguments for '{}'. Expected {expected} arguments but got {provided}.",
                        command.name
                    )
                }
                None => format!("unknown command '{token}'"),
            }
        }

        ErrorKind::MissingRequiredArgument => {
            context(ContextKind::InvalidArg).map_or_else(fallback_usage, |names| {
                // clap reports every missing argument; Commander names only the first.
                let first = names
                    .lines()
                    .next()
                    .unwrap_or(&names)
                    .trim_matches(|c| matches!(c, '<' | '>' | '[' | ']'));
                format!("missing required argument '{first}'")
            })
        }

        ErrorKind::InvalidValue | ErrorKind::TooFewValues | ErrorKind::NoEquals => {
            let id = context(ContextKind::InvalidArg).unwrap_or_default();
            let command = locate_command(args).map(|(_, command)| command);
            let key = id
                .split_whitespace()
                .next()
                .unwrap_or(&id)
                .trim_start_matches('-');
            spec::option_display(command, key).map_or_else(fallback_usage, |display| {
                format!("option '{display}' argument missing")
            })
        }

        _ => fallback_usage(),
    }
}

fn fallback_usage() -> String {
    "invalid arguments".to_owned()
}
