//! The Apps & Features entries installers write, under the registry's `Uninstall` keys.
//!
//! An install is tracked by the entry it created. vendor reads every entry before and after the
//! installer runs, and the difference is what it installed. That holds whatever the installer
//! named its key, and it holds for upgrades too, which rewrite the entry's version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL_32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

/// Every place Apps & Features reads entries from: machine-wide in both registry views, then the
/// current user's.
const ROOTS: [(&str, &str); 3] = [
    ("HKLM", UNINSTALL),
    ("HKLM", UNINSTALL_32),
    ("HKCU", UNINSTALL),
];

/// One Apps & Features entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The full key, like `HKCU\Software\...\Uninstall\{id}`.
    pub key: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub uninstall: Option<String>,
    pub quiet_uninstall: Option<String>,
    pub install_location: Option<String>,
    pub display_icon: Option<String>,
    /// Hidden from Apps & Features: a component of something else.
    pub system: bool,
}

impl Entry {
    /// The last segment of the key, which is the product code for an MSI.
    #[must_use]
    pub fn id(&self) -> &str {
        self.key.rsplit('\\').next().unwrap_or_default()
    }

    /// Whether the entry lives in the machine-wide hive, whose uninstall needs admin.
    #[must_use]
    pub fn is_machine_wide(&self) -> bool {
        self.key.starts_with("HKLM\\")
    }

    /// The program's own executable, when the entry's icon points at one.
    #[must_use]
    pub fn program(&self) -> Option<PathBuf> {
        let icon = self.display_icon.as_deref()?;
        let icon = icon.split_once(',').map_or(icon, |(path, _)| path);
        let path = PathBuf::from(icon.trim().trim_matches('"'));
        let is_exe = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
        (is_exe && path.is_absolute()).then_some(path)
    }

    /// The folder the program is installed in.
    ///
    /// Read from `InstallLocation`, or else from where its icon or uninstaller lives. A folder
    /// that holds other programs too, like `Program Files` or `System32`, never counts.
    #[must_use]
    pub fn folder(&self) -> Option<PathBuf> {
        let uninstaller = self
            .uninstall
            .as_deref()
            .map(|command| PathBuf::from(super::split_command(command).0));
        let folder = self
            .install_location
            .as_deref()
            .map(|location| PathBuf::from(location.trim_matches('"')))
            .or_else(|| self.program()?.parent().map(Path::to_path_buf))
            .or_else(|| uninstaller?.parent().map(Path::to_path_buf))?;
        own_folder(&folder).then_some(folder)
    }

    const fn visible(&self) -> bool {
        !self.system && self.name.is_some()
    }
}

/// Whether a folder plausibly belongs to one program: two levels deep at least, and outside the
/// Windows folder.
fn own_folder(folder: &Path) -> bool {
    use std::path::Component;
    let depth = folder
        .components()
        .filter(|part| matches!(part, Component::Normal(_)))
        .count();
    let lower = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
    let windows = std::env::var_os("SystemRoot").map(|root| lower(Path::new(&root)));
    folder.is_absolute()
        && depth >= 2
        && !windows.is_some_and(|root| lower(folder).starts_with(root))
}

fn hive(name: &str) -> Option<&'static Key> {
    match name {
        "HKLM" => Some(LOCAL_MACHINE),
        "HKCU" => Some(CURRENT_USER),
        _ => None,
    }
}

fn read_entry(key: &Key, full: String) -> Entry {
    let text = |value: &str| {
        key.get_string(value)
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
    };
    Entry {
        name: text("DisplayName"),
        version: text("DisplayVersion"),
        uninstall: text("UninstallString"),
        quiet_uninstall: text("QuietUninstallString"),
        install_location: text("InstallLocation"),
        display_icon: text("DisplayIcon"),
        system: key.get_u32("SystemComponent").is_ok_and(|flag| flag == 1),
        key: full,
    }
}

/// Every entry, keyed by its full key.
#[must_use]
pub fn snapshot() -> HashMap<String, Entry> {
    let mut entries = HashMap::new();
    for (hive_name, path) in ROOTS {
        let Some(root) = hive(hive_name).and_then(|hive| hive.open(path).ok()) else {
            continue;
        };
        let Ok(names) = root.keys() else {
            continue;
        };
        for name in names {
            let Ok(key) = root.open(&name) else {
                continue;
            };
            let full = format!(r"{hive_name}\{path}\{name}");
            entries.insert(full.clone(), read_entry(&key, full));
        }
    }
    entries
}

/// The entry at a full key, if it is still there.
#[must_use]
pub fn read(full: &str) -> Option<Entry> {
    let (hive_name, path) = full.split_once('\\')?;
    let key = hive(hive_name)?.open(path).ok()?;
    Some(read_entry(&key, full.to_owned()))
}

/// The entry a program sits under before an install runs: the one recorded last time, or else
/// the one whose name matches the dependency.
#[must_use]
pub fn existing(
    name: &str,
    entries: &HashMap<String, Entry>,
    previous: Option<&str>,
) -> Option<Entry> {
    if let Some(entry) = previous.and_then(|key| entries.get(key)) {
        return Some(entry.clone());
    }
    let mut every: Vec<&Entry> = entries.values().filter(|entry| entry.visible()).collect();
    every.sort_by(|a, b| a.key.cmp(&b.key));
    best_match(name, every.into_iter()).cloned()
}

