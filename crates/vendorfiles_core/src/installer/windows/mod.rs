//! Running installers and uninstallers on Windows.

mod arp;
mod shell;

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::{Installed, InstallerKind, InstallerOptions, Outcome, Scope, msix_identity};
use crate::error::{Result, VendorError};

/// How long an uninstall may take to remove its Apps & Features entry after the uninstaller
/// exits. NSIS uninstallers copy themselves to a temporary folder and return at once.
const UNINSTALL_SETTLE: Duration = Duration::from_mins(2);

/// Inno Setup's switches for a run with no UI at all.
const INNO_SILENT: &str = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-";
const INNO_UNINSTALL_SILENT: &str = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART";

/// A program to start, with its whole command line.
struct Launch {
    program: OsString,
    params: String,
    elevate: bool,
}

pub fn install(
    name: &str,
    path: &Path,
    version: &str,
    reinstall: bool,
    kind: InstallerKind,
    options: &InstallerOptions,
    previous: Option<&Installed>,
) -> Result<Outcome> {
    if kind == InstallerKind::Msix {
        return install_msix(name, path);
    }
    let previous_key = previous.and_then(|p| p.uninstall_key.as_deref());
    let before = arp::snapshot();
    let existing = arp::existing(name, &before, previous_key);

    if !reinstall
        && let Some(entry) = &existing
        && is_installed_release(
            entry,
            kind,
            options,
            version,
            entry
                .program()
                .and_then(|program| shell::file_version(&program))
                .map(dotted)
                .as_deref(),
        )
    {
        return Ok(Outcome {
            installed: Installed {
                kind,
                uninstall_key: Some(entry.key.clone()),
                display_name: entry.name.clone(),
                package: None,
            },
            reboot: false,
            ran: false,
        });
    }

    // An installer cannot replace files a running program holds open, and often reports success
    // anyway.
    let running = existing
        .as_ref()
        .and_then(arp::Entry::folder)
        .map(|folder| running_from(&folder))
        .unwrap_or_default();
    if !running.is_empty() {
        let program = existing
            .as_ref()
            .and_then(|entry| entry.name.clone())
            .unwrap_or_else(|| name.to_owned());
        if options.relaunch != Some(true) {
            return Err(VendorError::InstallerBlocked {
                name: name.to_owned(),
                program,
            });
        }
        close(name, &program, &running)?;
    }

    let outcome = run_install(
        name,
        path,
        version,
        kind,
        options,
        &before,
        existing.as_ref(),
        previous_key,
    );
    reopen(&running);
    outcome
}

/// Runs the installer, then checks that the program it replaced really changed.
#[expect(
    clippy::too_many_arguments,
    reason = "the pieces of one install, each read once"
)]
fn run_install(
    name: &str,
    path: &Path,
    version: &str,
    kind: InstallerKind,
    options: &InstallerOptions,
    before: &HashMap<String, arp::Entry>,
    existing: Option<&arp::Entry>,
    previous_key: Option<&str>,
) -> Result<Outcome> {
    let program_before = existing
        .and_then(arp::Entry::program)
        .and_then(|program| shell::file_version(&program));
    let launch = installer_launch(path, kind, options);
    let code = run(name, &launch)?;
    let reboot = check_exit(name, kind, code)?;
    let after = arp::snapshot();
    let entry = arp::pick(name, before, &after, previous_key);

    if let Some(entry) = &entry {
        let program_after = entry
            .program()
            .and_then(|program| shell::file_version(&program));
        let unchanged = before.get(&entry.key) == Some(entry);
        if let Some(stale) = left_behind(
            program_before.map(dotted),
            program_after.map(dotted),
            existing.and_then(|existing| existing.version.as_deref()),
            unchanged.then_some(entry.version.as_deref()).flatten(),
            version,
        ) {
            return Err(VendorError::InstallerError {
                name: name.to_owned(),
                message: format!(
                    "the installer reported success, but {} is still version {stale}",
                    entry.name.as_deref().unwrap_or(name)
                ),
            });
        }
    }
    Ok(Outcome {
        installed: Installed {
            kind,
            uninstall_key: entry.as_ref().map(|e| e.key.clone()),
            display_name: entry.and_then(|e| e.name),
            package: None,
        },
        reboot,
        ran: true,
    })
}

