//! Command dispatch — the layer between parsed arguments and the library's operations.

use anyhow::{Result, bail};
use vendorfiles_core::error::VendorError;
use vendorfiles_core::model::{DefaultOptions, RawDependency};
use vendorfiles_core::progress::Reporter;
use vendorfiles_core::template::{
    is_github_url, is_owner_repo_shorthand, owner_and_name_from_repo_url,
};
use vendorfiles_core::{GitHubClient, InstallOptions, Session, SyncOptions, Workspace, auth};

use crate::cli::{Cli, Command};
use crate::known;
use vendorfiles_core::registry;
use vendorfiles_core::ui;

/// Loads the workspace and runs the requested command.
///
/// `login` runs without a config file: authenticating is not a project-scoped action, and the
/// reference's blanket `preAction` hook made `vendor login` fail outside a project.
pub async fn dispatch(cli: Cli) -> Result<()> {
    // Before the config is looked for: a completion script has nothing to do with a project, and
    // asking for one outside a project should not fail.
    if let Command::Completions { shell } = &cli.command {
        return completions(shell);
    }

    if let Command::Login { token } = &cli.command {
        return match token {
            Some(token) => auth::login_with_token(token).await.map_err(Into::into),
            None => auth::login_with_device_flow().await.map_err(Into::into),
        };
    }

    // Set before the session builds its display: `--pr` means stdout is a machine-readable
    // summary, so nothing should animate. It only applies to a whole-project update, matching
    // the reference.
    if let Command::Update { names, pr: true } = &cli.command
        && names.is_empty()
    {
        vendorfiles_core::ui::set_pr_mode(true);
    }

    let config_location = cli.config;
    let workspace = Workspace::load(config_location.as_deref()).await?;
    let github = GitHubClient::new(auth::resolve_token_async().await)?;
    // `--plain` asks for the output a pipe would get: no region, just the lines.
    let mut session = if cli.plain {
        Session::with_reporter(github, workspace, Reporter::new(false))
    } else {
        Session::new(github, workspace)
    };

    match cli.command {
        Command::Sync { force } => {
            session
                .sync(SyncOptions {
                    should_update: false,
                    force,
                    show_outdated_only: false,
                })
                .await?;
        }

        Command::Update { names, pr: _ } => {
            if names.is_empty() {
                session
                    .sync(SyncOptions {
                        should_update: true,
                        force: false,
                        show_outdated_only: false,
                    })
                    .await?;
            } else {
                for name in names {
                    upgrade_one(&mut session, &name).await?;
                }
            }
        }

        Command::Outdated => {
            session
                .sync(SyncOptions {
                    should_update: true,
                    force: false,
                    show_outdated_only: true,
                })
                .await?;
        }

        Command::Install {
            source,
            version,
            name,
            files,
            refresh,
            dry_run,
        } => {
            install(
                &mut session,
                &source,
                version,
                name.flatten(),
                files,
                Preview { refresh, dry_run },
            )
            .await?;
        }

        Command::Uninstall { names } => {
            if names.is_empty() {
                bail!(VendorError::NoPackageNames);
            }
            for name in names {
                session.uninstall(&name).await?;
            }
        }

        // Handled before the workspace is loaded.
        Command::Login { .. } => unreachable!("login is dispatched without a workspace"),
        Command::Completions { .. } => {
            unreachable!("completions are dispatched without a workspace")
        }
    }

    Ok(())
}

/// `vendor update <name>` — re-resolve one dependency to its latest version.
async fn upgrade_one(session: &mut Session, name: &str) -> Result<()> {
    let entry = session
        .workspace
        .dependencies
        .get(name)
        .cloned()
        .ok_or_else(|| VendorError::NoDependencyNamed(name.to_owned()))?;

    if entry.repository.is_none() {
        bail!(VendorError::NoRepositoryForDependency(name.to_owned()));
    }
    if entry.files.as_ref().is_none_or(Vec::is_empty) {
        bail!(VendorError::NoFilesForDependency(name.to_owned()));
    }
    if entry.locked == Some(true) {
        bail!(VendorError::DependencyLocked(name.to_owned()));
    }

    let dependency = entry.resolve(name)?;
    session
        .install(
            dependency,
            InstallOptions {
                should_update: true,
                ..InstallOptions::default()
            },
        )
        .await?;
    Ok(())
}

