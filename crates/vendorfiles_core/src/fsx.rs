//! Filesystem helpers with Node-compatible path semantics.
//!
//! Paths appear verbatim in the tool's output (`INFO: Saved <path>`), so joining and
//! normalisation must match Node's `path.join` / `fs.realpath` rather than Rust's defaults —
//! in particular Rust's `canonicalize` returns `\\?\C:\…` on Windows, which Node never prints.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::error::{Result, VendorError};
use crate::progress::Transfer;

/// Lexically normalises a path: drops `.`, resolves `..`, and unifies separators.
///
/// Purely textual, like Node's `path.normalize` — the filesystem is never consulted.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut rooted = false;
    let mut normals = 0usize;

    for component in path.components() {
        match component {
            Component::Prefix(_) => out.push(component.as_os_str()),
            Component::RootDir => {
                rooted = true;
                out.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normals > 0 {
                    out.pop();
                    normals -= 1;
                } else if !rooted {
                    out.push("..");
                }
            }
            Component::Normal(part) => {
                out.push(part);
                normals += 1;
            }
        }
    }

    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Joins path segments the way Node's `path.join` does: concatenate, then normalise.
///
/// Unlike [`PathBuf::push`], a segment starting with a separator does *not* discard the base.
#[must_use]
pub fn join_normalized(base: &Path, parts: &[&str]) -> PathBuf {
    let mut joined = base.as_os_str().to_string_lossy().into_owned();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !joined.is_empty() && !joined.ends_with(['/', '\\', MAIN_SEPARATOR]) {
            joined.push(MAIN_SEPARATOR);
        }
        joined.push_str(part);
    }
    normalize(Path::new(&joined))
}

/// Whether `text` begins with a plain drive root such as `C:\`.
fn starts_with_drive(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && chars.next() == Some('\\')
}

/// Strips Windows' `\\?\` verbatim prefix from a simple drive path.
#[must_use]
pub fn simplify(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\")
        && starts_with_drive(rest)
    {
        return PathBuf::from(rest);
    }
    path
}

/// Resolves a path to its canonical location, in the shape Node's `fs.realpath` returns.
///
/// # Errors
///
/// Returns [`VendorError::ReadFile`] if the path does not exist or cannot be resolved.
pub fn real_path(path: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(path)
        .map(simplify)
        .map_err(|source| VendorError::ReadFile {
            path: path.to_path_buf(),
            source,
        })
}

/// Whether `path` is the binary that is currently running.
///
/// Compared canonically, so `./vendor.exe`, a relative path and a symlink all recognise the same
/// file. A path that does not exist yet cannot be the running binary, so anything that fails to
/// resolve is simply `false`.
#[must_use]
pub fn is_running_executable(path: &Path) -> bool {
    let Ok(current) = std::env::current_exe() else {
        return false;
    };
    match (std::fs::canonicalize(path), std::fs::canonicalize(&current)) {
        (Ok(candidate), Ok(current)) => candidate == current,
        _ => false,
    }
}

/// Replaces the running binary with the file at `staged`, consuming it.
///
/// A running executable cannot simply be overwritten — on Windows its image is locked for the
/// lifetime of the process — so the swap is left to `self-replace`, which moves the old image
/// aside and has the operating system delete it once this process exits.
///
/// # Errors
///
/// Returns [`VendorError::SaveFailed`] if the swap fails; the running binary is left intact.
pub async fn replace_running_executable(staged: &Path) -> Result<()> {
    let staged = staged.to_path_buf();
    let display = staged.display().to_string();
    tokio::task::spawn_blocking(move || {
        // `self-replace` does not document what it does with permissions, so the staged file is
        // given the mode of the binary it is about to become rather than whatever it inherited
        // from the archive or the temporary directory.
        copy_executable_mode(&staged)?;
        let outcome = self_replace::self_replace(&staged);
        // Its contract: the caller owns the staged file afterwards, success or not.
        let _ = std::fs::remove_file(&staged);
        outcome
    })
    .await
    .map_err(|joined| VendorError::Http(joined.to_string()))?
    .map_err(|source| VendorError::SaveFailed {
        path: display,
        source,
    })
}

/// Gives `staged` the permissions of the running binary.
#[cfg(unix)]
fn copy_executable_mode(staged: &Path) -> std::result::Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    let current = std::env::current_exe()?;
    let mode = std::fs::metadata(&current)?.permissions().mode();
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(mode))
}

/// Windows decides executability by extension, so there is nothing to copy.
#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the unix signature, which can fail"
)]
const fn copy_executable_mode(_staged: &Path) -> std::result::Result<(), std::io::Error> {
    Ok(())
}

