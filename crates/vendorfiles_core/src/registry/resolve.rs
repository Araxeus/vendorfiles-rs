//! Turning a registry entry into config for *this* host.
//!
//! `{release}` and `{version}` belong to the config layer and are left alone, so the entry that
//! gets written keeps resolving new versions afterwards. Only the host-dependent placeholders are
//! expanded here.

use indexmap::IndexMap;

use super::schema::{Member, Program, Target};
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
    /// Which tags count as releases, when the repository publishes several trains.
    pub release_regex: Option<String>,
    /// Whether to track a repository file by commit.
    pub hash_version_file: Option<bool>,
}

/// Resolves `program` for the given host key.
///
/// # Errors
///
/// Returns [`VendorError::RegistryUnsupportedHost`] when the entry lists no asset for this host,
/// and [`VendorError::RegistryInvalidEntry`] when it is malformed or names an output that would
/// escape the vendor folder.
pub fn for_host(name: &str, program: &Program, host: &str) -> Result<Entry> {
    if let Some(path) = program.path.as_ref() {
        if !program.targets.is_empty() || program.asset.is_some() || program.member.is_some() {
            return Err(VendorError::RegistryInvalidEntry {
                name: name.to_owned(),
                reason: "'path' vendors a repository file, so it cannot also define 'asset', \
                         'member' or 'targets'"
                    .to_owned(),
            });
        }
        return repository_file(name, program, path);
    }

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
                program
                    .member
                    .as_ref()
                    .map(|member| expand_member(member, triple, host)),
                program.output.as_ref().map(expanded),
            )
        }
    };

    let target = match member {
        // No member: the asset is the binary itself, saved under its own name or the one `as`
        // gives it.
        None => FileTarget::Rename(output_name(name, named.as_ref().unwrap_or(&asset))?),
        Some(member) => FileTarget::ExtractMap(extraction(name, &member, named.as_ref())?),
    };
    let mut files = IndexMap::new();
    files.insert(asset, target);

    Ok(Entry {
        name: name.to_owned(),
        repository: program.repository.clone(),
        files: vec![FileEntry::Mapped(files)],
        release_regex: program.release_regex.clone(),
        hash_version_file: program.hash_version_file,
    })
}

/// Resolves a program that vendors a file from the repository rather than a release.
///
/// Platform-independent by nature, so there is nothing to expand and no target to choose.
fn repository_file(name: &str, program: &Program, path: &str) -> Result<Entry> {
    let files = if let Some(output) = program.output.as_ref() {
        let output = output_name(name, output)?;
        let mut mapped = IndexMap::new();
        mapped.insert(path.to_owned(), FileTarget::Rename(output));
        FileEntry::Mapped(mapped)
    } else {
        // Saved under its own basename, which is what a bare string means in a config.
        output_name(name, path)?;
        FileEntry::Simple(path.to_owned())
    };
    Ok(Entry {
        name: name.to_owned(),
        repository: program.repository.clone(),
        files: vec![files],
        release_regex: program.release_regex.clone(),
        hash_version_file: program.hash_version_file,
    })
}

/// Substitutes the host-dependent placeholders, leaving the config's own alone.
fn expand(pattern: &str, triple: &str, host: &str) -> String {
    pattern
        .replace("{target}", triple)
        .replace("{ext}", archive_extension(host))
        .replace("{exe}", executable_suffix(host))
}

/// Substitutes the host-dependent placeholders in every path a member names.
fn expand_member(member: &Member, triple: &str, host: &str) -> Member {
    match member {
        Member::One(path) => Member::One(expand(path, triple, host)),
        Member::Many(paths) => Member::Many(
            paths
                .iter()
                .map(|path| expand(path, triple, host))
                .collect(),
        ),
    }
}

