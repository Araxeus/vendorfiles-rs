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
    /// The repository's URL.
    repository: String,
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
    let repository = if let Some(known) = known.as_ref() {
        known.repository.to_owned()
    } else if let Some(listed) = listed.as_ref() {
        listed.repository.clone()
    } else if is_github_url(source) {
        source.to_owned()
    } else if is_owner_repo_shorthand(source) {
        format!("https://github.com/{source}")
    } else {
        let found = search_github(session, source, unlisted).await?;
        searched = Some(unlisted);
        found
    };

    Ok(Source {
        repository,
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
        repository,
        known,
        listed,
        searched,
    } = resolve_source(session, source, files.is_some(), preview.refresh).await?;

    if !is_github_url(&repository) {
        bail!(VendorError::InvalidGitHubUrlQuoted(repository));
    }

    let name = name.filter(|n| !n.is_empty());
    let named = name.is_some();
    let name = match name {
        Some(name) => name,
        // A registry entry names itself, so `vendor add rg` keys the entry `ripgrep` - the
        // canonical name, not the alias that was typed.
        None => match listed.as_ref() {
            Some(listed) => listed.name.clone(),
            None => owner_and_name_from_repo_url(&repository)?.name,
        },
    };

    let Base {
        existing,
        neighbour,
    } = Base::find(&session.workspace, &name, &repository, files.is_some());

    // Only `-n` asks for a second entry. Any other name was derived from the source.
    if !named && let Some(neighbour) = neighbour.as_ref() {
        bail!(VendorError::AlreadyVendored {
            repository,
            name: neighbour.name.clone(),
        });
    }

    let files = chosen_files(files, &existing, known.as_ref(), listed.as_ref());
    if files.as_ref().is_none_or(Vec::is_empty) {
        // A searched repository is a guess, so say how it was reached before asking for files.
        if let Some(why) = searched {
            bail!(VendorError::SearchedRepositoryNeedsFiles {
                typed: source.to_owned(),
                why,
                repository: repository.clone(),
            });
        }
        bail!(VendorError::MissingFilesOption);
    }

    // Said only now, once the command is known to have something to install.
    if let Some(warning) = neighbour.and_then(|n| n.warning(&name, &repository)) {
        ui::warning(warning);
    }

    let mut entry = merge_install_entry(existing, &session.workspace.defaults, repository, files);
    if !session.workspace.dependencies.contains_key(&name) {
        fill_from_source(&mut entry, known.as_ref(), listed.as_ref());
    }
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
    /// The entry under this name, else the neighbour's files, else an empty entry.
    existing: RawDependency,
    /// The entry already vendoring this repository, when there is none under this name.
    neighbour: Option<Neighbour>,
}

impl Base {
    fn find(workspace: &Workspace, name: &str, repository: &str, files_given: bool) -> Self {
        if let Some(existing) = workspace.dependencies.get(name) {
            return Self {
                existing: existing.clone(),
                neighbour: None,
            };
        }
        let neighbour = workspace.dependencies.iter().find(|(_, dependency)| {
            dependency
                .repository
                .as_deref()
                .is_some_and(|url| same_repository(url, repository))
        });
        match neighbour {
            // Only the files carry over. The rest places and pins the neighbour's own install.
            Some((key, dependency)) => Self {
                existing: RawDependency {
                    files: dependency.files.clone(),
                    ..RawDependency::default()
                },
                neighbour: Some(Neighbour {
                    name: key.clone(),
                    lends_files: !files_given
                        && dependency.files.as_deref().is_some_and(|f| !f.is_empty()),
                }),
            },
            None => Self {
                existing: RawDependency::default(),
                neighbour: None,
            },
        }
    }
}

/// Whether two GitHub URLs name one repository, however each is spelled.
fn same_repository(a: &str, b: &str) -> bool {
    match (
        owner_and_name_from_repo_url(a),
        owner_and_name_from_repo_url(b),
    ) {
        (Ok(a), Ok(b)) => {
            a.owner.eq_ignore_ascii_case(&b.owner) && a.name.eq_ignore_ascii_case(&b.name)
        }
        _ => false,
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

/// Fills in what a known name or registry entry says about a new entry.
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

/// An entry under a different name that already vendors the same repository.
#[derive(Debug)]
struct Neighbour {
    name: String,
    /// Whether the new entry takes its files from this one.
    lends_files: bool,
}

impl Neighbour {
    /// What to warn about, or `None` when the new entry borrows nothing.
    fn warning(&self, name: &str, repository: &str) -> Option<String> {
        let neighbour = &self.name;
        self.lends_files.then(|| {
            format!(
                "'{neighbour}' already vendors {repository}, so '{name}' inherits its files. \
                 Pass --files to describe '{name}' separately."
            )
        })
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
    use super::{Neighbour, merge_install_entry};
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

    fn neighbour(lends_files: bool) -> Neighbour {
        Neighbour {
            name: "first".to_owned(),
            lends_files,
        }
    }

    #[test]
    fn a_neighbour_lending_files_is_named() {
        assert_eq!(
            neighbour(true).warning("second", "https://github.com/a/b"),
            Some(
                "'first' already vendors https://github.com/a/b, so 'second' inherits its files. \
                 Pass --files to describe 'second' separately."
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_neighbour_lending_nothing_is_not_mentioned() {
        assert_eq!(
            neighbour(false).warning("second", "https://github.com/a/b"),
            None
        );
    }
}
