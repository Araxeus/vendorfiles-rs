//! Dependencies `vendor` can install from a bare name.
//!
//! `vendor add fzf` has to search GitHub and then be told which files to take. A handful of
//! dependencies are worth knowing outright, starting with this tool: `vendor add vendorfiles`
//! writes an entry that points at the release asset for the running platform and lands it on top
//! of the running binary, which turns `vendor sync` into a self-update.

use indexmap::IndexMap;
use vendorfiles_core::{FileEntry, FileTarget};

/// Where `vendor` releases live.
const REPOSITORY: &str = "https://github.com/Araxeus/vendorfiles-rs";

/// A dependency that needs no `--files` to describe it.
#[derive(Debug, Clone)]
pub struct Known {
    /// The repository to install from.
    pub repository: &'static str,
    /// The release asset for this platform, and what to take out of it.
    pub files: Vec<FileEntry>,
    /// Where the files belong, when the usual `vendor/<name>` is wrong.
    pub folder: Option<String>,
}

/// The entry `name` describes, if there is one.
#[must_use]
pub fn find(name: &str) -> Option<Known> {
    match name {
        "vendorfiles" | "vendorfiles-rs" | "vendor" => Some(itself()),
        _ => None,
    }
}

/// An entry that keeps this binary up to date.
///
/// The asset is the one the release workflow publishes for this platform, and the folder is
/// wherever the running binary sits - so the extracted member's destination *is* the running
/// executable, and `ops::install` swaps it in place rather than writing over a locked image.
fn itself() -> Known {
    let (asset, member) = release_asset();
    let mut files = IndexMap::new();
    files.insert(
        format!("{{release}}/{asset}"),
        // A list keeps the member's own name, which is what makes the destination the running
        // binary rather than something beside it.
        FileTarget::ExtractList(vec![member.to_owned()]),
    );
    Known {
        repository: REPOSITORY,
        files: vec![FileEntry::Mapped(files)],
        folder: installed_beside_the_binary(),
    }
}

/// The release asset for this platform, and the executable inside it.
///
/// Mirrors `.github/workflows/release.yml`, which names assets
/// `vendor_<tag>_<platform>.<ext>`. `{version}` expands to the semver core of the tag, so the
/// `v` is spelled out here.
const fn release_asset() -> (&'static str, &'static str) {
    #[cfg(target_os = "windows")]
    {
        ("vendor_v{version}_windows.zip", "vendor.exe")
    }
    #[cfg(target_os = "macos")]
    {
        ("vendor_v{version}_macos.tar.gz", "vendor")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        ("vendor_v{version}_linux.tar.gz", "vendor")
    }
}

/// The directory holding the running binary, which is where its replacement goes.
fn installed_beside_the_binary() -> Option<String> {
    let executable = std::env::current_exe().ok()?;
    let directory = executable.parent()?;
    Some(directory.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::{find, itself, release_asset};
    use vendorfiles_core::{FileEntry, FileTarget};

    #[test]
    fn only_the_names_this_tool_answers_to_are_known() {
        assert!(find("vendorfiles").is_some());
        assert!(find("vendorfiles-rs").is_some());
        assert!(find("vendor").is_some());
        assert!(find("fzf").is_none());
        assert!(find("").is_none());
    }

    #[test]
    fn the_entry_extracts_this_platform_s_executable_from_a_release_asset() {
        let (asset, member) = release_asset();
        let known = itself();
        assert_eq!(known.repository, super::REPOSITORY);

        let [FileEntry::Mapped(files)] = known.files.as_slice() else {
            panic!("expected one mapped entry, found {:?}", known.files);
        };
        let (input, target) = files.iter().next().expect("one asset");
        assert_eq!(input, &format!("{{release}}/{asset}"));

        // Extracted under its own name, so the destination is the running binary itself.
        let FileTarget::ExtractList(members) = target else {
            panic!("expected an extraction list, found {target:?}");
        };
        assert_eq!(members, &vec![member.to_owned()]);
    }

    #[test]
    fn the_asset_name_carries_the_tag_s_leading_v() {
        // `{version}` is the semver core, so `v` has to be spelled out to match a `v2.0.4` tag.
        let (asset, _) = release_asset();
        assert!(asset.contains("_v{version}_"), "{asset}");
        assert!(asset.starts_with("vendor_"), "{asset}");
    }

    #[test]
    fn the_folder_is_the_one_holding_the_running_binary() {
        let folder = super::installed_beside_the_binary().expect("a running binary has a folder");
        let executable = std::env::current_exe().unwrap();
        assert_eq!(
            std::path::Path::new(&folder),
            executable.parent().unwrap(),
            "the replacement has to land on top of the running binary"
        );
    }
}
