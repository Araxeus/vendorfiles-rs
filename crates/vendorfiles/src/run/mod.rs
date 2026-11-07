//! Command dispatch - the layer between parsed arguments and the library's operations.

mod completions;
mod config;
mod install;
mod search;

use anyhow::{Result, bail};
use vendorfiles_core::error::VendorError;
use vendorfiles_core::progress::Reporter;
use vendorfiles_core::{GitHubClient, InstallOptions, Session, SyncOptions, Workspace, auth};

use crate::cli::{Cli, Command, ConfigCommand};
use completions::completions;
use config::{edit_config, list_dependencies};
use install::{Preview, install_all};
use search::search;

/// Answers the commands that need no config file, or only its path.
///
/// `Some` means the command was dealt with here; `None` hands it on to the workspace path.
async fn dispatch_without_workspace(cli: &Cli) -> Option<Result<()>> {
    Some(match &cli.command {
        // A completion script has nothing to do with a project, so asking for one outside a
        // project should not fail.
        Command::Completions { shell } => completions(shell).await,

        // A config that no longer loads is exactly when its path is worth asking for, and
        // `config edit` is how it gets repaired - so neither of these parses the file.
        Command::Config { command: None } => match Workspace::locate(cli.config.as_deref()).await {
            Ok(path) => {
                println!("{}", path.display());
                Ok(())
            }
            Err(error) => Err(error.into()),
        },
        Command::Config {
            command: Some(ConfigCommand::Edit { editor }),
        } => match Workspace::locate(cli.config.as_deref()).await {
            Ok(path) => edit_config(&path, editor.as_deref()),
            Err(error) => Err(error.into()),
        },

        // Authenticating is not a project-scoped action; the reference's blanket `preAction`
        // hook made `vendor login` fail outside a project.
        // No GitHub token is the word `status`, so `vendor auth status` cannot shadow a login.
        Command::Login { token } => match token.as_deref() {
            Some("status") => auth::status().await.map_err(Into::into),
            Some(token) => auth::login_with_token(token).await.map_err(Into::into),
            None => auth::login_with_device_flow().await.map_err(Into::into),
        },
        Command::Logout => auth::logout().await.map_err(Into::into),

        // The registry is not the project's, so finding a program to add needs no config.
        Command::Search { terms, refresh } => search(&terms.join(" "), *refresh).await,
        Command::Install {
            list: true,
            sources,
            refresh,
            ..
        } => search(&sources.join(" "), *refresh).await,

        _ => return None,
    })
}

/// Loads the workspace and runs the requested command.
pub async fn dispatch(cli: Cli) -> Result<()> {
    if let Some(result) = dispatch_without_workspace(&cli).await {
        return result;
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

    // Listing reads the file and nothing else, so it stops here rather than paying for a
    // credential lookup and a display it would never draw on.
    if matches!(
        cli.command,
        Command::List
            | Command::Config {
                command: Some(ConfigCommand::List)
            }
    ) {
        list_dependencies(&workspace);
        return Ok(());
    }

    let github = GitHubClient::new(auth::resolve_token_async().await)?;
    // `--plain` asks for the output a pipe would get: no region, just the lines.
    let mut session = if cli.plain {
        Session::with_reporter(github, workspace, Reporter::new(false))
    } else {
        Session::new(github, workspace)
    };

    let outcome = run_command(&mut session, cli.command).await;

    let already_said = outcome
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<VendorError>())
        .is_some_and(|error| matches!(error, VendorError::RateLimited(_)));
    if !already_said {
        session.github.report_quota().await;
    }
    outcome
}

async fn run_command(session: &mut Session, command: Command) -> Result<()> {
    match command {
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
                    upgrade_one(session, &name).await?;
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
            sources,
            name,
            files,
            refresh,
            dry_run,
            list: _,
        } => {
            install_all(
                session,
                &sources,
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

        // Handled above, either before or straight after the config is read.
        Command::List | Command::Config { .. } => {
            unreachable!("config and list are dispatched without a session")
        }
        Command::Login { .. } | Command::Logout => {
            unreachable!("login and logout are dispatched without a workspace")
        }
        Command::Completions { .. } | Command::Search { .. } => {
            unreachable!("completions and search are dispatched without a workspace")
        }
    }

    Ok(())
}

/// `vendor update <name>` - re-resolve one dependency to its latest version.
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
