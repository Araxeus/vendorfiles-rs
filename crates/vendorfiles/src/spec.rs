//! The command grammar, described once and used for help lookup, operand counting and error
//! wording.
//!
//! `clap` does the real parsing; this table exists because the CLI's observable contract is
//! Commander's, and reproducing Commander's help routing and error text needs to know the
//! shape of each command before `clap` gets a chance to reject it.

/// How many values an option consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    /// A boolean flag.
    None,
    /// `[value]` — takes the next token unless it looks like an option.
    Optional,
    /// `<value...>` — takes every following token until one looks like an option.
    Many,
}

/// One option, as Commander declares it.
#[derive(Debug, Clone, Copy)]
pub struct OptionSpec {
    pub short: Option<char>,
    pub long: &'static str,
    pub arity: Arity,
    /// Commander's rendering, used in `option '…' argument missing`.
    pub display: &'static str,
}

/// One subcommand.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Maximum positional operands, or `None` when variadic.
    pub max_operands: Option<usize>,
    pub options: &'static [OptionSpec],
    /// Help text, byte-identical to the reference CLI's but for `--plain` and the short form it
    /// took from `--pr`.
    pub help: &'static str,
}

impl CommandSpec {
    /// Whether `token` names this command.
    #[must_use]
    pub fn matches(&self, token: &str) -> bool {
        self.name == token || self.aliases.contains(&token)
    }

    /// The option `token` names, whether it belongs to this command or to the root.
    ///
    /// Root options are global, so `vendor sync -c path` is legal; without the fallback the
    /// scanner would not know `-c` consumes `path` and would count it as an operand.
    fn option_for(&self, token: &str) -> Option<&OptionSpec> {
        self.options.iter().chain(ROOT_OPTIONS).find(|option| {
            option.long == token
                || option
                    .short
                    .is_some_and(|short| token.len() == 2 && token.ends_with(short))
        })
    }

    /// Counts positional operands in a sub-argv, the way Commander separates them from options.
    #[must_use]
    pub fn count_operands(&self, args: &[String]) -> usize {
        let mut operands = 0;
        let mut index = 0;
        while index < args.len() {
            let token = &args[index];
            if token == "--" {
                return operands + (args.len() - index - 1);
            }
            if !is_option_like(token) {
                operands += 1;
                index += 1;
                continue;
            }
            let arity = self
                .option_for(token.split('=').next().unwrap_or(token))
                .map_or(Arity::None, |option| option.arity);
            index += 1;
            if token.contains('=') {
                continue;
            }
            match arity {
                Arity::None => {}
                Arity::Optional => {
                    if args.get(index).is_some_and(|next| !is_option_like(next)) {
                        index += 1;
                    }
                }
                Arity::Many => {
                    while args.get(index).is_some_and(|next| !is_option_like(next)) {
                        index += 1;
                    }
                }
            }
        }
        operands
    }
}

/// Whether a token would be read as an option rather than a value.
#[must_use]
pub fn is_option_like(token: &str) -> bool {
    token.len() > 1 && token.starts_with('-')
}

const CONFIG_OPTION: OptionSpec = OptionSpec {
    short: Some('c'),
    long: "--config",
    // Its value is required, so it always consumes the next token; `Optional` is what the
    // scanner needs — it consumes one when present, and a missing one is clap's to reject.
    arity: Arity::Optional,
    display: "-c, --config <file/folder path>",
};

const PLAIN_OPTION: OptionSpec = OptionSpec {
    short: Some('p'),
    long: "--plain",
    arity: Arity::None,
    display: "-p, --plain",
};

/// The root command's own options.
///
/// `--plain` is global, so it may also appear after the subcommand; being `Arity::None` it needs
/// no special handling when operands are counted.
pub const ROOT_OPTIONS: &[OptionSpec] = &[CONFIG_OPTION, PLAIN_OPTION];

/// Help text for `vendor` with no subcommand.
pub const ROOT_HELP: &str = include_str!("help/root.txt");

