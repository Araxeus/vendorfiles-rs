//! Filesystem helpers with Node-compatible path semantics.
//!
//! Paths appear verbatim in the tool's output (`INFO: Saved <path>`), so joining and
//! normalisation must match Node's `path.join` / `fs.realpath` rather than Rust's defaults -
//! in particular Rust's `canonicalize` returns `\\?\C:\…` on Windows, which Node never prints.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::error::{Result, VendorError};
use crate::progress::Transfer;

/// Lexically normalises a path: drops `.`, resolves `..`, and unifies separators.
///
/// Purely textual, like Node's `path.normalize` - the filesystem is never consulted.
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

/// Where a declared path lands, given the folder it belongs to.
///
/// A relative path hangs off `folder`, which is the ordinary case and keeps a checked-in project
/// working wherever it is cloned. An absolute one names a destination of its own and is taken at
/// its word: `vendorFolder` has always struck that bargain, and a single file gets it too, so one
/// binary can be dropped somewhere on `PATH` without moving the rest of the dependency.
///
/// The one rule for *where things go*, so placing a file and deleting it later cannot disagree.
#[must_use]
pub fn anchor(folder: &Path, path: &str) -> PathBuf {
    // `has_root` rather than `is_absolute` so a leading separator counts on Windows too;
    // `C:relative`, which has no root, stays relative on both.
    if Path::new(path).has_root() {
        normalize(Path::new(path))
    } else {
        join_normalized(folder, &[path])
    }
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
/// A running executable cannot simply be overwritten - on Windows its image is locked for the
/// lifetime of the process - so the swap is left to `self-replace`, which moves the old image
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
/// Stops at `root` and at the first non-empty directory.
///
/// A file that is already gone counts as deleted, and so does a `root` that is: this is asked
/// for a state rather than an action, and `uninstall` has to be able to drop a dependency whose
/// files the user removed by hand. The reference fails in that case and leaves callers to decide
/// whether it matters, which is what let a delete that *could not* be done pass for one that had
/// been - see §6.21.
///
/// # Errors
///
/// Returns [`VendorError::DeleteFailed`] if the file is there and will not go; on Windows the
/// usual reason is that it is an executable which is currently running. Returns
/// [`VendorError::ReadFile`] if `root` exists but cannot be resolved.
pub async fn delete_file_and_empty_folders(root: &Path, relative_path: &str) -> Result<()> {
    // Asked for a state, not an action: what is already gone needs no deleting, and `uninstall`
    // has to be able to drop a dependency whose files the user removed by hand. Every other
    // failure is returned - a file held open by a running program is not "already gone", and
    // calling it that is how an orphan gets left behind with nothing left recording it.
    if !root.exists() {
        return Ok(());
    }
    let root = real_path(root)?;
    let filepath = anchor(&root, relative_path);
    match tokio::fs::remove_file(&filepath).await {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(VendorError::DeleteFailed {
                path: filepath,
                source,
            });
        }
    }

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
/// Always reports. The reference had a `log` flag here that decided both whether to announce the
/// file and whether a write error was fatal, and the archive path passed it `false` so that a
/// temp archive which failed to save arrived as "cannot be extracted" instead. That sentence is
/// still what the user sees - [`download_and_extract`](crate::ops::install) puts it there - but
/// the reason is now carried underneath it rather than dropped here, where nothing else knew
/// whether the disk was full or the file was held open.
///
/// # Errors
///
/// Returns [`VendorError::SaveFailed`] if the body cannot be written.
pub async fn stream_to_file(
    response: reqwest::Response,
    save_path: &Path,
    transfer: Option<&Transfer<'_>>,
) -> Result<()> {
    write_stream(response, save_path, transfer)
        .await
        .map_err(|source| VendorError::SaveFailed {
            path: save_path.display().to_string(),
            source,
        })
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
    // What the server said it would send, so a body that stops early can be told from one that
    // simply ended.
    let promised = response.content_length();
    let mut written = 0_u64;
    let mut file = tokio::fs::File::create(save_path).await?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(std::io::Error::other)?;
        file.write_all(&chunk).await?;
        written += chunk.len() as u64;
        if let Some(transfer) = transfer {
            transfer.advance(chunk.len() as u64);
        }
    }
    file.flush().await?;

    // The transport already enforces `Content-Length` - a body that stops early surfaces as
    // "error decoding response body", which the tests below pin down. This is the second line of
    // defence, and it states the invariant in the code rather than leaving it to a dependency:
    // what gets saved is the whole asset. It compares against the length reqwest reports *after*
    // any decoding, so an encoded body is not mistaken for a short one.
    if let Some(promised) = promised
        && written != promised
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("expected {promised} bytes, received {written}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        anchor, is_running_executable, join_normalized, normalize, simplify, staged_beside,
        stream_to_file,
    };
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

    /// Serves one response with `length` promised and `body` actually sent, then hangs up.
    ///
    /// A real socket rather than a mocked client: the question is what happens to a body that
    /// stops early, and only the transport can answer it.
    fn serve_once(length: usize, body: &'static [u8]) -> String {
        serve(length, body.to_vec(), "")
    }

    /// As above, with extra response headers.
    fn serve(length: usize, body: Vec<u8>, extra: &str) -> String {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().unwrap().port();
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n{extra}\r\n");
        std::thread::spawn(move || {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 1024];
            let _ = socket.read(&mut request);
            let _ = socket.write_all(head.as_bytes());
            let _ = socket.write_all(&body);
            let _ = socket.flush();
            // Dropping the socket ends the body, short of what was promised when it is shorter.
        });
        format!("http://127.0.0.1:{port}/asset")
    }

    #[tokio::test]
    async fn a_body_that_stops_short_is_refused() {
        // The failure this guards against: a bare binary - `yt-dlp.exe`, `ox.exe` - saved
        // half-downloaded and reported as a success.
        let dir = tempfile::tempdir().unwrap();
        let save_path = dir.path().join("yt-dlp.exe");
        let url = serve_once(4096, b"only the first few bytes");

        let response = crate::github::http::client()
            .expect("a client")
            .get(&url)
            .send()
            .await
            .expect("a response");
        let outcome = stream_to_file(response, &save_path, None).await;

        // Refused either by the transport, which enforces `Content-Length`, or by the byte count
        // below it. Which one wins is not the point; that it never succeeds is.
        let error = outcome.expect_err("a truncated download must not succeed");
        assert!(
            error.to_string().contains(&save_path.display().to_string()),
            "the error should name the file: {error}"
        );
    }

    #[tokio::test]
    async fn a_complete_body_is_written_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let save_path = dir.path().join("tool.bin");
        let body = b"the whole thing";
        let url = serve_once(body.len(), body);

        let response = crate::github::http::client()
            .expect("a client")
            .get(&url)
            .send()
            .await
            .expect("a response");
        stream_to_file(response, &save_path, None)
            .await
            .expect("a complete download");
        assert_eq!(std::fs::read(&save_path).unwrap(), body);
    }

    #[tokio::test]
    async fn an_encoded_body_is_not_mistaken_for_a_short_one() {
        // `reqwest` decompresses transparently. If it reported the *compressed* length while
        // handing over decompressed bytes, comparing the two counts would reject every encoded
        // download - so this pins which length the completeness check is comparing against.
        use std::io::Write;

        let plain = vec![b'a'; 4096];
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&plain).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(
            compressed.len() < plain.len(),
            "the test needs real compression"
        );

        let dir = tempfile::tempdir().unwrap();
        let save_path = dir.path().join("theme.json");
        let url = serve(compressed.len(), compressed, "Content-Encoding: gzip\r\n");

        let response = crate::github::http::client()
            .expect("a client")
            .get(&url)
            .send()
            .await
            .expect("a response");
        stream_to_file(response, &save_path, None)
            .await
            .expect("an encoded body is complete, not short");
        assert_eq!(std::fs::read(&save_path).unwrap(), plain);
    }

    #[tokio::test]
    async fn a_short_body_is_reported_even_on_the_archive_path() {
        // It used to be swallowed here so the follow-on error read "cannot be extracted". The
        // user still reads that sentence - `download_and_extract` writes it - but the reason
        // now travels with it instead of dying at this line.
        let dir = tempfile::tempdir().unwrap();
        let save_path = dir.path().join("asset.zip");
        let url = serve_once(4096, b"truncated");

        let response = crate::github::http::client()
            .expect("a client")
            .get(&url)
            .send()
            .await
            .expect("a response");
        let error = stream_to_file(response, &save_path, None)
            .await
            .expect_err("a truncated archive must not pass for a saved one");
        assert!(
            matches!(error, crate::VendorError::SaveFailed { .. }),
            "{error}"
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
    fn a_relative_destination_hangs_off_the_folder() {
        let folder = PathBuf::from("proj").join("vendor").join("dep");
        assert_eq!(anchor(&folder, "tool.exe"), folder.join("tool.exe"));
        assert_eq!(
            anchor(&folder, "bin/tool.exe"),
            folder.join("bin").join("tool.exe")
        );
        // `..` still climbs, as it does for any relative output the config declares.
        assert_eq!(
            anchor(&folder, "../licenses/L"),
            PathBuf::from("proj")
                .join("vendor")
                .join("licenses")
                .join("L")
        );
    }

    #[test]
    fn an_absolute_destination_is_taken_at_its_word() {
        // What `vendorFolder` has always done, now for a single file too: one binary can land on
        // `PATH` without the rest of the dependency moving with it.
        let folder = PathBuf::from("proj").join("vendor").join("dep");

        assert_eq!(
            anchor(&folder, "/opt/tools/tool"),
            normalize(Path::new("/opt/tools/tool"))
        );
        #[cfg(windows)]
        assert_eq!(
            anchor(&folder, r"C:\tools\tool.exe"),
            PathBuf::from(r"C:\tools\tool.exe")
        );

        // `C:relative` has no root on either platform, so it takes the relative branch rather
        // than being mistaken for a destination of its own. What `normalize` then makes of a
        // drive-prefixed segment mid-path is its own affair, and not what this rule decides.
        assert_eq!(
            anchor(&folder, "C:relative"),
            join_normalized(&folder, &["C:relative"])
        );
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