/// Whether the entry already holds this release, installed the way the dependency asks.
///
/// Every check has to pass, and any doubt runs the installer. The entry and the program's own
/// file version, when it has one, must both name the release. Its key must have the shape the
/// installer type writes, so the uninstaller vendor picks later fits it. A requested scope must
/// match the hive. Extra `args` always run the installer, since they may install something the
/// existing copy lacks.
fn is_installed_release(
    entry: &arp::Entry,
    kind: InstallerKind,
    options: &InstallerOptions,
    release: &str,
    program_version: Option<&str>,
) -> bool {
    let is_release = |found: &str| super::is_release_version(found, release) == Some(true);
    let id = entry.id();
    let shaped = match kind {
        InstallerKind::Inno => id.ends_with("_is1"),
        InstallerKind::Msi => id.starts_with('{'),
        InstallerKind::Nsis => !id.ends_with("_is1") && !id.starts_with('{'),
        InstallerKind::Exe => true,
        InstallerKind::Msix => false,
    };
    let scoped = match options.scope {
        Some(Scope::User) => !entry.is_machine_wide(),
        Some(Scope::Machine) => entry.is_machine_wide(),
        None => true,
    };
    options.args.is_empty()
        && shaped
        && scoped
        && entry.version.as_deref().is_some_and(is_release)
        && program_version.is_none_or(is_release)
}

/// The old version an install left in place, if it left one.
///
/// The program's own file version decides when it agreed with the Apps & Features entry before
/// the install: unchanged and not the release means nothing was replaced. Otherwise an entry that
/// did not change and names another version means the same.
fn left_behind(
    program_before: Option<String>,
    program_after: Option<String>,
    listed_before: Option<&str>,
    unchanged_entry: Option<&str>,
    release: &str,
) -> Option<String> {
    let is_old = |found: &str| super::is_release_version(found, release) == Some(false);
    let tracked = program_before.filter(|program| {
        listed_before.is_some_and(|listed| super::is_release_version(program, listed) == Some(true))
    });
    if let (Some(before), Some(after)) = (tracked, program_after) {
        return (before == after && is_old(&after)).then_some(after);
    }
    unchanged_entry
        .filter(|version| is_old(version))
        .map(str::to_owned)
}

fn dotted(version: [u16; 4]) -> String {
    version.map(|part| part.to_string()).join(".")
}

/// A process running from a program's install folder.
struct Running {
    pid: u32,
    parent: u32,
    image: PathBuf,
}

/// Every process, other than this one, that runs from inside `folder`.
fn running_from(folder: &Path) -> Vec<Running> {
    let lower = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
    let folder = lower(&std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_owned()));
    let own = std::process::id();
    shell::processes()
        .into_iter()
        .filter(|&(pid, _)| pid != own)
        .filter_map(|(pid, parent)| {
            let image = shell::image_path(pid)?;
            let resolved = std::fs::canonicalize(&image).unwrap_or_else(|_| image.clone());
            lower(&resolved)
                .starts_with(&folder)
                .then_some(Running { pid, parent, image })
        })
        .collect()
}

/// How long a program gets to quit before the install gives up on it.
const CLOSE_LIMIT: Duration = Duration::from_secs(30);