/// The members to take out of an asset, as input->output pairs.
///
/// The two forms name their outputs differently, because they mean different things. One member
/// is *the executable*, so it lands under its basename and `as` may rename it - a member nested
/// in the archive's own versioned directory would otherwise recreate that directory inside the
/// vendor folder. A list is *a layout*, so each path is written out as it stands: `herdr.exe`
/// looks for `conpty/x64/OpenConsole.exe` beside it, and flattening would collide that with the
/// `arm64` build of the same name.
///
/// # Errors
///
/// Returns [`VendorError::RegistryInvalidEntry`] for an empty list, for a list paired with `as`,
/// and for any path that would write outside the vendor folder.
fn extraction(
    name: &str,
    member: &Member,
    named: Option<&String>,
) -> Result<IndexMap<String, String>> {
    let refuse = |reason: String| VendorError::RegistryInvalidEntry {
        name: name.to_owned(),
        reason,
    };

    let paths = member.paths();
    if !member.is_list() {
        let path = &paths[0];
        let output = output_name(name, named.unwrap_or(path))?;
        return Ok(std::iter::once((path.clone(), output)).collect());
    }

    if paths.is_empty() {
        return Err(refuse(
            "'member' lists no file to take out of the asset".to_owned(),
        ));
    }
    if let Some(named) = named {
        return Err(refuse(format!(
            "'member' lists {} files, written out as they stand, so there is no single file for \
             'as' to rename to '{named}'",
            paths.len()
        )));
    }
    paths
        .iter()
        .map(|path| member_path(name, path).map(|output| (path.clone(), output)))
        .collect()
}

