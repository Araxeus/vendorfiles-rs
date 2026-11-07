//! `vendor completions` - shell completion scripts.

use anyhow::{Result, bail};
use vendorfiles_core::registry;

use crate::cli::Cli;

/// The shells `vendor completions` can write a script for.
const SHELLS: [(&str, clap_complete::Shell); 5] = [
    ("bash", clap_complete::Shell::Bash),
    ("elvish", clap_complete::Shell::Elvish),
    ("fish", clap_complete::Shell::Fish),
    ("powershell", clap_complete::Shell::PowerShell),
    ("zsh", clap_complete::Shell::Zsh),
];

/// The command the completion scripts describe.
///
/// Not the parser: `Cli` turns clap's help and version handling off, because `help::intercept`
/// answers `-h`, `-v` and `help [command]` from captured text before parsing, so Commander's
/// wording and exit codes survive. Generating from the parser alone would therefore promise less
/// than the binary accepts, so those three go back on here. Nothing parses this command; only its
/// shape is read.
///
/// `programs` become the values offered for `install`'s operand. They are only suggestions to the
/// shell: nothing parses with this command, so any other source is still accepted.
fn completion_command(programs: &[String]) -> clap::Command {
    use clap::{Arg, ArgAction, Command as ClapCommand, CommandFactory};

    // Wording taken from the served help text, so what a shell shows is what `vendor --help`
    // prints.
    let help_flag = || {
        Arg::new("help")
            .short('h')
            .long("help")
            .help("display help for command")
            .action(ArgAction::Help)
    };

    let mut command = Cli::command()
        .version(vendorfiles_core::VERSION)
        .arg(help_flag())
        .arg(
            Arg::new("version")
                .short('v')
                .long("version")
                .help("output the current version")
                .action(ArgAction::Version),
        );

    // Every subcommand takes `-h` as well, and every one of them disables clap's own. `config`
    // has subcommands of its own, and `vendor config edit -h` is answered like any other, so the
    // flag goes on those too. `help` is appended afterwards deliberately: `vendor help -h` is a
    // usage error, so it must not gain one.
    let topics: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for topic in &topics {
        command = command.mut_subcommand(topic, |sub| {
            let nested: Vec<String> = sub
                .get_subcommands()
                .map(|inner| inner.get_name().to_owned())
                .collect();
            nested
                .iter()
                .fold(sub, |sub, name| {
                    sub.mut_subcommand(name, |inner| inner.arg(help_flag()))
                })
                .arg(help_flag())
        });
    }

    if !programs.is_empty() {
        command = command.mut_subcommand("install", |install| {
            install.mut_arg("sources", |sources| {
                sources.value_parser(clap::builder::PossibleValuesParser::new(
                    programs.iter().cloned(),
                ))
            })
        });
    }

    command.subcommand(
        ClapCommand::new("help")
            .about("display help for command")
            // `help help` is not a topic, so the names collected above are exactly the list.
            .arg(
                Arg::new("command")
                    .value_name("command")
                    .value_parser(topics),
            ),
    )
}

/// Every name and alias in the registry, or none when it cannot be had.
///
/// Silent on failure: a completion script is still worth having without them, and stderr may be
/// where the user is looking for nothing at all.
async fn registry_names() -> Vec<String> {
    registry::load(false).await.map_or_else(
        |_| Vec::new(),
        |registry| {
            registry
                .programs()
                .flat_map(|(name, program)| {
                    std::iter::once(name.to_owned()).chain(program.aliases.iter().cloned())
                })
                .collect()
        },
    )
}

/// Writes a completion script for `shell` to stdout.
///
/// Generated from the parser itself, so it stays in step with the flags rather than being a second
/// description of them that has to be maintained. Registry names are taken as they are now, so a
/// script generated before a program joined the registry does not offer it.
pub(super) async fn completions(shell: &str) -> Result<()> {
    let wanted = shell.to_ascii_lowercase();
    let Some((_, generator)) = SHELLS.iter().find(|(name, _)| *name == wanted) else {
        let accepted = SHELLS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("unknown shell '{shell}'. Expected one of {accepted}");
    };

    let mut command = completion_command(&registry_names().await);
    clap_complete::generate(
        *generator,
        &mut command,
        "vendor",
        &mut std::io::stdout().lock(),
    );
    Ok(())
}
