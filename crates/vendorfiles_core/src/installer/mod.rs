//! Installers: downloads that are run rather than kept.
//!
//! A dependency with an `installer` key downloads exactly one file into a staging folder, runs it
//! silently, and records what it installed in the lockfile. The installer itself is thrown away,
//! and nothing is written into the dependency folder for it, so a `setup.exe` never lands on
//! `PATH`.
//!
//! Running installers is Windows-only for now. Detection is plain byte sniffing and works
//! anywhere, which is what lets the config be checked on every platform.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VendorError};

mod detect;
#[cfg(windows)]
mod windows;

pub use detect::{detect, msix_identity};

/// The installer technologies vendor knows how to run silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallerKind {
    /// Nullsoft (NSIS): `/S`.
    Nsis,
    /// Inno Setup: `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-`.
    Inno,
    /// Windows Installer, run through `msiexec`.
    Msi,
    /// An MSIX or `AppX` package, or a bundle of them.
    Msix,
    /// Any other executable. It runs with exactly the `args` the entry gives it.
    Exe,
}

impl InstallerKind {
    /// The name the config uses for this kind.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Nsis => "nsis",
            Self::Inno => "inno",
            Self::Msi => "msi",
            Self::Msix => "msix",
            Self::Exe => "exe",
        }
    }
}

impl std::fmt::Display for InstallerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Who an install is for. Left unset, the installer decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The current user only, with no admin rights where the installer allows it.
    User,
    /// Every user of the machine. Needs admin, so Windows asks through UAC.
    Machine,
}

/// The config's `installer` key: `true`, a type name, or the full form.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum InstallerKey {
    /// `installer: true` runs the file as whatever it turns out to be. `false` turns it off.
    Flag(bool),
    /// `installer: inno` names the type outright.
    Kind(InstallerKind),
    /// `installer: { type, args, scope }`.
    Options(InstallerOptions),
}

impl InstallerKey {
    /// What this key asks for, or `None` when it turns installing off.
    #[must_use]
    pub fn options(&self) -> Option<InstallerOptions> {
        match self {
            Self::Flag(false) => None,
            Self::Flag(true) => Some(InstallerOptions::default()),
            Self::Kind(kind) => Some(InstallerOptions {
                kind: Some(*kind),
                ..InstallerOptions::default()
            }),
            Self::Options(options) => Some(options.clone()),
        }
    }
}

/// How to run one installer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallerOptions {
    /// The installer's technology. Unset means detect it from the file.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<InstallerKind>,
    /// Extra arguments, passed after the silent switches exactly as written.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Who the install is for. Unset leaves it to the installer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
}

impl InstallerOptions {
    /// The shortest `installer` value that asks for these options, for writing back to a config.
    #[must_use]
    pub fn as_key(&self) -> serde_json::Value {
        match self {
            Self {
                kind: None,
                args,
                scope: None,
            } if args.is_empty() => serde_json::Value::Bool(true),
            Self {
                kind: Some(kind),
                args,
                scope: None,
            } if args.is_empty() => serde_json::Value::String(kind.name().to_owned()),
            full => serde_json::to_value(full).unwrap_or(serde_json::Value::Bool(true)),
        }
    }
}

/// What an installer left behind, as the lockfile records it.
///
/// The uninstall command is not stored. It is read from the registry when it is needed, since
/// every upgrade may rewrite it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Installed {
    #[serde(rename = "type")]
    pub kind: InstallerKind,
    /// The Apps & Features entry the install created, like `HKCU\...\Uninstall\{id}`.
    #[serde(
        rename = "uninstallKey",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub uninstall_key: Option<String>,
    /// The name Apps & Features shows.
    #[serde(
        rename = "displayName",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<String>,
    /// An MSIX package's identity name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

/// What running an installer produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub installed: Installed,
    /// The installer reported that a restart is needed to finish.
    pub reboot: bool,
}

/// Runs the installer at `path` silently and works out what it installed. Blocking.
///
/// `name` is the dependency's name, used to recognise the program among the Apps & Features
/// entries. `version` is the release being installed. `previous` is what the last install
/// recorded, which an upgrade usually reuses.
///
/// # Errors
///
/// Returns [`VendorError::NotAnInstaller`] when the type cannot be detected,
/// [`VendorError::InstallerBlocked`] when the program is running,
/// [`VendorError::InstallerFailed`] for a failing exit code,
/// [`VendorError::InstallerError`] when the program is left at its old version,
/// [`VendorError::InstallerDeclined`] when the UAC prompt was refused, or
/// [`VendorError::InstallersUnsupported`] off Windows.
pub fn install(
    name: &str,
    path: &Path,
    version: &str,
    options: &InstallerOptions,
    previous: Option<&Installed>,
) -> Result<Outcome> {
    let kind = match options.kind {
        Some(kind) => kind,
        None => detect(path)?.ok_or_else(|| VendorError::NotAnInstaller {
            file: file_name(path),
        })?,
    };
    #[cfg(windows)]
    {
        windows::install(name, path, version, kind, options, previous)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, version, kind, previous);
        Err(VendorError::InstallersUnsupported)
    }
}

