//! Turning a registry entry into config for *this* host.
//!
//! `{release}` and `{version}` belong to the config layer and are left alone, so the entry that
//! gets written keeps resolving new versions afterwards. Only the host-dependent placeholders are
//! expanded here.

use indexmap::IndexMap;

use super::schema::{Program, Target};
use crate::error::{Result, VendorError};
use crate::model::{FileEntry, FileTarget};

/// The host key a registry entry is looked up under: `windows-x86_64`, `macos-aarch64`, …
///
/// Taken straight from what the compiler reports, so there is no mapping table to drift.
#[must_use]
pub fn host() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether a host key names Windows.
///
/// Taken from the key rather than from `cfg!`, so resolving an entry for another platform gives
/// that platform's answer. That is what lets one machine check every host in the registry.
fn is_windows(host: &str) -> bool {
    host.starts_with("windows")
}

/// The archive extension that platform's releases conventionally use.
fn archive_extension(host: &str) -> &'static str {
    if is_windows(host) { ".zip" } else { ".tar.gz" }
}

/// The executable suffix on that platform.
fn executable_suffix(host: &str) -> &'static str {
    if is_windows(host) { ".exe" } else { "" }
}

/// A registry entry resolved for this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The canonical name, which becomes the config key.
    pub name: String,
    /// The repository to install from.
    pub repository: String,
    /// The `files` array to write.
    pub files: Vec<FileEntry>,
}

/// Resolves `program` for the given host key.
///
/// # Errors
///
/// Returns [`VendorError::RegistryUnsupportedHost`] when the entry lists no asset for this host,
/// and [`VendorError::RegistryInvalidEntry`] when it is malformed or names an output that would
/// escape the vendor folder.
pub fn for_host(name: &str, program: &Program, host: &str) -> Result<Entry> {
    let Some(target) = program.targets.get(host) else {
        return Err(VendorError::RegistryUnsupportedHost {
            name: name.to_owned(),
            host: host.to_owned(),
        });
    };

    let (asset, member, named) = match target {
        Target::Explicit(explicit) => (
            explicit.asset.clone(),
            explicit.member.clone(),
            explicit.output.clone().or_else(|| program.output.clone()),
        ),
        Target::Triple(triple) => {
            let Some(asset) = program.asset.as_ref() else {
                return Err(VendorError::RegistryInvalidEntry {
                    name: name.to_owned(),
                    reason: format!(
                        "target '{host}' names the triple '{triple}', so the entry needs a \
                         top-level 'asset' pattern"
                    ),
                });
            };
            let expanded = |pattern: &String| expand(pattern, triple, host);
            (
                expand(asset, triple, host),
                program.member.as_ref().map(expanded),
                program.output.as_ref().map(expanded),
            )
        }
    };

    // Whichever names the file: the member inside an archive, or the asset itself when the asset
    // *is* the executable.
    let output = output_name(name, named.as_ref().or(member.as_ref()).unwrap_or(&asset))?;

    let target = match member {
        // An extraction map, not a list: a list maps each member to itself, which for a member
        // nested in the archive's own directory would recreate that directory inside the folder.
        Some(member) => {
            let mut members = IndexMap::new();
            members.insert(member, output);
            FileTarget::ExtractMap(members)
        }
        // No member: the asset is the binary, saved under `output` as it stands.
        None => FileTarget::Rename(output),
    };
    let mut files = IndexMap::new();
    files.insert(asset, target);

    Ok(Entry {
        name: name.to_owned(),
        repository: program.repository.clone(),
        files: vec![FileEntry::Mapped(files)],
    })
}

/// Substitutes the host-dependent placeholders, leaving the config's own alone.
fn expand(pattern: &str, triple: &str, host: &str) -> String {
    pattern
        .replace("{target}", triple)
        .replace("{ext}", archive_extension(host))
        .replace("{exe}", executable_suffix(host))
}