/// The relative path a listed member is written to, which is the member's own.
///
/// The trust check [`output_name`] makes, without the flattening: every segment has to be a plain
/// name, so nothing absolute, climbing or drive-qualified reaches `join_normalized` - which
/// resolves `..` against the folder it joins onto, and would let one escape.
fn member_path(name: &str, member: &str) -> Result<String> {
    let segments: Vec<&str> = member.split(['/', '\\']).collect();
    let plain = |segment: &&str| {
        !segment.is_empty() && *segment != "." && *segment != ".." && !segment.contains(':')
    };
    if !segments.iter().all(plain) {
        return Err(VendorError::RegistryInvalidEntry {
            name: name.to_owned(),
            reason: format!("'{member}' is not a plain relative path"),
        });
    }
    Ok(segments.join("/"))
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

    use super::{Entry, for_host, host, member_path, output_name};
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

    /// Every input->output pair of a single-asset entry, in order.
    fn extracted(entry: &Entry) -> Vec<(String, String)> {
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (_, target) = files.iter().next().expect("one asset");
        let FileTarget::ExtractMap(members) = target else {
            panic!("expected an extraction map, found {target:?}");
        };
        members
            .iter()
            .map(|(input, output)| (input.clone(), output.clone()))
            .collect()
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
    fn a_repository_file_needs_no_target() {
        // Not a release at all: the same file on every platform, tracked by commit.
        let document = program(
            r"
version: 1
programs:
  omp-theme:
    repository: https://github.com/JanDeDobbeleer/oh-my-posh
    path: themes/powerlevel10k_rainbow.omp.json
    hashVersionFile: true
",
        );
        let entry = for_host("omp-theme", &document.programs["omp-theme"], "any").unwrap();
        assert_eq!(entry.hash_version_file, Some(true));
        assert_eq!(
            entry.files,
            vec![FileEntry::Simple(
                "themes/powerlevel10k_rainbow.omp.json".to_owned()
            )],
            "a bare path is saved under its own basename"
        );
    }

    #[test]
    fn a_repository_file_can_be_renamed() {
        let document = program(
            r"
version: 1
programs:
  omp-theme:
    repository: https://github.com/JanDeDobbeleer/oh-my-posh
    path: themes/powerlevel10k_rainbow.omp.json
    as: my-prompt.json
",
        );
        let entry = for_host("omp-theme", &document.programs["omp-theme"], "any").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected a mapped entry, found {:?}", entry.files);
        };
        assert_eq!(
            files.get("themes/powerlevel10k_rainbow.omp.json"),
            Some(&FileTarget::Rename("my-prompt.json".to_owned()))
        );
    }

    #[test]
    fn a_path_beside_any_release_field_is_refused() {
        // The two describe different things, and silently ignoring one would be a trap. All three
        // release fields count, not just `targets`.
        for extra in [
            "    asset: '{release}/thing.zip'",
            "    member: 'thing.exe'",
            "    targets:\n      windows-x86_64: x86_64-pc-windows-msvc",
        ] {
            let yaml = format!(
                "version: 1\nprograms:\n  confused:\n    repository: https://github.com/e/c\n    path: some/file.txt\n{extra}\n"
            );
            let document = program(&yaml);
            let error = for_host("confused", &document.programs["confused"], "windows-x86_64")
                .expect_err("must be refused");
            let message = error.to_string();
            assert!(
                message.contains("asset") && message.contains("targets"),
                "for {extra:?}: {message}"
            );
        }
    }

    #[test]
    fn a_path_and_targets_together_are_refused() {
        // They describe two different things; silently preferring one would be a trap.
        let document = program(
            r"
version: 1
programs:
  confused:
    repository: https://github.com/example/confused
    path: some/file.txt
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
",
        );
        let error = for_host("confused", &document.programs["confused"], "windows-x86_64")
            .expect_err("must be refused");
        assert!(error.to_string().contains("targets"), "{error}");
    }

    #[test]
    fn a_release_regex_reaches_the_entry() {
        // Without it, `bitwarden/sdk`'s newest release is one with no assets.
        let document = program(
            r"
version: 1
programs:
  bws:
    repository: https://github.com/bitwarden/sdk
    releaseRegex: '^bws-v\d+\.\d+\.\d+$'
    asset: '{release}/bws-{target}-{version}.zip'
    member: 'bws{exe}'
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
",
        );
        let entry = for_host("bws", &document.programs["bws"], "windows-x86_64").unwrap();
        assert_eq!(
            entry.release_regex.as_deref(),
            Some(r"^bws-v\d+\.\d+\.\d+$")
        );
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

    /// An entry whose Windows asset holds a layout rather than a lone executable.
    const HERDR: &str = r#"
version: 1
programs:
  herdr:
    repository: https://github.com/herdrdev/herdr
    targets:
      windows-x86_64:
        asset: "{release}/herdr-windows-x86_64.zip"
        member:
          - herdr.exe
          - conpty/conpty.dll
          - conpty/x64/OpenConsole.exe
          - conpty/arm64/OpenConsole.exe
      linux-x86_64:
        asset: "{release}/herdr-linux-x86_64"
        as: herdr
"#;

    #[test]
    fn a_listed_member_keeps_the_path_it_has_in_the_archive() {
        // `herdr.exe` loads `conpty/` from beside itself, so the directories have to survive -
        // and the two `OpenConsole.exe` builds would collide the moment they did not.
        let document = program(HERDR);
        let entry = for_host("herdr", &document.programs["herdr"], "windows-x86_64").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().expect("one asset");
        assert_eq!(asset, "{release}/herdr-windows-x86_64.zip");
        let FileTarget::ExtractMap(members) = target else {
            panic!("expected an extraction map, found {target:?}");
        };
        assert_eq!(
            members.iter().collect::<Vec<_>>(),
            [
                (&"herdr.exe".to_owned(), &"herdr.exe".to_owned()),
                (
                    &"conpty/conpty.dll".to_owned(),
                    &"conpty/conpty.dll".to_owned()
                ),
                (
                    &"conpty/x64/OpenConsole.exe".to_owned(),
                    &"conpty/x64/OpenConsole.exe".to_owned()
                ),
                (
                    &"conpty/arm64/OpenConsole.exe".to_owned(),
                    &"conpty/arm64/OpenConsole.exe".to_owned()
                ),
            ]
        );

        // The hosts that publish a bare binary are untouched by any of it.
        let entry = for_host("herdr", &document.programs["herdr"], "linux-x86_64").unwrap();
        let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
            panic!("expected one mapped entry");
        };
        let (asset, target) = files.iter().next().unwrap();
        assert_eq!(asset, "{release}/herdr-linux-x86_64");
        assert_eq!(target, &FileTarget::Rename("herdr".to_owned()));
    }

    #[test]
    fn every_listed_member_is_expanded_for_the_host() {
        // The compact form substitutes into each path, not only into the first.
        let document = program(
            r#"
version: 1
programs:
  layout:
    repository: https://github.com/example/layout
    asset: "{release}/layout-{target}{ext}"
    member:
      - "layout{exe}"
      - "support/{target}/data.bin"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      linux-x86_64: x86_64-unknown-linux-gnu
"#,
        );
        let layout = &document.programs["layout"];

        let entry = for_host("layout", layout, "windows-x86_64").unwrap();
        assert_eq!(
            extracted(&entry),
            [
                ("layout.exe".to_owned(), "layout.exe".to_owned()),
                (
                    "support/x86_64-pc-windows-msvc/data.bin".to_owned(),
                    "support/x86_64-pc-windows-msvc/data.bin".to_owned()
                ),
            ]
        );

        let entry = for_host("layout", layout, "linux-x86_64").unwrap();
        assert_eq!(
            extracted(&entry),
            [
                ("layout".to_owned(), "layout".to_owned()),
                (
                    "support/x86_64-unknown-linux-gnu/data.bin".to_owned(),
                    "support/x86_64-unknown-linux-gnu/data.bin".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn a_member_list_beside_an_as_is_refused() {
        // `as` renames one downloaded file; a list is a layout, and there is no one file to mean.
        for entry in [
            "    asset: '{release}/thing.zip'\n    as: thing\n    member: [thing, extra/data.bin]\n    targets:\n      linux-x86_64: x86_64-unknown-linux-gnu",
            "    targets:\n      linux-x86_64:\n        asset: '{release}/thing.zip'\n        as: thing\n        member: [thing, extra/data.bin]",
        ] {
            let yaml = format!(
                "version: 1\nprograms:\n  thing:\n    repository: https://github.com/e/c\n{entry}\n"
            );
            let document = program(&yaml);
            let error = for_host("thing", &document.programs["thing"], "linux-x86_64")
                .expect_err("must be refused");
            let message = error.to_string();
            assert!(message.contains("'as'"), "for {entry:?}: {message}");
        }
    }

    #[test]
    fn a_member_list_with_nothing_in_it_is_refused() {
        // Silently extracting nothing would leave an "installed" dependency with no files.
        let document = program(
            r"
version: 1
programs:
  empty:
    repository: https://github.com/example/empty
    targets:
      linux-x86_64:
        asset: '{release}/empty.tar.gz'
        member: []
",
        );
        let error =
            for_host("empty", &document.programs["empty"], "linux-x86_64").expect_err("refused");
        assert!(error.to_string().contains("no file"), "{error}");
    }

    #[test]
    fn a_listed_member_that_would_leave_the_vendor_folder_is_refused() {
        // `join_normalized` resolves `..` against the folder, so a climbing segment escapes it.
        for bad in [
            "../outside",
            "conpty/../../outside",
            "/etc/passwd",
            "C:/Windows/System32/evil.dll",
            "conpty//OpenConsole.exe",
            "conpty/",
        ] {
            assert!(member_path("x", bad).is_err(), "{bad} must be refused");
        }
        // A plain relative path is kept whole, separators normalised to the archive's own.
        assert_eq!(member_path("x", "herdr.exe").unwrap(), "herdr.exe");
        assert_eq!(
            member_path("x", "conpty\\x64\\OpenConsole.exe").unwrap(),
            "conpty/x64/OpenConsole.exe"
        );
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
