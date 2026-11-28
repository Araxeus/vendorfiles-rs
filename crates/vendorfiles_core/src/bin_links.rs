//! `bin`: commands a dependency exposes from deep inside its own files.
//!
//! A toolchain like Go needs its whole tree, with the executables in `bin/`. Spilled into a
//! folder on `PATH`, the tree's `src/` and `lib/` folders land among everything else, and the
//! commands still sit one folder too deep. `bin` names the files to expose, and each one gets a
//! link at the top of the dependency folder.
//!
//! On Unix a link is a relative symlink, so the program finds its tree through it. Windows only
//! lets an ordinary user make symlinks in Developer Mode, and a program started through a hard
//! link looks for its files beside the link. So there each command gets two launchers: a `.cmd`
//! for cmd, PowerShell and other programs, and an extensionless `sh` script for Git Bash, as npm
//! writes them.

use std::path::{Path, PathBuf};

use crate::error::{Result, VendorError};
use crate::fsx::anchor;

/// One file `bin` writes at the top of the dependency folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The file written, relative to the dependency folder: a bare name.
    pub file: String,
    /// The file it runs, relative to the dependency folder, with `/` separators.
    pub target: String,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A symlink, on Unix.
    Symlink,
    /// A `.cmd` launcher, on Windows.
    Cmd,
    /// An `sh` launcher for Git Bash, on Windows.
    Sh,
}

/// The links `bin` makes on this platform, in order.
#[must_use]
pub fn links(bin: &[String]) -> Vec<Link> {
    links_for(bin, cfg!(windows))
}

/// The links `bin` makes, on Windows or elsewhere.
fn links_for(bin: &[String], windows: bool) -> Vec<Link> {
    bin.iter()
        .filter_map(|entry| normalized(entry).ok())
        .flat_map(|target| {
            let name = command_name(&target, windows).to_owned();
            if windows {
                vec![
                    Link {
                        file: format!("{name}.cmd"),
                        target: target.clone(),
                        kind: Kind::Cmd,
                    },
                    Link {
                        file: name,
                        target,
                        kind: Kind::Sh,
                    },
                ]
            } else {
                vec![Link {
                    file: name,
                    target,
                    kind: Kind::Symlink,
                }]
            }
        })
        .collect()
}

/// The command a target is run as: its file name, without `.exe` on Windows.
fn command_name(target: &str, windows: bool) -> &str {
    let file = target.rsplit('/').next().unwrap_or(target);
    let is_exe = Path::new(file)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
    if windows && is_exe {
        &file[..file.len() - 4]
    } else {
        file
    }
}

/// A `bin` entry as a relative path with `/` separators, or why it cannot be one.
fn normalized(entry: &str) -> std::result::Result<String, String> {
    let mut segments = Vec::new();
    for segment in entry.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => return Err("climbs out of the dependency folder".to_owned()),
            _ if segment.contains(':') => return Err("is not a relative path".to_owned()),
            _ => segments.push(segment),
        }
    }
    if entry.starts_with(['/', '\\']) {
        return Err("is not a relative path".to_owned());
    }
    if entry.ends_with(['/', '\\']) || segments.is_empty() {
        return Err("names a folder, not a file to run".to_owned());
    }
    Ok(segments.join("/"))
}

/// Checks a dependency's `bin` against the files it writes.
///
/// `outputs` are the dependency's literal outputs, relative to its folder. A link may not take
/// the place of one of them, or of another link.
///
/// # Errors
///
/// Returns [`VendorError::InvalidBin`] naming the first entry that cannot be exposed.
pub fn check(name: &str, bin: &[String], outputs: &[String]) -> Result<()> {
    check_for(name, bin, outputs, cfg!(windows))
}