/// The entry an install created or rewrote.
///
/// The entries that changed come first: the one recorded last time if it is among them, else the
/// only one, else the one whose name matches the dependency. With no change at all - a reinstall
/// of the same version - the recorded entry stands, and failing that a name match among every
/// entry.
#[must_use]
pub fn pick(
    name: &str,
    before: &HashMap<String, Entry>,
    after: &HashMap<String, Entry>,
    previous: Option<&str>,
) -> Option<Entry> {
    let mut changed: Vec<&Entry> = after
        .values()
        .filter(|entry| entry.visible() && before.get(&entry.key) != Some(*entry))
        .collect();
    changed.sort_by(|a, b| a.key.cmp(&b.key));

    if let Some(previous) = previous
        && let Some(entry) = changed.iter().find(|entry| entry.key == previous)
    {
        return Some((*entry).clone());
    }
    if let [only] = changed.as_slice() {
        return Some((*only).clone());
    }
    if let Some(entry) = best_match(name, changed.iter().copied()) {
        return Some(entry.clone());
    }
    if let Some(first) = changed.first() {
        return Some((*first).clone());
    }
    if let Some(entry) = previous.and_then(|key| after.get(key)) {
        return Some(entry.clone());
    }
    let mut every: Vec<&Entry> = after.values().filter(|entry| entry.visible()).collect();
    every.sort_by(|a, b| a.key.cmp(&b.key));
    best_match(name, every.into_iter()).cloned()
}

/// The entry whose display name names the dependency, compared on letters and digits only.
fn best_match<'a>(name: &str, entries: impl Iterator<Item = &'a Entry>) -> Option<&'a Entry> {
    let wanted = squash(name);
    if wanted.len() < 3 {
        return None;
    }
    entries
        .filter(|entry| {
            entry.name.as_deref().is_some_and(|display| {
                let display = squash(display);
                display.contains(&wanted) || (display.len() >= 3 && wanted.contains(&display))
            })
        })
        .min_by_key(|entry| entry.name.as_ref().map_or(usize::MAX, String::len))
}

fn squash(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{Entry, pick};

    fn entry(key: &str, name: &str, version: &str) -> Entry {
        Entry {
            key: key.to_owned(),
            name: Some(name.to_owned()),
            version: Some(version.to_owned()),
            uninstall: Some("u.exe".to_owned()),
            quiet_uninstall: None,
            install_location: None,
            display_icon: None,
            system: false,
        }
    }

    fn map(entries: &[Entry]) -> HashMap<String, Entry> {
        entries.iter().map(|e| (e.key.clone(), e.clone())).collect()
    }

    #[test]
    fn the_folder_comes_from_the_location_or_else_the_icon() {
        let mut qbittorrent = entry(r"HKLM\x\qBittorrent", "qBittorrent", "5.3.0");
        qbittorrent.display_icon =
            Some(r#""C:\Program Files\qBittorrent\qbittorrent.exe",0"#.to_owned());
        assert_eq!(
            qbittorrent.program(),
            Some(r"C:\Program Files\qBittorrent\qbittorrent.exe".into())
        );
        assert_eq!(
            qbittorrent.folder(),
            Some(r"C:\Program Files\qBittorrent".into())
        );
        qbittorrent.install_location = Some(r"D:\Apps\qBittorrent".to_owned());
        assert_eq!(qbittorrent.folder(), Some(r"D:\Apps\qBittorrent".into()));
    }

    #[test]
    fn a_shared_folder_is_never_the_install_folder() {
        let mut shared = entry(r"HKLM\x\{1}", "Tool", "1");
        shared.display_icon = Some(r"C:\Windows\System32\msiexec.exe,0".to_owned());
        assert_eq!(shared.folder(), None);
        shared.install_location = Some(r"C:\Program Files\".to_owned());
        assert_eq!(shared.folder(), None);
    }

    #[test]
    fn a_new_entry_is_what_was_installed() {
        let other = entry(r"HKLM\x\Git", "Git", "2.0");
        let before = map(std::slice::from_ref(&other));
        let after = map(&[other, entry(r"HKCU\x\WizTree_is1", "WizTree 4.34", "4.34")]);
        assert_eq!(
            pick("wiztree", &before, &after, None).unwrap().key,
            r"HKCU\x\WizTree_is1"
        );
    }

    #[test]
    fn an_upgrade_is_found_by_its_changed_version() {
        let before = map(&[entry(r"HKLM\x\qBittorrent", "qBittorrent", "5.3.0")]);
        let after = map(&[entry(r"HKLM\x\qBittorrent", "qBittorrent", "5.3.1")]);
        assert_eq!(
            pick("anything", &before, &after, None)
                .unwrap()
                .version
                .as_deref(),
            Some("5.3.1")
        );
    }

    #[test]
    fn among_several_changes_the_name_decides() {
        let after = map(&[
            entry(r"HKLM\x\{VC}", "Microsoft Visual C++ Redistributable", "14"),
            entry(r"HKLM\x\{AB}", "Audio Bartender", "1.2.0"),
        ]);
        assert_eq!(
            pick("Audio-Bartender", &HashMap::new(), &after, None)
                .unwrap()
                .key,
            r"HKLM\x\{AB}"
        );
    }

    #[test]
    fn a_reinstall_keeps_the_recorded_entry_and_hidden_entries_never_count() {
        let mut hidden = entry(r"HKLM\x\part", "Part", "1");
        hidden.system = true;
        let same = map(&[entry(r"HKLM\x\app", "App", "1"), hidden.clone()]);
        assert_eq!(
            pick("zzz", &same, &same, Some(r"HKLM\x\app")).unwrap().key,
            r"HKLM\x\app"
        );
        let after = map(&[hidden]);
        assert_eq!(pick("part", &HashMap::new(), &after, None), None);
    }
}
