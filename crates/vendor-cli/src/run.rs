//! Command dispatch — the layer between parsed arguments and the library's operations.

use anyhow::{bail, Result};
use vendorfiles::error::VendorError;
use vendorfiles::model::RawDependency;
use vendorfiles::template::{is_github_url, is_owner_repo_shorthand, owner_and_name_from_repo_url};
use vendorfiles::{auth, GitHubClient, InstallOptions, Session, SyncOptions, Workspace};

use crate::cli::{Cli, Command};

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

    let config_location = cli.config.flatten();
    let workspace = Workspace::load(config_location.as_deref()).await?;
    let github = GitHubClient::new(auth::resolve_token())?;
    let mut session = Session::new(github, workspace);

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

        Command::Update { names, pr } => {
            if names.is_empty() {
                // `--pr` only applies to a full update, matching the reference.
                vendorfiles::ui::set_pr_mode(pr);
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
        } => {
            install(&mut session, &source, version, name.flatten(), files).await?;
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

/// `vendor install <url/name> [version]`.
async fn install(
    session: &mut Session,
    source: &str,
    version: Option<String>,
    name: Option<String>,
    files: Option<Vec<String>>,
) -> Result<()> {
    let url = if is_github_url(source) {
        source.to_owned()
    } else if is_owner_repo_shorthand(source) {
        format!("https://www.github.com/{source}")
    } else {
        session.github.find_repo_url(source).await?
    };

    if !is_github_url(&url) {
        bail!(VendorError::InvalidGitHubUrlQuoted(url));
    }

    let name = match name.filter(|n| !n.is_empty()) {
        Some(name) => name,
        None => owner_and_name_from_repo_url(&url)?.name,
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
                .find(|dependency| dependency.repository.as_deref() == Some(url.as_str()))
        })
        .cloned()
        .unwrap_or_default();

    let files = match files {
        Some(files) => Some(
            files
                .into_iter()
                .map(vendorfiles::FileEntry::Simple)
                .collect(),
        ),
        None => existing.files.clone(),
    };
    if files.as_ref().is_none_or(Vec::is_empty) {
        bail!(VendorError::MissingFilesOption);
    }

    let entry = RawDependency {
        repository: Some(url),
        files,
        ..existing
    };
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