/// What a new entry takes from a *neighbour* — an entry under a different name that already
/// vendors the same repository.
///
/// The whole neighbour is used as the base of the new entry, so in principle everything unset on
/// the new one comes from it. Only two of those fields are worth naming, because only they change
/// what the command visibly does:
///
/// * `files`, but only when the command was given none and the neighbour has some. A neighbour
///   with no files leaves a known name or the registry to describe them, and nothing is borrowed.
/// * `version`, always: the new entry starts out claiming the neighbour's, so when that already
///   matches what gets installed the entry is never written to the config at all. An explicit
///   version on the command line does not stop this — it decides what to install, not what the
///   entry starts out at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Inherited {
    files: bool,
    version: bool,
}

impl Inherited {
    /// What `neighbour` hands over, given whether the command supplied its own `--files`.
    fn from_neighbour(neighbour: &RawDependency, files_given: bool) -> Self {
        Self {
            files: !files_given
                && neighbour
                    .files
                    .as_deref()
                    .is_some_and(|files| !files.is_empty()),
            version: neighbour
                .version
                .as_deref()
                .is_some_and(|version| !version.is_empty()),
        }
    }

    /// What to warn about, or `None` when nothing came from the neighbour.
    fn warning(self, neighbour: &str, name: &str, repository: &str) -> Option<String> {
        let (borrowed, advice) = match (self.files, self.version) {
            // `--files` replaces the files and only the files. Saying it describes the new entry
            // "separately" would promise more than it delivers, since the version — the half that
            // can swallow the config write — comes from the neighbour whatever the command says.
            (true, true) => (
                "its files and version",
                format!(
                    " Pass --files to give '{name}' its own files; the version comes from \
                     '{neighbour}' either way."
                ),
            ),
            (true, false) => (
                "its files",
                format!(" Pass --files to describe '{name}' separately."),
            ),
            // Nothing to pass: this is the half no argument overrides.
            (false, true) => ("its version", String::new()),
            (false, false) => return None,
        };
        Some(format!(
            "'{neighbour}' already vendors {repository}, so '{name}' inherits {borrowed}.{advice}"
        ))
    }
}

fn merge_install_entry(
    existing: RawDependency,
    defaults: &DefaultOptions,
    repository: String,
    files: Option<Vec<vendorfiles_core::FileEntry>>,
) -> RawDependency {
    let mut entry = RawDependency {
        // An entry already in the config keeps its own URL; rewriting a user's URL from a
        // shorthand would be rude.
        repository: existing.repository.clone().or(Some(repository)),
        files,
        ..existing
    };
    entry.apply_defaults(defaults);
    entry
}