/// Closes the running processes and waits until every one has exited.
fn close(name: &str, program: &str, running: &[Running]) -> Result<()> {
    let failed = |reason: String| VendorError::InstallerError {
        name: name.to_owned(),
        message: format!("could not close {program}: {reason}"),
    };
    let pids: Vec<u32> = running.iter().map(|process| process.pid).collect();
    shell::close(&pids).map_err(|error| failed(error.to_string()))?;
    let started = Instant::now();
    while shell::processes().iter().any(|(pid, _)| pids.contains(pid)) {
        if started.elapsed() > CLOSE_LIMIT {
            return Err(failed("it is still running".to_owned()));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(())
}

/// Starts again the programs [`close`] closed. Child processes are left for their parent to start.
fn reopen(running: &[Running]) {
    let pids: HashSet<u32> = running.iter().map(|process| process.pid).collect();
    let mut started = HashSet::new();
    for process in running
        .iter()
        .filter(|process| !pids.contains(&process.parent))
    {
        if !started.insert(process.image.clone()) {
            continue;
        }
        if let Err(error) = shell::start(&process.image) {
            crate::ui::warning(format!(
                "Could not start {} again: {error}",
                process.image.display()
            ));
        }
    }
}

/// The command that runs an installer of `kind` silently.
fn installer_launch(path: &Path, kind: InstallerKind, options: &InstallerOptions) -> Launch {
    let mut params: Vec<String> = Vec::new();
    let mut program = path.as_os_str().to_owned();
    let mut elevate = false;
    match kind {
        InstallerKind::Nsis => params.push("/S".to_owned()),
        InstallerKind::Inno => {
            params.push(INNO_SILENT.to_owned());
            match options.scope {
                Some(Scope::User) => params.push("/CURRENTUSER".to_owned()),
                Some(Scope::Machine) => params.push("/ALLUSERS".to_owned()),
                None => {}
            }
        }
        InstallerKind::Msi => {
            program = OsString::from("msiexec.exe");
            params.push(format!(
                "/i {} /qn /norestart",
                shell::quote(&path.to_string_lossy())
            ));
            match options.scope {
                Some(Scope::User) => params.push("ALLUSERS=2 MSIINSTALLPERUSER=1".to_owned()),
                Some(Scope::Machine) => params.push("ALLUSERS=1".to_owned()),
                None => {}
            }
            // With no UI, msiexec cannot raise a UAC prompt of its own, so a per-machine
            // package would fail. A per-user one is asked for without admin.
            elevate = options.scope != Some(Scope::User);
        }
        InstallerKind::Exe | InstallerKind::Msix => {}
    }
    params.extend(options.args.iter().cloned());
    Launch {
        program,
        params: params.join(" "),
        elevate,
    }
}

/// How long to wait for an uninstaller's temporary copy, which may be asking the user something.
const SECOND_PHASE_LIMIT: Duration = Duration::from_mins(30);

/// Starts a launch and waits for it, turning a declined UAC prompt into its own error.
fn run(name: &str, launch: &Launch) -> Result<u32> {
    shell::run(&launch.program, &launch.params, launch.elevate).map_err(|error| {
        if error.raw_os_error() == Some(shell::ERROR_CANCELLED) {
            VendorError::InstallerDeclined {
                name: name.to_owned(),
            }
        } else {
            VendorError::InstallerError {
                name: name.to_owned(),
                message: format!(
                    "could not start {}: {error}",
                    Path::new(&launch.program).display()
                ),
            }
        }
    })
}

/// Starts a launch and waits for it and for any copy of itself it started from the temporary
/// folder, returning its exit code.
///
/// Inno and NSIS uninstallers copy themselves to the temporary folder, start the copy and exit
/// at once, so the program they remove is still going when the first process ends. The copy may
/// stop to ask the user something the silent switches cannot suppress. Elevation relaunches the
/// uninstaller in between, so the copy's parent is often gone by then. Every process that started
/// during the launch and runs from the temporary folder is waited for instead. Anything else, such
/// as the app itself, is not.
fn run_through(name: &str, launch: &Launch) -> Result<u32> {
    let before: HashSet<u32> = shell::processes().into_iter().map(|(pid, _)| pid).collect();
    let code = run(name, launch)?;
    // Canonical, because `TEMP` often holds an 8.3 name like `ORBENM~1` and image paths never do.
    let temp = std::env::temp_dir();
    let temp = std::fs::canonicalize(&temp).unwrap_or(temp);
    let started = Instant::now();
    while started.elapsed() < SECOND_PHASE_LIMIT && started_from(&temp, &before) {
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok(code)
}

/// Whether a process that is not in `before` runs from inside `temp`.
fn started_from(temp: &Path, before: &HashSet<u32>) -> bool {
    shell::processes()
        .into_iter()
        .filter(|(pid, _)| !before.contains(pid))
        .filter_map(|(pid, _)| shell::image_path(pid))
        .any(|image| {
            std::fs::canonicalize(&image)
                .unwrap_or(image)
                .starts_with(temp)
        })
}

/// Whether an exit code means success, and if so whether a restart is pending.
///
/// # Errors
///
/// Returns [`VendorError::InstallerFailed`] for every other code, explained where the code is a
/// documented one.
fn check_exit(name: &str, kind: InstallerKind, code: u32) -> Result<bool> {
    match code {
        0 => return Ok(false),
        // ERROR_SUCCESS_REBOOT_REQUIRED and ERROR_SUCCESS_REBOOT_INITIATED.
        3010 | 1641 => return Ok(true),
        _ => {}
    }
    let reason = match (kind, code) {
        (InstallerKind::Inno, 2 | 5) | (InstallerKind::Nsis, 1) | (_, 1602) => {
            "the install was cancelled"
        }
        (InstallerKind::Nsis, 2) => "the installer stopped itself",
        (InstallerKind::Inno, 8) => "a restart is needed before it can install",
        (_, 1618) => "another installation is in progress - wait for it and try again",
        (_, 1603) => "Windows Installer reported a fatal error",
        (_, 1625) => "a system policy forbids this install",
        (_, 1638) => "another version of this product is already installed",
        (_, 1633) => "this package is for a different processor",
        _ => "",
    };
    Err(VendorError::InstallerFailed {
        name: name.to_owned(),
        kind,
        code,
        reason: reason.to_owned(),
    })
}

pub fn is_present(installed: &Installed) -> bool {
    if let Some(package) = &installed.package {
        return msix_present(package);
    }
    installed
        .uninstall_key
        .as_deref()
        .is_none_or(|key| arp::read(key).is_some())
}

pub fn uninstall(name: &str, installed: &Installed) -> Result<()> {
    if let Some(package) = &installed.package {
        return uninstall_msix(name, package);
    }
    let Some(key) = installed.uninstall_key.as_deref() else {
        return Err(VendorError::UninstallFailed {
            name: name.to_owned(),
            message: "vendor found no Apps & Features entry when it installed it, so remove it \
                      from Settings > Apps, then run this again"
                .to_owned(),
        });
    };
    let Some(entry) = arp::read(key) else {
        return Ok(());
    };
    let launch = uninstaller_launch(name, installed.kind, &entry)?;
    let code = run_through(name, &launch)?;
    if !matches!(code, 0 | 3010 | 1641) {
        return Err(VendorError::UninstallFailed {
            name: name.to_owned(),
            message: format!("the uninstaller exited with code {code}"),
        });
    }
    let started = Instant::now();
    while arp::read(key).is_some() {
        if started.elapsed() > UNINSTALL_SETTLE {
            return Err(VendorError::UninstallFailed {
                name: name.to_owned(),
                message: format!(
                    "the uninstaller finished but {} is still installed",
                    entry.name.as_deref().unwrap_or(name)
                ),
            });
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

/// The command that removes an Apps & Features entry without asking anything.
fn uninstaller_launch(name: &str, kind: InstallerKind, entry: &arp::Entry) -> Result<Launch> {
    let msi_product = entry.id().starts_with('{').then(|| entry.id().to_owned());
    if let Some(product) = msi_product.filter(|_| kind == InstallerKind::Msi || is_msiexec(entry)) {
        return Ok(Launch {
            program: OsString::from("msiexec.exe"),
            params: format!("/x {product} /qn /norestart"),
            elevate: entry.is_machine_wide(),
        });
    }
    // vendor knows the quietest switches for these two, which beat the `/SILENT` an installer
    // often writes as its own quiet command.
    let known = matches!(kind, InstallerKind::Inno | InstallerKind::Nsis);
    if !known && let Some(quiet) = entry.quiet_uninstall.as_deref() {
        let (program, params) = split_command(quiet);
        return Ok(Launch {
            program,
            params,
            elevate: false,
        });
    }
    let Some(command) = entry.uninstall.as_deref() else {
        return Err(VendorError::UninstallFailed {
            name: name.to_owned(),
            message: "its Apps & Features entry names no uninstaller".to_owned(),
        });
    };
    let (program, mut params) = split_command(command);
    let silent = match kind {
        InstallerKind::Inno => INNO_UNINSTALL_SILENT,
        InstallerKind::Nsis => "/S",
        InstallerKind::Msi | InstallerKind::Msix | InstallerKind::Exe => "",
    };
    if !silent.is_empty() {
        if !params.is_empty() {
            params.push(' ');
        }
        params.push_str(silent);
    }
    Ok(Launch {
        program,
        params,
        elevate: false,
    })
}

fn is_msiexec(entry: &arp::Entry) -> bool {
    entry
        .uninstall
        .as_deref()
        .is_some_and(|command| command.to_ascii_lowercase().contains("msiexec"))
}

/// Splits a registry command line into the program and the rest.
///
/// The program is either quoted or runs up to its `.exe`, since installers often write a path with
/// spaces and no quotes.
fn split_command(command: &str) -> (OsString, String) {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return (
            OsString::from(&rest[..end]),
            rest[end + 1..].trim().to_owned(),
        );
    }
    let lower = command.to_ascii_lowercase();
    if let Some(end) = lower.find(".exe") {
        let end = end + ".exe".len();
        return (
            OsString::from(&command[..end]),
            command[end..].trim().to_owned(),
        );
    }
    match command.split_once(' ') {
        Some((program, rest)) => (OsString::from(program), rest.trim().to_owned()),
        None => (OsString::from(command), String::new()),
    }
}

/// Where Windows registers the MSIX packages installed for the current user.
const MSIX_PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

fn install_msix(name: &str, path: &Path) -> Result<Outcome> {
    let package = msix_identity(path).ok_or_else(|| VendorError::InstallerError {
        name: name.to_owned(),
        message: "the package has no identity in its manifest".to_owned(),
    })?;
    powershell(
        name,
        &format!(
            "Add-AppxPackage -Path {} -ForceApplicationShutdown",
            ps_quote(&path.to_string_lossy())
        ),
    )?;
    Ok(Outcome {
        installed: Installed {
            kind: InstallerKind::Msix,
            uninstall_key: None,
            display_name: None,
            package: Some(package),
        },
        reboot: false,
        ran: true,
    })
}

fn uninstall_msix(name: &str, package: &str) -> Result<()> {
    if !msix_present(package) {
        return Ok(());
    }
    powershell(
        name,
        &format!(
            "Get-AppxPackage -Name {} | Remove-AppxPackage",
            ps_quote(package)
        ),
    )
    .map_err(|error| VendorError::UninstallFailed {
        name: name.to_owned(),
        message: error.to_string(),
    })
}

fn msix_present(package: &str) -> bool {
    let prefix = format!("{package}_");
    windows_registry::CURRENT_USER
        .open(MSIX_PACKAGES)
        .ok()
        .and_then(|root| {
            root.keys()
                .ok()
                .map(|mut names| names.any(|n| n.starts_with(&prefix)))
        })
        .unwrap_or(false)
}

/// Runs one Windows PowerShell command, where the Appx cmdlets live.
fn powershell(name: &str, command: &str) -> Result<()> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(OsStr::new(&format!(
            "$ErrorActionPreference = 'Stop'; {command}"
        )))
        .output()
        .map_err(|error| VendorError::InstallerError {
            name: name.to_owned(),
            message: format!("could not start PowerShell: {error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("PowerShell reported a failure")
        .to_owned();
    Err(VendorError::InstallerError {
        name: name.to_owned(),
        message,
    })
}

/// A PowerShell single-quoted string, in which only `'` needs escaping.
fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        InstallerKind, InstallerOptions, Scope, check_exit, installer_launch, split_command,
    };

    #[test]
    fn each_kind_runs_with_its_silent_switches() {
        let path = Path::new(r"C:\Temp dir\setup.exe");
        let launch = |kind, options: &InstallerOptions| {
            let launch = installer_launch(path, kind, options);
            (launch.program, launch.params, launch.elevate)
        };
        let plain = InstallerOptions::default();
        assert_eq!(
            launch(InstallerKind::Nsis, &plain),
            (path.as_os_str().to_owned(), "/S".to_owned(), false)
        );
        let user = InstallerOptions {
            scope: Some(Scope::User),
            args: vec!["/MERGETASKS=!desktopicon".to_owned()],
            ..InstallerOptions::default()
        };
        assert_eq!(
            launch(InstallerKind::Inno, &user).1,
            "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /CURRENTUSER /MERGETASKS=!desktopicon"
        );
        let (program, params, elevate) = launch(InstallerKind::Msi, &plain);
        assert_eq!(program, "msiexec.exe");
        assert_eq!(params, r#"/i "C:\Temp dir\setup.exe" /qn /norestart"#);
        assert!(elevate);
        assert!(!launch(InstallerKind::Msi, &user).2);
        assert_eq!(launch(InstallerKind::Exe, &plain).1, "");
    }

    #[test]
    fn only_a_clear_match_skips_the_installer() {
        use super::{InstallerKind as Kind, Scope, arp::Entry, is_installed_release};
        let entry = |key: &str, version: &str| Entry {
            key: key.to_owned(),
            name: Some("qBittorrent".to_owned()),
            version: Some(version.to_owned()),
            uninstall: None,
            quiet_uninstall: None,
            install_location: None,
            display_icon: None,
            system: false,
        };
        let plain = InstallerOptions::default();
        let release = "release-5.3.1";
        let installed = entry(r"HKLM\x\qBittorrent", "5.3.1");
        let check = |entry: &Entry, kind, options: &InstallerOptions, program: Option<&str>| {
            is_installed_release(entry, kind, options, release, program)
        };

        assert!(check(&installed, Kind::Nsis, &plain, Some("5.3.1.0")));
        assert!(check(&installed, Kind::Nsis, &plain, None));

        // The half-finished install: the entry says 5.3.1, the program is still 5.3.0.
        assert!(!check(&installed, Kind::Nsis, &plain, Some("5.3.0.0")));
        assert!(!check(
            &entry(r"HKLM\x\qBittorrent", "5.3.0"),
            Kind::Nsis,
            &plain,
            None
        ));

        // An entry another installer type wrote gets the installer run, so its uninstaller fits.
        assert!(!check(&installed, Kind::Inno, &plain, None));
        assert!(check(
            &entry(r"HKCU\x\App_is1", "5.3.1"),
            Kind::Inno,
            &plain,
            None
        ));
        assert!(!check(
            &entry(r"HKLM\x\{GUID}", "5.3.1"),
            Kind::Nsis,
            &plain,
            None
        ));

        let user = InstallerOptions {
            scope: Some(Scope::User),
            ..InstallerOptions::default()
        };
        assert!(!check(&installed, Kind::Nsis, &user, None));
        let args = InstallerOptions {
            args: vec!["/D=C:\\Apps".to_owned()],
            ..InstallerOptions::default()
        };
        assert!(!check(&installed, Kind::Nsis, &args, None));
    }

    #[test]
    fn an_install_that_left_the_old_program_is_caught() {
        use super::left_behind;
        let some = |text: &str| Some(text.to_owned());
        let release = "release-5.3.1";

        // qBittorrent was open, so its files stayed at 5.3.0 while its entry said 5.3.1.
        let listed = Some("5.3.0");
        assert_eq!(
            left_behind(some("5.3.0.0"), some("5.3.0.0"), listed, None, release),
            some("5.3.0.0")
        );
        assert_eq!(
            left_behind(some("5.3.0.0"), some("5.3.1.0"), listed, None, release),
            None
        );
        // A reinstall of the same version changes nothing and is fine.
        assert_eq!(
            left_behind(
                some("5.3.1.0"),
                some("5.3.1.0"),
                Some("5.3.1"),
                None,
                release
            ),
            None
        );
        // A file version that never tracked the program's version proves nothing.
        assert_eq!(
            left_behind(some("1.0.0.0"), some("1.0.0.0"), listed, None, release),
            None
        );
        assert_eq!(
            left_behind(None, None, None, Some("5.3.0"), release),
            some("5.3.0")
        );
        assert_eq!(left_behind(None, None, None, Some("5.3.1"), release), None);
        assert_eq!(left_behind(None, None, None, None, release), None);
    }

    #[test]
    fn restarts_succeed_and_known_failures_are_explained() {
        assert!(!check_exit("a", InstallerKind::Msi, 0).unwrap());
        assert!(check_exit("a", InstallerKind::Msi, 3010).unwrap());
        let failed = check_exit("a", InstallerKind::Msi, 1618).unwrap_err();
        assert!(
            failed.to_string().contains("another installation"),
            "{failed}"
        );
        let failed = check_exit("a", InstallerKind::Inno, 5).unwrap_err();
        assert!(failed.to_string().contains("cancelled"), "{failed}");
    }

    #[test]
    fn a_copy_started_from_the_temporary_folder_is_waited_for() {
        // `cmd` copies `PING.EXE` into a temporary folder, starts the copy and exits at once, the
        // way an Inno or NSIS uninstaller hands over to its second phase.
        let folder = tempfile::tempdir().unwrap();
        let copy = folder.path().join("second-phase.exe");
        let launch = super::Launch {
            program: "cmd.exe".into(),
            params: format!(
                r#"/c copy /y C:\Windows\System32\PING.EXE "{0}" >nul && start "" "{0}" -n 4 127.0.0.1"#,
                copy.display()
            ),
            elevate: false,
        };
        let started = std::time::Instant::now();
        assert_eq!(super::run_through("test", &launch).unwrap(), 0);
        assert!(
            started.elapsed() >= std::time::Duration::from_secs(2),
            "returned after {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn registry_commands_split_with_or_without_quotes() {
        assert_eq!(
            split_command(r#""C:\Program Files\App\unins000.exe" /SILENT"#),
            (
                r"C:\Program Files\App\unins000.exe".into(),
                "/SILENT".to_owned()
            )
        );
        assert_eq!(
            split_command(r"C:\Program Files\qBittorrent\uninst.exe"),
            (
                r"C:\Program Files\qBittorrent\uninst.exe".into(),
                String::new()
            )
        );
        assert_eq!(
            split_command("MsiExec.exe /X{1234}"),
            ("MsiExec.exe".into(), "/X{1234}".to_owned())
        );
    }
}