/// Deletes `relative_path` under `root`, then prunes the directories it leaves empty.
///
/// Stops at `root` and at the first non-empty directory. Fails if the file is not there,
/// matching the reference — callers decide whether that matters.
///
/// # Errors
///
/// Returns [`VendorError::ReadFile`] if `root` cannot be resolved or the file is not there.
pub async fn delete_file_and_empty_folders(root: &Path, relative_path: &str) -> Result<()> {
    let root = real_path(root)?;
    let filepath = join_normalized(&root, &[relative_path]);
    tokio::fs::remove_file(&filepath)
        .await
        .map_err(|source| VendorError::ReadFile {
            path: filepath.clone(),
            source,
        })?;

    let mut dir = filepath.parent().map(Path::to_path_buf);
    while let Some(current) = dir {
        if current == root || !current.starts_with(&root) {
            break;
        }
        let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
            break;
        };
        if entries.next_entry().await.ok().flatten().is_some() {
            break;
        }
        if tokio::fs::remove_dir_all(&current).await.is_err() {
            break;
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    Ok(())
}

/// Streams an HTTP response body to `save_path`, creating parent directories.
///
/// `report_failures` mirrors the reference's `log` flag, which decided both whether to announce
/// the file and whether a write error was fatal. Announcing is now the caller's job — it
/// batches the lines so `sync` can keep them in dependency order — but the error behaviour is
/// preserved: a silent write failure is swallowed so the caller reports the follow-on error
/// (a temp archive that fails to save shows up as "cannot be extracted").
///
/// # Errors
///
/// Returns [`VendorError::SaveFailed`] if the body cannot be written and `report_failures` is
/// set; otherwise write failures are swallowed.
pub async fn stream_to_file(
    response: reqwest::Response,
    save_path: &Path,
    report_failures: bool,
    transfer: Option<&Transfer<'_>>,
) -> Result<()> {
    match write_stream(response, save_path, transfer).await {
        Err(source) if report_failures => Err(VendorError::SaveFailed {
            path: save_path.display().to_string(),
            source,
        }),
        Ok(()) | Err(_) => Ok(()),
    }
}

async fn write_stream(
    response: reqwest::Response,
    save_path: &Path,
    transfer: Option<&Transfer<'_>>,
) -> std::result::Result<(), std::io::Error> {
    if is_running_executable(save_path) {
        // Streaming straight onto the running binary would fail on Windows and corrupt the
        // image everywhere else; stage it beside the target and let the swap be atomic.
        let staged = staged_beside(save_path);
        write_bytes(response, &staged, transfer).await?;
        return replace_running_executable(&staged)
            .await
            .map_err(std::io::Error::other);
    }
    write_bytes(response, save_path, transfer).await
}

/// A temporary path next to `target`, so the swap never crosses a filesystem.
fn staged_beside(target: &Path) -> PathBuf {
    let directory = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map_or_else(|| "vendor".into(), |name| name.to_string_lossy());
    directory.join(format!(".{name}.vendorfiles-update"))
}

async fn write_bytes(
    response: reqwest::Response,
    save_path: &Path,
    transfer: Option<&Transfer<'_>>,
) -> std::result::Result<(), std::io::Error> {
    if let Some(parent) = save_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::File::create(save_path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(std::io::Error::other)?;
        file.write_all(&chunk).await?;
        if let Some(transfer) = transfer {
            transfer.advance(chunk.len() as u64);
        }
    }
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_running_executable, join_normalized, normalize, simplify, staged_beside};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_running_binary_recognises_itself() {
        let current = std::env::current_exe().expect("a test binary is running");
        assert!(is_running_executable(&current));
    }

    #[test]
    fn the_running_binary_is_recognised_through_a_relative_path() {
        // Installing writes a joined path, not a canonical one, so the comparison has to
        // resolve both sides.
        let current = std::env::current_exe().unwrap();
        let directory = current.parent().unwrap();
        let indirect = directory.join(".").join(current.file_name().unwrap());
        assert!(is_running_executable(&indirect));
    }

    #[test]
    fn an_ordinary_file_is_not_the_running_binary() {
        let file = tempfile::NamedTempFile::new().unwrap();
        assert!(!is_running_executable(file.path()));
    }

    #[test]
    fn a_path_that_does_not_exist_is_not_the_running_binary() {
        // A first install has nothing at the destination yet, and must not be mistaken for a
        // self-update.
        assert!(!is_running_executable(Path::new("no/such/vendor.exe")));
        assert!(!is_running_executable(Path::new("")));
    }

    #[test]
    fn the_staging_path_sits_beside_its_target() {
        let staged = staged_beside(Path::new("/tools/bin/vendor.exe"));
        assert_eq!(staged.parent(), Some(Path::new("/tools/bin")));
        assert_ne!(staged.file_name(), Some(std::ffi::OsStr::new("vendor.exe")));
        // Beside it, so the swap is a rename within one filesystem rather than a copy across.
        assert!(
            staged
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("vendor.exe")
        );
    }

    #[test]
    fn normalize_resolves_dot_segments() {
        assert_eq!(normalize(Path::new("a/./b")), PathBuf::from("a").join("b"));
        assert_eq!(
            normalize(Path::new("a/b/../c")),
            PathBuf::from("a").join("c")
        );
        assert_eq!(normalize(Path::new("")), PathBuf::from("."));
        assert_eq!(normalize(Path::new("./")), PathBuf::from("."));
    }

    #[test]
    fn normalize_keeps_leading_parents_only_when_relative() {
        assert_eq!(normalize(Path::new("../a")), PathBuf::from("..").join("a"));
        #[cfg(unix)]
        assert_eq!(normalize(Path::new("/a/../../b")), PathBuf::from("/b"));
    }

    #[test]
    fn join_does_not_let_a_rooted_segment_escape_the_base() {
        let joined = join_normalized(Path::new("proj"), &["/abs", "x"]);
        assert_eq!(joined, PathBuf::from("proj").join("abs").join("x"));
    }

    #[test]
    fn join_expands_relative_outputs() {
        assert_eq!(
            join_normalized(Path::new("proj/vendor/dep"), &["../licenses/L"]),
            PathBuf::from("proj")
                .join("vendor")
                .join("licenses")
                .join("L")
        );
    }

    #[test]
    fn simplify_strips_the_windows_verbatim_prefix() {
        assert_eq!(
            simplify(PathBuf::from(r"\\?\C:\proj\vendor")),
            PathBuf::from(r"C:\proj\vendor")
        );
        assert_eq!(simplify(PathBuf::from("/tmp/x")), PathBuf::from("/tmp/x"));
    }
}