/// Prints the entry an install would add, and where its files would land.
///
/// Deliberately offline: resolving a name, merging the defaults and choosing the platform's asset
/// all happen locally, and stopping before the first request is what makes this answer "what will
/// this put in my config" instantly — and what lets the tests cover the registry without a
/// network. The version is left to the install, being the one part that has to ask GitHub.
fn report_entry(session: &Session, name: &str, entry: &RawDependency) -> Result<()> {
    let folder = session
        .workspace
        .dependency_folder(entry.vendor_folder.as_deref(), name);
    let mut wrapper = serde_json::Map::new();
    wrapper.insert(
        name.to_owned(),
        serde_json::to_value(entry).map_err(|source| VendorError::SerializeConfig {
            path: name.to_owned(),
            message: source.to_string(),
        })?,
    );
    let rendered =
        serde_json::to_string_pretty(&serde_json::Value::Object(wrapper)).map_err(|source| {
            VendorError::SerializeConfig {
                path: name.to_owned(),
                message: source.to_string(),
            }
        })?;

    ui::info(format!("{name} would be added as:"));
    vendorfiles_core::progress::print_out(&rendered);
    ui::info(format!("files would be written to {}", folder.display()));
    ui::info("nothing was downloaded or written");
    Ok(())
}

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
fn completion_command() -> clap::Command {
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

    // Every subcommand takes `-h` as well, and every one of them disables clap's own. `help` is
    // appended afterwards deliberately: `vendor help -h` is a usage error, so it must not gain one.
    let topics: Vec<String> = command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect();
    for topic in &topics {
        command = command.mut_subcommand(topic, |sub| sub.arg(help_flag()));
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

/// Writes a completion script for `shell` to stdout.
///
/// Generated from the parser itself, so it stays in step with the flags rather than being a second
/// description of them that has to be maintained.
fn completions(shell: &str) -> Result<()> {
    let wanted = shell.to_ascii_lowercase();
    let Some((_, generator)) = SHELLS.iter().find(|(name, _)| *name == wanted) else {
        let accepted = SHELLS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("unknown shell '{shell}'. Expected one of {accepted}");
    };

    let mut command = completion_command();
    clap_complete::generate(
        *generator,
        &mut command,
        "vendor",
        &mut std::io::stdout().lock(),
    );
    Ok(())
}

/// What a `source` argument turned out to name.
struct Source {
    /// The URL the reference builds, warts and all — `owner/repo` becomes
    /// `https://www.github.com/owner/repo`, and that exact string is what config entries are
    /// compared against.
    lookup: String,
    /// The same repository without the `www.`, the form every documented example uses and the
    /// form the search path returns.
    stored: String,
    /// A name this tool knows describes itself.
    known: Option<known::Known>,
    /// A name the hosted registry describes.
    listed: Option<registry::Entry>,
}

/// Works out which repository `source` means, and what is already known about it.
async fn resolve_source(
    session: &Session,
    source: &str,
    files_given: bool,
    refresh: bool,
) -> Result<Source> {
    // A name this tool knows needs neither a search nor `--files`. Anything explicit the user
    // passed still wins.
    let known = if files_given {
        None
    } else {
        known::find(source)
    };

    // Then the hosted registry, which is what makes `vendor add fd` work without a repository or
    // a `--files` list. A registry that cannot be reached is a miss, not a failure: the search
    // still works, and so does `vendor add owner/repo`.
    let bare_name = !files_given
        && known.is_none()
        && !is_github_url(source)
        && !is_owner_repo_shorthand(source);
    let listed = if bare_name {
        match registry::lookup(source, refresh).await {
            Ok(entry) => entry,
            Err(error) => {
                ui::warning(error);
                None
            }
        }
    } else {
        None
    };

    let (lookup, stored) = if let Some(known) = known.as_ref() {
        (known.repository.to_owned(), known.repository.to_owned())
    } else if let Some(listed) = listed.as_ref() {
        (listed.repository.clone(), listed.repository.clone())
    } else if is_github_url(source) {
        (source.to_owned(), source.to_owned())
    } else if is_owner_repo_shorthand(source) {
        (
            format!("https://www.github.com/{source}"),
            format!("https://github.com/{source}"),
        )
    } else {
        let found = session.github.find_repo_url(source).await?;
        (found.clone(), found)
    };

    Ok(Source {
        lookup,
        stored,
        known,
        listed,
    })
}

/// `vendor install <url/name> [version]`.
/// How much of an install to actually perform.
struct Preview {
    /// Re-check the registry rather than trusting the cached copy.
    refresh: bool,
    /// Resolve and report, but download nothing and write nothing.
    dry_run: bool,
}

async fn install(
    session: &mut Session,
    source: &str,
    version: Option<String>,
    name: Option<String>,
    files: Option<Vec<String>>,
    preview: Preview,
) -> Result<()> {
    let Source {
        lookup,
        stored,
        known,
        listed,
    } = resolve_source(session, source, files.is_some(), preview.refresh).await?;

    if !is_github_url(&lookup) {
        bail!(VendorError::InvalidGitHubUrlQuoted(lookup));
    }

    let name = match name.filter(|n| !n.is_empty()) {
        Some(name) => name,
        // A registry entry names itself, so `vendor add rg` keys the entry `ripgrep` — the
        // canonical name, not the alias that was typed.
        None => match listed.as_ref() {
            Some(listed) => listed.name.clone(),
            None => owner_and_name_from_repo_url(&lookup)?.name,
        },
    };

    // Files may be inherited from an entry under this name, or from any entry pointing at the
    // same repository — the reference looks in both places.
    let under_this_name = session.workspace.dependencies.get(&name).cloned();
    // Which *other* entry it would be borrowed from, when there is nothing under this name. Worth
    // knowing separately: inheriting from an entry the user did not name is the surprising half.
    let neighbour = if under_this_name.is_some() {
        None
    } else {
        session
            .workspace
            .dependencies
            .iter()
            .find(|(_, dependency)| dependency.repository.as_deref() == Some(lookup.as_str()))
            .map(|(key, dependency)| (key.clone(), dependency.clone()))
    };
    let inherited = neighbour
        .as_ref()
        .map(|(_, dependency)| Inherited::from_neighbour(dependency, files.is_some()));
    let existing = under_this_name
        .or_else(|| neighbour.as_ref().map(|(_, dependency)| dependency.clone()))
        .unwrap_or_default();

    let files = match files {
        Some(files) => Some(
            files
                .into_iter()
                .map(vendorfiles_core::FileEntry::Simple)
                .collect(),
        ),
        // An entry already in the config describes itself; otherwise a known name or the
        // registry does.
        None => existing
            .files
            .clone()
            .or_else(|| known.as_ref().map(|known| known.files.clone()))
            .or_else(|| listed.as_ref().map(|listed| listed.files.clone())),
    };
    if files.as_ref().is_none_or(Vec::is_empty) {
        bail!(VendorError::MissingFilesOption);
    }

    // Say what the neighbour handed over, now that the whole precedence chain has run and the
    // command is known to have something to install. Borrowing from an entry the user never
    // named is the surprising half of `install`, and the borrowed `version` is the sharp edge:
    // the new entry starts out claiming it, so when it already matches what gets installed the
    // entry is never written at all — which looks like nothing happening for no reason.
    if let Some(((neighbour, _), inherited)) = neighbour.as_ref().zip(inherited)
        && let Some(warning) = inherited.warning(neighbour, &name, &lookup)
    {
        ui::warning(warning);
    }

    let mut entry = merge_install_entry(existing, &session.workspace.defaults, stored, files);
    // Where a known dependency belongs, unless the config already says.
    if entry.vendor_folder.is_none()
        && let Some(known) = known.as_ref()
    {
        entry.vendor_folder = known.folder.clone();
    }
    // What a registry entry knows about the repository that the config does not yet.
    if let Some(listed) = listed.as_ref() {
        if entry.release_regex.is_none() {
            entry.release_regex = listed.release_regex.clone();
        }
        if entry.hash_version_file.is_none() && listed.hash_version_file == Some(true) {
            entry.hash_version_file = Some(vendorfiles_core::model::HashVersionFile::Flag(true));
        }
    }
    let dependency = entry.resolve(&name)?;

    if preview.dry_run {
        return report_entry(session, &name, &entry);
    }

    session
        .install(
            dependency,
            InstallOptions {
                should_update: version.is_none(),
                new_version: version,
                ..InstallOptions::default()
            },
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Inherited, merge_install_entry};
    use vendorfiles_core::model::{DefaultOptions, RawDependency};

    fn defaults(json: &str) -> DefaultOptions {
        serde_json::from_str(json).expect("valid default block")
    }

    fn files(list: &[&str]) -> Vec<vendorfiles_core::FileEntry> {
        list.iter()
            .map(|f| vendorfiles_core::FileEntry::Simple((*f).to_owned()))
            .collect()
    }

    #[test]
    fn a_dependency_the_config_never_listed_still_inherits_the_default_block() {
        // The reported bug: the first `vendor add` ignored `default.vendorFolder`, so the file
        // landed in a folder named after the dependency, and the second run - which read the
        // entry back from the config with defaults applied - used a different folder.
        let entry = merge_install_entry(
            RawDependency::default(),
            &defaults(r#"{"vendorFolder": "{vendorFolder}", "locked": false}"#),
            "https://github.com/mvdan/sh".to_owned(),
            Some(files(&["{release}/shfmt_v{version}_windows_amd64.exe"])),
        );
        assert_eq!(entry.vendor_folder.as_deref(), Some("{vendorFolder}"));
        assert_eq!(entry.locked, Some(false));
    }

    #[test]
    fn the_second_run_resolves_to_the_same_entry_as_the_first() {
        let block = defaults(r#"{"vendorFolder": "{vendorFolder}"}"#);
        let repository = "https://github.com/mvdan/sh".to_owned();
        let requested = Some(files(&["{release}/shfmt_v{version}_windows_amd64.exe"]));

        let first = merge_install_entry(
            RawDependency::default(),
            &block,
            repository.clone(),
            requested.clone(),
        );
        // What the first run wrote to the config, as `Workspace::load` would hand it back:
        // only the keys specific to the dependency, with the defaults folded in again.
        let mut from_config = RawDependency {
            version: Some("v3.13.1".to_owned()),
            repository: Some(repository.clone()),
            files: requested.clone(),
            ..RawDependency::default()
        };
        from_config.apply_defaults(&block);
        let second = merge_install_entry(from_config, &block, repository, requested);

        assert_eq!(first.vendor_folder, second.vendor_folder);
        assert_eq!(first.files, second.files);
        assert_eq!(first.repository, second.repository);
    }

    #[test]
    fn an_explicit_value_beats_the_default_block() {
        let entry = merge_install_entry(
            RawDependency {
                vendor_folder: Some("./somewhere-else".to_owned()),
                ..RawDependency::default()
            },
            &defaults(r#"{"vendorFolder": "{vendorFolder}"}"#),
            "https://github.com/a/b".to_owned(),
            Some(files(&["LICENSE"])),
        );
        assert_eq!(entry.vendor_folder.as_deref(), Some("./somewhere-else"));
    }

    #[test]
    fn a_configured_repository_url_is_not_rewritten_by_the_shorthand() {
        let entry = merge_install_entry(
            RawDependency {
                repository: Some("https://github.com/mvdan/sh".to_owned()),
                ..RawDependency::default()
            },
            &DefaultOptions::default(),
            "https://www.github.com/mvdan/sh".to_owned(),
            Some(files(&["LICENSE"])),
        );
        assert_eq!(
            entry.repository.as_deref(),
            Some("https://github.com/mvdan/sh")
        );
    }

    #[test]
    fn files_fall_back_to_the_default_block_when_none_are_given() {
        let entry = merge_install_entry(
            RawDependency::default(),
            &defaults(r#"{"files": ["LICENSE"]}"#),
            "https://github.com/a/b".to_owned(),
            None,
        );
        assert_eq!(entry.files, Some(files(&["LICENSE"])));
    }
    // -----------------------------------------------------------------------------------
    // What a neighbouring entry hands over, and what gets said about it
    // -----------------------------------------------------------------------------------

    /// A neighbour: an entry under some other name already vendoring the same repository.
    fn neighbour(version: Option<&str>, list: Option<&[&str]>) -> RawDependency {
        RawDependency {
            version: version.map(str::to_owned),
            repository: Some("https://github.com/a/b".to_owned()),
            files: list.map(files),
            ..RawDependency::default()
        }
    }

    /// What `install` would print, given that neighbour and `--files` state.
    fn warning(existing: &RawDependency, files_given: bool) -> Option<String> {
        Inherited::from_neighbour(existing, files_given).warning(
            "first",
            "second",
            "https://github.com/a/b",
        )
    }

    #[test]
    fn a_neighbour_with_files_and_a_version_hands_over_both() {
        let said = warning(&neighbour(Some("v1.0.0"), Some(&["LICENSE"])), false)
            .expect("both halves were borrowed");
        assert_eq!(
            said,
            "'first' already vendors https://github.com/a/b, so 'second' inherits its files and \
             version. Pass --files to give 'second' its own files; the version comes from 'first' \
             either way."
        );
        // The advice must not promise that `--files` separates the whole entry.
        assert!(!said.contains("separately"), "{said}");
    }

    #[test]
    fn an_explicit_files_list_still_leaves_the_version_borrowed() {
        // The `--files` half is described by the command, but the entry still starts out at the
        // neighbour's version, so the config write can still be skipped.
        let said = warning(&neighbour(Some("v1.0.0"), Some(&["LICENSE"])), true)
            .expect("the version is borrowed regardless");
        assert_eq!(
            said,
            "'first' already vendors https://github.com/a/b, so 'second' inherits its version."
        );
        // Advice that would not help is not given.
        assert!(!said.contains("--files"), "{said}");
    }

    #[test]
    fn a_neighbour_with_no_files_does_not_claim_to_have_supplied_them() {
        // Files come from a known name or the registry here, not from the neighbour.
        let said = warning(&neighbour(Some("v1.0.0"), None), false).expect("the version is still");
        assert!(said.ends_with("inherits its version."), "{said}");
    }

    #[test]
    fn an_empty_files_list_counts_as_none() {
        let said = warning(&neighbour(Some("v1.0.0"), Some(&[])), false).expect("the version");
        assert!(said.ends_with("inherits its version."), "{said}");
    }

    #[test]
    fn a_neighbour_with_no_version_hands_over_only_its_files() {
        let said =
            warning(&neighbour(None, Some(&["LICENSE"])), false).expect("the files are borrowed");
        assert_eq!(
            said,
            "'first' already vendors https://github.com/a/b, so 'second' inherits its files. \
             Pass --files to describe 'second' separately."
        );
    }

    #[test]
    fn an_empty_version_counts_as_none() {
        assert_eq!(warning(&neighbour(Some(""), None), false), None);
    }

    #[test]
    fn a_neighbour_that_hands_over_nothing_is_not_worth_mentioning() {
        // Bare `repository`, and `--files` on the command line: everything about the new entry
        // was decided elsewhere.
        assert_eq!(warning(&neighbour(None, Some(&["LICENSE"])), true), None);
        assert_eq!(warning(&neighbour(None, None), false), None);
    }
}