/// [`check`] as it runs on Windows, or elsewhere.
///
/// # Errors
///
/// As [`check`].
pub fn check_for(name: &str, bin: &[String], outputs: &[String], windows: bool) -> Result<()> {
    let invalid = |entry: &str, reason: String| VendorError::InvalidBin {
        name: name.to_owned(),
        entry: entry.to_owned(),
        reason,
    };
    let same = |a: &str, b: &str| {
        if windows {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let outputs: Vec<String> = outputs
        .iter()
        .filter_map(|output| normalized(output).ok())
        .collect();
    let mut taken: Vec<String> = Vec::new();
    for entry in bin {
        let target = normalized(entry).map_err(|reason| invalid(entry, reason))?;
        if !target.contains('/') {
            return Err(invalid(
                entry,
                "is already at the top of the dependency folder".to_owned(),
            ));
        }
        for link in links_for(std::slice::from_ref(entry), windows) {
            if let Some(reason) = reserved(&link.file) {
                return Err(invalid(entry, reason));
            }
            // A folder at the top, like the tree's own, cannot share a link's name.
            let folders = outputs.iter().chain(std::iter::once(&target));
            if folders
                .filter_map(|path| path.split_once('/').map(|(top, _)| top))
                .any(|top| same(top, &link.file))
            {
                return Err(invalid(
                    entry,
                    format!("would be linked as '{}', a folder's name", link.file),
                ));
            }
            if let Some(output) = outputs.iter().find(|output| same(output, &link.file)) {
                return Err(invalid(entry, format!("would replace the file '{output}'")));
            }
            if taken.iter().any(|file| same(file, &link.file)) {
                return Err(invalid(
                    entry,
                    format!("is exposed as '{}' twice", link.file),
                ));
            }
            taken.push(link.file);
        }
    }
    Ok(())
}

/// Why a link may not take the name `file`, if it may not.
///
/// vendor writes its lockfile and config through any link in their place, and Windows keeps
/// device names like `con` whatever their extension.
fn reserved(file: &str) -> Option<String> {
    let lower = file.to_ascii_lowercase();
    if lower == crate::lockfile::LOCKFILE_NAME || lower.starts_with("vendor.") {
        return Some(format!(
            "would be linked as '{file}', which vendor writes itself"
        ));
    }
    let stem = lower.split('.').next().unwrap_or_default();
    let device = matches!(stem, "con" | "prn" | "aux" | "nul")
        || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    device.then(|| format!("would be linked as '{file}', a name Windows reserves"))
}

/// What a launcher holds.
fn launcher(link: &Link) -> Option<String> {
    match link.kind {
        Kind::Symlink => None,
        // `%~dp0` is the launcher's own folder, with a trailing `\`.
        Kind::Cmd => Some(format!(
            "@\"%~dp0{}\" %*\r\n",
            link.target.replace('/', "\\")
        )),
        Kind::Sh => Some(format!(
            "#!/bin/sh\nexec \"$(dirname \"$0\")/{}\" \"$@\"\n",
            link.target
        )),
    }
}

/// Whether `link` is in `folder` as `bin` would write it, and its target is there.
#[must_use]
pub fn is_in_place(folder: &Path, link: &Link) -> bool {
    anchor(folder, &link.target).is_file() && is_ours(&anchor(folder, &link.file), link)
}

/// Whether the file at `path` is exactly what `link` would write.
fn is_ours(path: &Path, link: &Link) -> bool {
    launcher(link).map_or_else(
        || std::fs::read_link(path).is_ok_and(|to| to == Path::new(&link.target)),
        |content| std::fs::read_to_string(path).is_ok_and(|held| held == content),
    )
}

/// Checks, before anything downloads, that no other file holds a link's name.
///
/// # Errors
///
/// Returns [`VendorError::BinInTheWay`] naming the first file in the way.
pub async fn check_free(name: &str, folder: &Path, links: &[Link]) -> Result<()> {
    for link in links {
        let path = anchor(folder, &link.file);
        if tokio::fs::symlink_metadata(&path).await.is_ok() && !is_ours(&path, link) {
            return Err(VendorError::BinInTheWay {
                name: name.to_owned(),
                path,
            });
        }
    }
    Ok(())
}

/// Writes each link into `folder`, after the files they run are in place.
///
/// A link already there as `bin` would write it is kept. Anything else in its place belongs to
/// someone else, so it is left alone and the install fails.
///
/// # Errors
///
/// Returns [`VendorError::BinTargetMissing`] when a target was not installed,
/// [`VendorError::BinInTheWay`] when another file holds a link's name, and an I/O error when a
/// link cannot be written.
pub async fn create(name: &str, folder: &Path, links: &[Link]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::with_capacity(links.len());
    for link in links {
        let target = anchor(folder, &link.target);
        if !target.is_file() {
            return Err(VendorError::BinTargetMissing {
                name: name.to_owned(),
                target: link.target.clone(),
            });
        }
        let path = anchor(folder, &link.file);
        if tokio::fs::symlink_metadata(&path).await.is_ok() {
            if is_ours(&path, link) {
                continue;
            }
            return Err(VendorError::BinInTheWay {
                name: name.to_owned(),
                path,
            });
        }
        write(&path, link).await?;
        written.push(path);
    }
    Ok(written)
}

async fn write(path: &Path, link: &Link) -> Result<()> {
    let fail = |source: std::io::Error| VendorError::WriteFile {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(fail)?;
    }
    if let Some(content) = launcher(link) {
        return tokio::fs::write(path, content).await.map_err(fail);
    }
    #[cfg(unix)]
    let made = tokio::fs::symlink(&link.target, path).await;
    #[cfg(not(unix))]
    let made = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks are only written on Unix",
    ));
    made.map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::{Kind, Link, check_for, create, is_in_place, launcher, links, links_for};

    fn bin(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|&entry| entry.to_owned()).collect()
    }

    #[test]
    fn unix_links_each_command_by_its_file_name() {
        let made = links_for(&bin(&["libexec/go/bin/go", "libexec/go/bin/gofmt"]), false);
        let files: Vec<_> = made.iter().map(|link| link.file.as_str()).collect();
        assert_eq!(files, ["go", "gofmt"]);
        assert_eq!(made[0].target, "libexec/go/bin/go");
        assert_eq!(made[0].kind, Kind::Symlink);
    }

    #[test]
    fn windows_writes_a_cmd_and_an_sh_launcher_without_the_exe() {
        let made = links_for(&bin(&["libexec\\zig\\zig.EXE"]), true);
        let files: Vec<_> = made.iter().map(|link| link.file.as_str()).collect();
        assert_eq!(files, ["zig.cmd", "zig"]);
        assert_eq!(
            launcher(&made[0]).unwrap(),
            "@\"%~dp0libexec\\zig\\zig.EXE\" %*\r\n"
        );
        assert_eq!(
            launcher(&made[1]).unwrap(),
            "#!/bin/sh\nexec \"$(dirname \"$0\")/libexec/zig/zig.EXE\" \"$@\"\n"
        );
    }

    #[test]
    fn an_entry_must_be_a_relative_file_below_the_top() {
        for (entry, reason) in [
            ("../go/bin/go", "climbs out"),
            ("/usr/bin/go", "not a relative path"),
            ("C:/go/bin/go.exe", "not a relative path"),
            ("libexec/go/bin/", "names a folder"),
            ("go", "already at the top"),
        ] {
            let error = check_for("go", &bin(&[entry]), &[], false).expect_err(entry);
            assert!(error.to_string().contains(reason), "{entry}: {error}");
        }
    }

    #[test]
    fn a_link_may_not_replace_a_file_or_another_link() {
        let error = check_for(
            "go",
            &bin(&["libexec/go/bin/go"]),
            &["go".to_owned()],
            false,
        )
        .expect_err("a file holds that name");
        assert!(
            error.to_string().contains("replace the file 'go'"),
            "{error}"
        );

        for (entry, reason) in [
            ("a/vendor-lock.json", "vendor writes itself"),
            ("a/vendor.yml", "vendor writes itself"),
            ("a/nul", "Windows reserves"),
            ("a/COM1.exe", "Windows reserves"),
            ("libexec/x/libexec", "a folder's name"),
        ] {
            let error = check_for("x", &bin(&[entry]), &[], false).expect_err(entry);
            assert!(error.to_string().contains(reason), "{entry}: {error}");
        }
        let error = check_for(
            "x",
            &bin(&["t/bin/tool"]),
            &["tool/readme".to_owned()],
            false,
        )
        .expect_err("a folder holds that name");
        assert!(error.to_string().contains("a folder"), "{error}");

        let error = check_for("x", &bin(&["a/tool", "b/tool"]), &[], false).expect_err("twice");
        assert!(error.to_string().contains("twice"), "{error}");

        // Windows names match in any case, and `.exe` drops out.
        let error = check_for("x", &bin(&["a/Tool.exe", "b/tool.exe"]), &[], true)
            .expect_err("one name on Windows");
        assert!(error.to_string().contains("twice"), "{error}");
        check_for("x", &bin(&["a/Tool", "b/tool"]), &[], false).expect("two names on Unix");
    }

    #[tokio::test]
    async fn links_are_written_once_and_kept_when_already_in_place() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("libexec/tool/bin")).unwrap();
        std::fs::write(dir.path().join("libexec/tool/bin/tool"), "x").unwrap();
        let made = links(&bin(&["libexec/tool/bin/tool"]));

        let written = create("tool", dir.path(), &made).await.unwrap();
        assert_eq!(written.len(), made.len());
        assert!(made.iter().all(|link| is_in_place(dir.path(), link)));

        let again = create("tool", dir.path(), &made).await.unwrap();
        assert_eq!(
            again,
            Vec::<std::path::PathBuf>::new(),
            "an existing link is kept"
        );
    }

    #[tokio::test]
    async fn someone_else_s_file_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("libexec/tool")).unwrap();
        std::fs::write(dir.path().join("libexec/tool/tool"), "x").unwrap();
        let made = links(&bin(&["libexec/tool/tool"]));
        std::fs::write(dir.path().join(&made[0].file), "mine").unwrap();

        let error = create("tool", dir.path(), &made)
            .await
            .expect_err("in the way");
        assert!(error.to_string().contains("in the way"), "{error}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&made[0].file)).unwrap(),
            "mine"
        );
    }

    #[tokio::test]
    async fn a_target_that_was_not_installed_fails() {
        let dir = tempfile::tempdir().unwrap();
        let error = create("tool", dir.path(), &links(&bin(&["libexec/tool/tool"])))
            .await
            .expect_err("no target");
        assert!(error.to_string().contains("libexec/tool/tool"), "{error}");
        assert!(!is_in_place(
            dir.path(),
            &Link {
                file: "tool".to_owned(),
                target: "libexec/tool/tool".to_owned(),
                kind: Kind::Sh,
            }
        ));
    }
}