/// Whether a version a program reports is the release's version.
///
/// Compares the numbers only, so `v5.3.1` and `5.3.1.0` both match the release `release-5.3.1`.
/// `None` when the release has no numbers to compare.
#[must_use]
pub fn is_release_version(found: &str, release: &str) -> Option<bool> {
    let release = numbers(crate::template::version_token(release));
    if release.is_empty() {
        return None;
    }
    let found = numbers(found);
    let width = release.len().max(found.len());
    let part = |list: &[u64], index: usize| list.get(index).copied().unwrap_or(0);
    Some((0..width).all(|index| part(&release, index) == part(&found, index)))
}

/// The first dotted run of numbers in `text`, like `[5, 3, 1]` in `v5.3.1-beta`.
fn numbers(text: &str) -> Vec<u64> {
    let start = text
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(text.len());
    let run = &text[start..];
    let end = run
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(run.len());
    run[..end]
        .split('.')
        .map_while(|part| part.parse().ok())
        .collect()
}

/// Whether what an install recorded is still on the machine.
///
/// An entry with nothing to look up counts as present, since there is no evidence otherwise.
#[cfg(windows)]
#[must_use]
pub fn is_present(installed: &Installed) -> bool {
    windows::is_present(installed)
}

/// Off Windows there is nothing to look an install up in, so it counts as present.
#[cfg(not(windows))]
#[must_use]
pub const fn is_present(_installed: &Installed) -> bool {
    true
}

/// Runs the uninstaller for what an install recorded, and waits until it is gone. Blocking.
///
/// Something already removed by hand counts as uninstalled.
///
/// # Errors
///
/// Returns [`VendorError::UninstallFailed`] when the uninstaller fails or the program is still
/// there afterwards, [`VendorError::InstallerDeclined`] when the UAC prompt was refused, or
/// [`VendorError::InstallersUnsupported`] off Windows.
#[cfg(windows)]
pub fn uninstall(name: &str, installed: &Installed) -> Result<()> {
    windows::uninstall(name, installed)
}

/// Off Windows no installer ran, so there is no uninstaller to run either.
///
/// # Errors
///
/// Always returns [`VendorError::InstallersUnsupported`].
#[cfg(not(windows))]
pub const fn uninstall(_name: &str, _installed: &Installed) -> Result<()> {
    Err(VendorError::InstallersUnsupported)
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{InstallerKey, InstallerKind, InstallerOptions, Scope};

    fn key(json: &str) -> InstallerKey {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn every_form_of_the_key_parses() {
        assert_eq!(key("true").options(), Some(InstallerOptions::default()));
        assert_eq!(key("false").options(), None);
        assert_eq!(
            key(r#""inno""#).options().unwrap().kind,
            Some(InstallerKind::Inno)
        );
        let full = key(r#"{"type":"msi","args":["A=1"],"scope":"user"}"#)
            .options()
            .unwrap();
        assert_eq!(full.kind, Some(InstallerKind::Msi));
        assert_eq!(full.args, ["A=1"]);
        assert_eq!(full.scope, Some(Scope::User));
    }

    #[test]
    fn a_reported_version_matches_the_release_on_its_numbers() {
        use super::is_release_version;
        assert_eq!(is_release_version("5.3.1.0", "release-5.3.1"), Some(true));
        assert_eq!(is_release_version("v5.3.1", "release-5.3.1"), Some(true));
        assert_eq!(is_release_version("5.3.0.0", "release-5.3.1"), Some(false));
        assert_eq!(is_release_version("5.3.1.2", "v5.3.1"), Some(false));
        assert_eq!(is_release_version("1.0", "nightly"), None);
    }

    #[test]
    fn an_unknown_type_or_field_is_refused() {
        assert!(serde_json::from_str::<InstallerKey>(r#""wix""#).is_err());
        assert!(serde_json::from_str::<InstallerKey>(r#"{"silent":true}"#).is_err());
    }
}
