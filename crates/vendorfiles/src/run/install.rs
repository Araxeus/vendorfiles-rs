//! `vendor install` - resolve sources and add them to the config.

use anyhow::{Result, bail};
use vendorfiles_core::error::VendorError;
use vendorfiles_core::model::{DefaultOptions, HashVersionFile, RawDependency};
use vendorfiles_core::template::{
    is_github_url, is_owner_repo_shorthand, owner_and_name_from_repo_url,
};
use vendorfiles_core::{FileEntry, InstallOptions, Session, Workspace, registry, ui};

use crate::{known, source};

/// What a `source` argument turned out to name.
struct Source {
    /// The URL the reference builds, warts and all - `owner/repo` becomes
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
    /// Why the repository came from a GitHub search, when it did.
    searched: Option<&'static str>,
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
    //
    // A name the registry knows but cannot install here fails outright. Searching GitHub for it
    // would find the same project, with no files to take from it.
    let bare_name = !files_given
        && known.is_none()
        && !is_github_url(source)
        && !is_owner_repo_shorthand(source);
    let (listed, unlisted) = if bare_name {
        look_up_registry(source, refresh).await?
    } else {
        (None, NOT_LISTED)
    };

    let mut searched = None;
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
        let found = search_github(session, source, unlisted).await?;
        searched = Some(unlisted);
        (found.clone(), found)
    };

    Ok(Source {
        lookup,
        stored,
        known,
        listed,
        searched,
    })
}

/// Why a bare name fell through to the GitHub search.
const NOT_LISTED: &str = "is not in the program registry";
const REGISTRY_UNREACHABLE: &str = "could not be looked up in the program registry";

/// Looks `source` up in the hosted registry.
///
/// Returns the entry, if any, and the reason to give if the search after it also misses.
async fn look_up_registry(
    source: &str,
    refresh: bool,
) -> Result<(Option<registry::Entry>, &'static str)> {
    let registry = match registry::load(refresh).await {
        Ok(registry) => registry,
        Err(error) => {
            ui::warning(error);
            return Ok((None, REGISTRY_UNREACHABLE));
        }
    };
    match registry.entry(source) {
        Ok(entry) => {
            if entry.is_none() {
                suggest(&registry, source);
            }
            Ok((entry, NOT_LISTED))
        }
        Err(error) => Err(match instead(&registry, source) {
            Some(others) => anyhow::anyhow!("{error} - try {others} instead"),
            None => error.into(),
        }),
    }
}

/// Searches GitHub for `source`, the last resort for a bare name.
///
/// A miss also says why the registry missed first.
async fn search_github(session: &Session, source: &str, unlisted: &'static str) -> Result<String> {
    let found = session
        .github
        .find_repo_url(source)
        .await
        .map_err(|error| match error {
            VendorError::NoSearchResults(_) => VendorError::NotFoundAnywhere {
                typed: source.to_owned(),
                why: unlisted,
            },
            VendorError::NoSearchResultsDidYouMean { suggestion, .. } => {
                VendorError::NotFoundAnywhereDidYouMean {
                    typed: source.to_owned(),
                    why: unlisted,
                    suggestion,
                }
            }
            other => other,
        })?;
    Ok(found)
}

/// Names registry programs close to `typed`, which the registry does not know.
///
/// Only a hint: the GitHub search still runs afterwards, since `typed` may well be a repository.
fn suggest(registry: &registry::Registry, typed: &str) {
    let close = registry::search::suggest(registry, typed);
    let Some((last, rest)) = close.split_last() else {
        return;
    };
    let names = if rest.is_empty() {
        (*last).to_owned()
    } else {
        format!("{} or {last}", rest.join(", "))
    };
    ui::info(format!(
        "'{typed}' is not in the program registry - did you mean {names}? `vendor search` lists them all"
    ));
}

/// Registry programs that do the job of `typed` and install on this host, joined for a hint.
///
/// `tmux` has no Windows build, but `psmux` mentions tmux and has one.
fn instead(registry: &registry::Registry, typed: &str) -> Option<String> {
    let (canonical, _) = registry.find(typed)?;
    let host = registry::resolve::host();
    let others: Vec<&str> = registry::search::search(registry, canonical)
        .into_iter()
        .filter(|(name, program)| *name != canonical && program.covers(&host))
        .map(|(name, _)| name)
        .take(3)
        .collect();
    (!others.is_empty()).then(|| others.join(" or "))
}