/// Every subcommand, in the order the root help lists them.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "sync",
        aliases: &["s"],
        max_operands: Some(0),
        options: &[OptionSpec {
            short: Some('f'),
            long: "--force",
            arity: Arity::None,
            display: "-f, --force",
        }],
        help: include_str!("help/sync.txt"),
    },
    CommandSpec {
        name: "update",
        aliases: &["upgrade", "bump", "up", "u"],
        max_operands: None,
        options: &[OptionSpec {
            // No short: `-p` means `--plain` everywhere.
            short: None,
            long: "--pr",
            arity: Arity::None,
            display: "--pr",
        }],
        help: include_str!("help/update.txt"),
    },
    CommandSpec {
        name: "outdated",
        aliases: &["o"],
        max_operands: Some(0),
        options: &[],
        help: include_str!("help/outdated.txt"),
    },
    CommandSpec {
        name: "install",
        aliases: &["add", "i", "a"],
        max_operands: Some(2),
        options: &[
            OptionSpec {
                short: Some('n'),
                long: "--name",
                arity: Arity::Optional,
                display: "-n, --name [name]",
            },
            OptionSpec {
                short: Some('f'),
                long: "--files",
                arity: Arity::Many,
                display: "-f, --files <files...>",
            },
            OptionSpec {
                short: None,
                long: "--refresh",
                arity: Arity::None,
                display: "--refresh",
            },
            OptionSpec {
                short: None,
                long: "--dry-run",
                arity: Arity::None,
                display: "--dry-run",
            },
        ],
        help: include_str!("help/install.txt"),
    },
    CommandSpec {
        name: "uninstall",
        aliases: &["remove", "delete", "del", "rm", "un", "r"],
        max_operands: None,
        options: &[],
        help: include_str!("help/uninstall.txt"),
    },
    CommandSpec {
        name: "login",
        aliases: &["auth"],
        max_operands: Some(1),
        options: &[],
        help: include_str!("help/login.txt"),
    },
];

/// Looks a command up by name or alias.
#[must_use]
pub fn find(token: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| command.matches(token))
}

/// Looks an option up across the root and a command, for `argument missing` wording.
#[must_use]
pub fn option_display(command: Option<&CommandSpec>, id: &str) -> Option<&'static str> {
    let haystack = command.map_or(ROOT_OPTIONS, |c| c.options);
    haystack
        .iter()
        .chain(ROOT_OPTIONS)
        .find(|option| option.long.trim_start_matches('-') == id)
        .map(|option| option.display)
}

#[cfg(test)]
mod tests {
    use super::{find, is_option_like, option_display};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn aliases_resolve_to_their_command() {
        assert_eq!(find("s").unwrap().name, "sync");
        assert_eq!(find("bump").unwrap().name, "update");
        assert_eq!(find("rm").unwrap().name, "uninstall");
        assert_eq!(find("auth").unwrap().name, "login");
        assert!(find("nope").is_none());
    }

    #[test]
    fn operand_counting_skips_options_and_their_values() {
        let install = find("install").unwrap();
        assert_eq!(install.count_operands(&args(&["React", "v1"])), 2);
        // A variadic option swallows every following value, so `v1` never becomes an operand.
        assert_eq!(
            install.count_operands(&args(&["React", "-f", "a.txt", "b.txt", "v1"])),
            1
        );
        assert_eq!(
            install.count_operands(&args(&["React", "v1", "-f", "a.txt"])),
            2
        );
        assert_eq!(install.count_operands(&args(&["-n", "Name", "React"])), 1);
        assert_eq!(install.count_operands(&args(&["--name=Foo", "React"])), 1);

        let sync = find("sync").unwrap();
        // A root option is global, so its value is not an operand wherever it appears.
        assert_eq!(sync.count_operands(&args(&["-c", "some/path"])), 0);
        assert_eq!(sync.count_operands(&args(&["-c", "some/path", "extra"])), 1);
        assert_eq!(
            install.count_operands(&args(&["React", "v1", "--config", "p"])),
            2
        );
        assert_eq!(sync.count_operands(&args(&["-f"])), 0);
        assert_eq!(sync.count_operands(&args(&["-f", "extra"])), 1);
    }

    #[test]
    fn double_dash_makes_everything_an_operand() {
        let sync = find("sync").unwrap();
        assert_eq!(sync.count_operands(&args(&["--", "-f", "x"])), 2);
    }

    #[test]
    fn option_like_ignores_a_lone_dash() {
        assert!(is_option_like("-f"));
        assert!(is_option_like("--files"));
        assert!(!is_option_like("-"));
        assert!(!is_option_like("file"));
    }

    #[test]
    fn option_display_uses_commander_wording() {
        assert_eq!(
            option_display(find("install"), "files"),
            Some("-f, --files <files...>")
        );
        // `<…>`, not the reference's `[…]`: the value is required here.
        assert_eq!(
            option_display(None, "config"),
            Some("-c, --config <file/folder path>")
        );
    }
}
