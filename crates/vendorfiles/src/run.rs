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
        } => {
            install(
                &mut session,
                &source,
                version,
                name.flatten(),
                files,
                refresh,
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

/// `vendor install <url/name> [version]`.
async fn install(
    session: &mut Session,
    source: &str,
    version: Option<String>,
    name: Option<String>,
    files: Option<Vec<String>>,
    refresh: bool,
) -> Result<()> {
    // `lookup` is the URL the reference builds, warts and all — `owner/repo` becomes
    // `https://www.github.com/owner/repo`, and that exact string is what it compares against
    // config entries below. `stored` is the same repository without the `www.`, which is the
    // form every documented example uses and the form the search path already returns.
    // A name this tool knows describes itself, so neither a search nor `--files` is needed.
    // Anything explicit the user passed still wins.
    let known = if files.is_none() {
        known::find(source)
    } else {
        None
    };

    // Then the hosted registry, which is what makes `vendor add fd` work without a repository or
    // a `--files` list. A registry that cannot be reached is a miss, not a failure: the search
    // below still works, and so does `vendor add owner/repo`.
    let bare_name = files.is_none()
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

    if !is_github_url(&lookup) {
        bail!(VendorError::InvalidGitHubUrlQuoted(lookup));
    }

    let name = match name.filter(|n| !n.is_empty()) {
        Some(name) => name,
        // A registry entry names itself, so `vendor add rg` keys the entry `rg` rather than
        // `ripgrep`.
        None => match listed.as_ref() {
            Some(listed) => listed.name.clone(),
            None => owner_and_name_from_repo_url(&lookup)?.name,
        },
    };

    // Files may be inherited from an entry under this name, or from any entry pointing at the
    // same repository — the reference looks in both places.
    let existing = session
        .workspace
        .dependencies
        .get(&name)
        .or_else(|| {
            session
                .workspace
                .dependencies
                .values()
                .find(|dependency| dependency.repository.as_deref() == Some(lookup.as_str()))
        })
        .cloned()
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

    let mut entry = merge_install_entry(existing, &session.workspace.defaults, stored, files);
    // Where a known dependency belongs, unless the config already says.
    if entry.vendor_folder.is_none()
        && let Some(known) = known.as_ref()
    {
        entry.vendor_folder = known.folder.clone();
    }
    let dependency = entry.resolve(&name)?;

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
    use super::merge_install_entry;
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
}