/// How much of an install to actually perform.
#[derive(Clone, Copy)]
pub(super) struct Preview {
    /// Re-check the registry rather than trusting the cached copy.
    pub(super) refresh: bool,
    /// Resolve and report, but download nothing and write nothing.
    pub(super) dry_run: bool,
}

/// `vendor install <url/name...>` - every operand in turn.
///
/// Sequential and fail-fast, like the `update <names>` and `uninstall <names>` loops: an install
/// writes the config as it goes, so stopping at the first failure leaves a config that says
/// exactly which sources got as far as being installed.
///
/// Both argument checks below run before the first install, so a mistake never lands
/// half a run.
pub(super) async fn install_all(
    session: &mut Session,
    sources: &[String],
    name: Option<String>,
    files: Option<Vec<String>>,
    preview: Preview,
) -> Result<()> {
    // `-n` names one entry and `-f` describes one entry's files. Spreading either across
    // several sources would either collide or quietly give unrelated repositories the same file
    // list, so neither is guessed at.
    if sources.len() > 1 {
        if name.is_some() {
            bail!(VendorError::SingleSourceOption("-n or --name"));
        }
        if files.is_some() {
            bail!(VendorError::SingleSourceOption("-f or --files"));
        }
    }

    let operands: Vec<(&str, Option<&str>)> =
        sources.iter().map(|arg| source::split(arg)).collect();

    // The reference's second operand was a version. Every operand is a source now, so without
    // this `vendor add owner/repo v1.0.0` would search GitHub for a repository named `v1.0.0`.
    // Only after the first operand: a lone version-shaped source has nothing to be attached to,
    // and a repository really could be named that.
    for (index, (candidate, version)) in operands.iter().enumerate().skip(1) {
        if version.is_none() && source::looks_like_bare_version(candidate) {
            bail!(VendorError::VersionAsSource {
                version: (*candidate).to_owned(),
                suggestion: format!("{}@{candidate}", operands[index - 1].0),
            });
        }
    }

    for (source, version) in operands {
        install(
            session,
            source,
            version.map(str::to_owned),
            name.clone(),
            files.clone(),
            preview,
        )
        .await?;
    }
    Ok(())
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
        searched,
    } = resolve_source(session, source, files.is_some(), preview.refresh).await?;

    if !is_github_url(&lookup) {
        bail!(VendorError::InvalidGitHubUrlQuoted(lookup));
    }

    let name = name.filter(|n| !n.is_empty());
    let named = name.is_some();
    let name = match name {
        Some(name) => name,
        // A registry entry names itself, so `vendor add rg` keys the entry `ripgrep` - the
        // canonical name, not the alias that was typed.
        None => match listed.as_ref() {
            Some(listed) => listed.name.clone(),
            None => owner_and_name_from_repo_url(&lookup)?.name,
        },
    };

    let Base {
        existing,
        neighbour,
    } = Base::find(&session.workspace, &name, &lookup, files.is_some());

    // Only `-n` asks for a second entry. Any other name was derived from the source.
    if !named && let Some((holder, _)) = neighbour.as_ref() {
        bail!(VendorError::AlreadyVendored {
            repository: stored,
            name: holder.clone(),
        });
    }

    let files = chosen_files(files, &existing, known.as_ref(), listed.as_ref());
    if files.as_ref().is_none_or(Vec::is_empty) {
        // A searched repository is a guess, so say how it was reached before asking for files.
        if let Some(why) = searched {
            bail!(VendorError::SearchedRepositoryNeedsFiles {
                typed: source.to_owned(),
                why,
                repository: lookup.clone(),
            });
        }
        bail!(VendorError::MissingFilesOption);
    }

    // Say what the neighbour handed over, now that the whole precedence chain has run and the
    // command is known to have something to install. Borrowing from an entry the user never
    // named is the surprising half of `install`, and the borrowed `version` is the sharp edge:
    // the new entry starts out claiming it, so when it already matches what gets installed the
    // entry is never written at all - which looks like nothing happening for no reason.
    if let Some((neighbour, inherited)) = neighbour
        && let Some(warning) = inherited.warning(&neighbour, &name, &lookup)
    {
        ui::warning(warning);
    }

    let mut entry = merge_install_entry(existing, &session.workspace.defaults, stored, files);
    fill_from_source(&mut entry, known.as_ref(), listed.as_ref());
    let dependency = entry.resolve(&name)?;

    if preview.dry_run {
        return report_entry(session, &name, &entry, version.as_deref());
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

/// The config entry a new install builds on.
///
/// Files may be inherited from an entry under this name, or from any entry pointing at the same
/// repository - the reference looks in both places.
struct Base {
    /// The entry under this name, else the neighbour, else an empty entry.
    existing: RawDependency,
    /// The neighbour's name and what it hands over, when there is no entry under this name.
    /// Inheriting from an entry the user did not name is the surprising half, so it is kept apart.
    neighbour: Option<(String, Inherited)>,
}

impl Base {
    fn find(workspace: &Workspace, name: &str, repository: &str, files_given: bool) -> Self {
        if let Some(existing) = workspace.dependencies.get(name) {
            return Self {
                existing: existing.clone(),
                neighbour: None,
            };
        }
        let neighbour = workspace
            .dependencies
            .iter()
            .find(|(_, dependency)| dependency.repository.as_deref() == Some(repository));
        match neighbour {
            Some((key, dependency)) => Self {
                existing: dependency.clone(),
                neighbour: Some((
                    key.clone(),
                    Inherited::from_neighbour(dependency, files_given),
                )),
            },
            None => Self {
                existing: RawDependency::default(),
                neighbour: None,
            },
        }
    }
}

/// The files the new entry lists, in precedence order.
///
/// `--files` wins. Otherwise an entry already in the config describes itself, and failing that a
/// known name or the registry does.
fn chosen_files(
    given: Option<Vec<String>>,
    existing: &RawDependency,
    known: Option<&known::Known>,
    listed: Option<&registry::Entry>,
) -> Option<Vec<FileEntry>> {
    if let Some(given) = given {
        return Some(given.into_iter().map(FileEntry::Simple).collect());
    }
    existing
        .files
        .clone()
        .or_else(|| known.map(|known| known.files.clone()))
        .or_else(|| listed.map(|listed| listed.files.clone()))
}

/// Fills in what a known name or registry entry says and the config does not yet.
fn fill_from_source(
    entry: &mut RawDependency,
    known: Option<&known::Known>,
    listed: Option<&registry::Entry>,
) {
    // Where a known dependency belongs, unless the config already says.
    if entry.vendor_folder.is_none()
        && let Some(known) = known
    {
        entry.vendor_folder.clone_from(&known.folder);
    }
    if let Some(listed) = listed {
        if entry.release_regex.is_none() {
            entry.release_regex.clone_from(&listed.release_regex);
        }
        if entry.hash_version_file.is_none() && listed.hash_version_file == Some(true) {
            entry.hash_version_file = Some(HashVersionFile::Flag(true));
        }
    }
}

/// What a new entry takes from a *neighbour* - an entry under a different name that already
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
///   version on the command line does not stop this - it decides what to install, not what the
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
            // "separately" would promise more than it delivers, since the version - the half that
            // can swallow the config write - comes from the neighbour whatever the command says.
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
/// this put in my config" instantly - and what lets the tests cover the registry without a
/// network. The version is left to the install, being the one part that has to ask GitHub.
fn report_entry(
    session: &Session,
    name: &str,
    entry: &RawDependency,
    pinned: Option<&str>,
) -> Result<()> {
    // A `source@version` pin is what the entry would end up carrying, so the report says so
    // rather than showing whatever the config had. Only here: giving the real install path an
    // entry already at the requested version would make it look up to date and skip the work.
    let pinned = pinned.map(|version| RawDependency {
        version: Some(version.to_owned()),
        ..entry.clone()
    });
    let entry = pinned.as_ref().unwrap_or(entry);

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