/// The name the extracted member is written under: its basename.
///
/// Also the trust check. A registry says what to fetch, never where it goes, so an output that is
/// absolute, empty, or climbing out of the folder is refused rather than normalised.
fn output_name(name: &str, member: &str) -> Result<String> {
    let refuse = |reason: &str| VendorError::RegistryInvalidEntry {
        name: name.to_owned(),
        reason: reason.to_owned(),
    };

    let basename = member
        .rsplit(['/', '\\'])
        .next()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| refuse("the member names no file"))?;
    if basename == ".." || basename == "." {
        return Err(refuse("the member names no file"));
    }
    // Belt and braces: `join_normalized` already keeps writes inside the folder, but a registry
    // entry should never even look like it is trying to leave.
    if basename.contains(':') || basename.starts_with('/') || basename.starts_with('\\') {
        return Err(refuse(&format!("'{basename}' is not a plain file name")));
    }
    Ok(basename.to_owned())
}

#[cfg(test)]
mod tests {
    // `{target}`, `{ext}` and `{release}` are registry placeholders, read by our own
    // expander rather than by `format!`.
    #![expect(clippy::literal_string_with_formatting_args, reason = "placeholders")]

    use super::{Entry, for_host, host, output_name};
    use crate::model::{FileEntry, FileTarget};
    use crate::registry::schema::Document;

    fn program(yaml: &str) -> Document {
        serde_yaml_ng::from_str(yaml).expect("valid registry")
    }

    const FD: &str = r#"
version: 1
programs:
  fd:
    repository: https://github.com/sharkdp/fd
    asset: "{release}/fd-v{version}-{target}{ext}"
    member: "fd-v{version}-{target}/fd{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
"#;

    /// The single asset and its one member, for readable assertions.
    fn only(entry: &Entry) -> (String, String, String) {
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().expect("one asset");
        let FileTarget::ExtractMap(members) = target else {
            panic!("expected an extraction map, found {target:?}");
        };
        let (member, output) = members.iter().next().expect("one member");
        (asset.clone(), member.clone(), output.clone())
    }

    #[test]
    fn the_host_key_is_what_the_compiler_reports() {
        assert_eq!(
            host(),
            format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    #[test]
    fn every_target_expands_to_its_own_asset() {
        let document = program(FD);
        let fd = &document.programs["fd"];

        // Windows: a zip, an `.exe`, and the archive's own directory in the member path.
        let entry = for_host("fd", fd, "windows-x86_64").unwrap();
        let (asset, member, output) = only(&entry);
        assert_eq!(asset, "{release}/fd-v{version}-x86_64-pc-windows-msvc.zip");
        assert_eq!(member, "fd-v{version}-x86_64-pc-windows-msvc/fd.exe");
        assert_eq!(output, "fd.exe");

        // macOS on Apple Silicon: a tarball and no suffix.
        let entry = for_host("fd", fd, "macos-aarch64").unwrap();
        let (asset, member, output) = only(&entry);
        assert_eq!(asset, "{release}/fd-v{version}-aarch64-apple-darwin.tar.gz");
        assert_eq!(member, "fd-v{version}-aarch64-apple-darwin/fd");
        assert_eq!(output, "fd");

        let entry = for_host("fd", fd, "linux-x86_64").unwrap();
        let (asset, _, output) = only(&entry);
        assert_eq!(
            asset,
            "{release}/fd-v{version}-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(output, "fd");
    }

    #[test]
    fn the_config_placeholders_survive_untouched() {
        // `{version}` has to stay symbolic or the entry would only ever install one release.
        let document = program(FD);
        let entry = for_host("fd", &document.programs["fd"], "linux-x86_64").unwrap();
        let (asset, member, _) = only(&entry);
        assert!(asset.starts_with("{release}/"), "{asset}");
        assert!(asset.contains("{version}"), "{asset}");
        assert!(member.contains("{version}"), "{member}");
    }

    #[test]
    fn the_explicit_form_needs_no_shared_patterns() {
        let document = program(
            r#"
version: 1
programs:
  fzf:
    repository: https://github.com/junegunn/fzf
    targets:
      windows-x86_64:
        asset: "{release}/fzf-{version}-windows_amd64.zip"
        member: "fzf.exe"
"#,
        );
        let entry = for_host("fzf", &document.programs["fzf"], "windows-x86_64").unwrap();
        let (asset, member, output) = only(&entry);
        assert_eq!(asset, "{release}/fzf-{version}-windows_amd64.zip");
        assert_eq!(member, "fzf.exe");
        assert_eq!(output, "fzf.exe");
    }

    #[test]
    fn an_asset_with_no_member_is_downloaded_as_it_stands() {
        // Plenty of projects publish a bare binary rather than an archive.
        let document = program(
            r"
version: 1
programs:
  ox:
    repository: https://github.com/curlpipe/ox
    targets:
      windows-x86_64:
        asset: '{release}/ox.exe'
      macos-x86_64:
        asset: '{release}/ox-macos'
        as: ox
",
        );
        let ox = &document.programs["ox"];

        let entry = for_host("ox", ox, "windows-x86_64").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().unwrap();
        assert_eq!(asset, "{release}/ox.exe");
        // A rename, not an extraction: there is no archive to look inside.
        assert_eq!(target, &FileTarget::Rename("ox.exe".to_owned()));

        // `as` renames an asset whose own name is not the command's.
        let entry = for_host("ox", ox, "macos-x86_64").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().unwrap();
        assert_eq!(asset, "{release}/ox-macos");
        assert_eq!(target, &FileTarget::Rename("ox".to_owned()));
    }

    #[test]
    fn the_output_name_is_expanded_for_the_host_too() {
        // `shfmt` publishes bare binaries whose suffix differs by platform.
        let document = program(
            r"
version: 1
programs:
  shfmt:
    repository: https://github.com/mvdan/sh
    asset: '{release}/shfmt_v{version}_{target}{exe}'
    as: 'shfmt{exe}'
    targets:
      windows-x86_64: windows_amd64
      linux-x86_64: linux_amd64
",
        );
        let shfmt = &document.programs["shfmt"];

        let entry = for_host("shfmt", shfmt, "windows-x86_64").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().unwrap();
        assert_eq!(asset, "{release}/shfmt_v{version}_windows_amd64.exe");
        assert_eq!(target, &FileTarget::Rename("shfmt.exe".to_owned()));

        let entry = for_host("shfmt", shfmt, "linux-x86_64").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().unwrap();
        assert_eq!(asset, "{release}/shfmt_v{version}_linux_amd64");
        assert_eq!(target, &FileTarget::Rename("shfmt".to_owned()));
    }

    #[test]
    fn a_host_the_entry_does_not_cover_is_reported_as_such() {
        let document = program(FD);
        let error = for_host("fd", &document.programs["fd"], "freebsd-x86_64").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("fd"), "{message}");
        assert!(message.contains("freebsd-x86_64"), "{message}");
    }

    #[test]
    fn a_triple_target_without_shared_patterns_is_an_error() {
        let document = program(
            r"
version: 1
programs:
  broken:
    repository: https://github.com/example/broken
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
",
        );
        let error = for_host("broken", &document.programs["broken"], "windows-x86_64").unwrap_err();
        assert!(error.to_string().contains("asset"), "{error}");
    }

    #[test]
    fn an_output_that_would_leave_the_vendor_folder_is_refused() {
        // A registry may say what to fetch, never where it lands.
        assert!(output_name("x", "some/dir/../..").is_err());
        assert!(output_name("x", "").is_err());
        assert!(output_name("x", "dir/").is_err());
        assert!(output_name("x", "C:\\Windows\\System32\\evil.dll:").is_err());
        // A nested member is fine; only its basename is written.
        assert_eq!(output_name("x", "a/b/c/tool").unwrap(), "tool");
    }
}
