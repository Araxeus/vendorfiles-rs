//! Reading named members out of a remote ZIP without downloading the rest of it.
//!
//! A release asset is routinely far bigger than the files a dependency declares: every
//! platform's binary, a manual, completions for shells nobody asked about. Downloading all of
//! it to move two files out of the pile pays for the whole asset twice - once over the network,
//! once onto the disk.
//!
//! A ZIP is the one common container that can be read out of order, because its index - the
//! central directory - sits at the *end* of the file. Given a server that honours `Range`, three
//! reads are enough: the tail, which holds the index; the index, which says where each member's
//! bytes are; and each wanted member's own span. Nothing else is transferred.
//!
//! The ZIP itself is parsed by the `zip` crate, the same one the local extraction uses, over the
//! [`RangeReader`] below - so Zip64, the compression methods, the data-descriptor rules and every
//! other corner of the format are handled by code that is already tested, rather than by an
//! end-of-central-directory scanner written here.
//!
//! `.tar.gz` gets none of this, and cannot. Tar has no index at all, and DEFLATE is one
//! continuous stream whose every byte depends on the ones before it, so there is no offset to
//! start from - the tail of a `.tar.gz` is only reachable by decompressing everything ahead of
//! it. What is done for those instead is to stop *unpacking* early, in
//! [`archive::extract_members`](crate::archive::extract_members).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{Result, VendorError};

/// How much of an object the first cache miss pulls down.
///
/// Small, because most members wanted out of a release asset are: a single request of this size
/// covers a whole binary of a few tens of kilobytes, and overshooting it costs real bytes.
const MIN_WINDOW: u64 = 64 * 1024;

/// The largest a window grows to while reads keep running off its end.
///
/// A member far bigger than [`MIN_WINDOW`] would otherwise be fetched a page at a time; doubling
/// up to here brings a multi-megabyte one down in a handful of requests instead.
const MAX_WINDOW: u64 = 8 * 1024 * 1024;

/// How much of the tail is fetched before the index is looked for.
///
const TAIL: u64 = 64 * 1024;

/// The asset size below which ranges are not worth attempting.
///
/// Three round trips against a small asset lose to one download of it, whatever is inside.
pub const WORTH_RANGING: u64 = 4 * 1024 * 1024;

/// A fixed remote object that byte ranges can be pulled out of.
///
/// Blocking, and driven from a blocking task: what consumes it is a `Read + Seek` for the `zip`
/// crate, and there is no async equivalent of that reader in the tree.
pub trait RangeSource {
    /// The object's total size in bytes.
    ///
    /// Named `size` rather than `len` because a range source is never empty in any sense that
    /// would make an `is_empty` mean anything.
    fn size(&self) -> u64;

    /// The bytes of `start..end`, which callers keep inside `0..len`.
    ///
    /// # Errors
    ///
    /// Returns an error if the range cannot be fetched, or comes back as anything other than
    /// exactly the bytes asked for.
    fn fetch(&self, start: u64, end: u64) -> Result<Vec<u8>>;
}

/// A seekable view over a [`RangeSource`], holding one window of it at a time.
///
/// One window rather than a cache of them: every read pattern the `zip` crate produces here is
/// either a scan of the tail or a walk forwards through one member, so a second window would
/// only ever hold bytes nothing asks for again.
pub struct RangeReader<S> {
    source: S,
    position: u64,
    window: Vec<u8>,
    window_start: u64,
    /// How much the next miss will fetch. Grown by sequential reads, reset by a jump.
    window_size: u64,
}

impl<S: RangeSource> RangeReader<S> {
    /// A reader over `source`, positioned at the start.
    pub const fn new(source: S) -> Self {
        Self {
            source,
            position: 0,
            window: Vec::new(),
            window_start: 0,
            window_size: MIN_WINDOW,
        }
    }

    /// Pulls the object's tail down before anything has read from it.
    ///
    /// The `zip` crate finds the index by scanning backwards from the end a kilobyte at a time.
    /// Left to the ordinary window logic that would be a request per kilobyte scanned; fetching
    /// the tail up front makes the scan, and usually the whole index it leads to, one request.
    ///
    /// # Errors
    ///
    /// Returns whatever the source's fetch produced.
    pub fn prime_tail(&mut self) -> Result<()> {
        let len = self.source.size();
        self.load(len.saturating_sub(TAIL), len)
    }

    /// Whether the window currently held covers `position`.
    const fn holds(&self, position: u64) -> bool {
        position >= self.window_start && position - self.window_start < self.window.len() as u64
    }

    /// How much to fetch for a miss at `position`, as ordinary read-ahead.
    ///
    /// A read that ran off the end of the window is walking through a member, and the next one
    /// will too, so the window doubles. A read that jumped somewhere else starts small again -
    /// it is usually a local file header, and fetching megabytes to find one would undo the
    /// point of reading the archive this way.
    fn window_for(&mut self, position: u64) -> u64 {
        let sequential = position == self.window_start + self.window.len() as u64;
        self.window_size = if sequential {
            self.window_size.saturating_mul(2).min(MAX_WINDOW)
        } else {
            MIN_WINDOW
        };
        self.window_size
    }

    fn load(&mut self, start: u64, end: u64) -> Result<()> {
        self.window = self.source.fetch(start, end)?;
        self.window_start = start;
        Ok(())
    }
}

impl<S: RangeSource> Read for RangeReader<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = self.source.size();
        if buf.is_empty() || self.position >= len {
            return Ok(0);
        }
        if !self.holds(self.position) {
            let end = self
                .position
                .saturating_add(self.window_for(self.position))
                .min(len);
            self.load(self.position, end)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        }

        let offset = usize::try_from(self.position - self.window_start)
            .map_err(|_| std::io::Error::other("window offset out of range"))?;
        let available = &self.window[offset..];
        let taken = available.len().min(buf.len());
        buf[..taken].copy_from_slice(&available[..taken]);
        self.position += taken as u64;
        Ok(taken)
    }
}

impl<S: RangeSource> Seek for RangeReader<S> {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        // Signed arithmetic in a wider type: a seek relative to the end or to the current
        // position is allowed to be negative, and only the *result* has to land in range.
        let target = match from {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.source.size()) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        // Seeking past the end is legal and reads nothing, as it does on a file.
        self.position = u64::try_from(target).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start")
        })?;
        Ok(self.position)
    }
}

/// Writes the members `wanted` names out of a remote ZIP into `dest`.
///
/// Returns whether every name was found and written. `false` is not a failure: it means this
/// route did not produce the whole selection - a member is a directory, a name is spelled
/// differently inside the archive - and the caller should download the asset and extract it the
/// ordinary way.
///
/// Blocking; call from [`tokio::task::spawn_blocking`].
///
/// # Errors
///
/// Returns an error if a range request fails, if the ZIP cannot be opened, or if a member
/// cannot be written out. Callers treat an error and `false` the same way, because the answer to
/// both is to download the asset - the distinction is only there for anyone debugging which of
/// the two happened.
pub fn extract_members<S: RangeSource>(source: S, dest: &Path, wanted: &[String]) -> Result<bool> {
    let mut reader = RangeReader::new(source);
    reader.prime_tail()?;
    crate::archive::extract_zip_members(reader, dest, wanted)
}

/// A release asset served over HTTP, read through `Range` requests.
///
/// Holds a runtime handle rather than being async itself: the `zip` reader on the other side is
/// blocking, so each fetch hands its request to the runtime and waits on a channel. The wait is
/// a `blocking_recv` rather than a `block_on` because this runs on a blocking thread, where
/// driving a future directly depends on the runtime's flavour.
pub struct HttpRangeSource {
    client: reqwest::Client,
    url: String,
    size: u64,
    handle: tokio::runtime::Handle,
    /// Where arriving bytes are reported, so the display can count them as they land.
    arrivals: Option<tokio::sync::mpsc::UnboundedSender<u64>>,
}

impl HttpRangeSource {
    /// A source over `len` bytes at `url`, fetched on `client`.
    #[must_use]
    pub const fn new(
        client: reqwest::Client,
        url: String,
        size: u64,
        handle: tokio::runtime::Handle,
        arrivals: Option<tokio::sync::mpsc::UnboundedSender<u64>>,
    ) -> Self {
        Self {
            client,
            url,
            size,
            handle,
            arrivals,
        }
    }
}

impl RangeSource for HttpRangeSource {
    fn size(&self) -> u64 {
        self.size
    }

    fn fetch(&self, start: u64, end: u64) -> Result<Vec<u8>> {
        let client = self.client.clone();
        let url = self.url.clone();
        let total = self.size;
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.handle.spawn(async move {
            // The receiver going away means the blocking side gave up; nothing to report to.
            let _ = tx.send(get_range(&client, &url, start, end, total).await);
        });
        let bytes = rx
            .blocking_recv()
            .map_err(|_| VendorError::Http("a range request was dropped".to_owned()))??;

        if let Some(arrivals) = &self.arrivals {
            let _ = arrivals.send(bytes.len() as u64);
        }
        Ok(bytes)
    }
}

/// Fetches exactly `start..end`, refusing anything that is not that.
async fn get_range(
    client: &reqwest::Client,
    url: &str,
    start: u64,
    end: u64,
    total: u64,
) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .header(reqwest::header::RANGE, format!("bytes={start}-{}", end - 1))
        // Asked for explicitly because reqwest is built with `gzip`, which otherwise advertises
        // it: a body the server re-encoded on the way out has nothing to do with the byte
        // offsets the index named.
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|e| VendorError::Http(e.to_string()))?;

    // Anything but a 206 means the range was not honoured. A 200 in particular is the dangerous
    // one: the server is handing over the whole object, and reading it as though it started at
    // `start` would silently produce the wrong bytes.
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(VendorError::Http(format!(
            "range request answered with {}",
            response.status()
        )));
    }

    let advertised = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if !range_matches(&advertised, start, end, total) {
        return Err(VendorError::Http(format!(
            "asked for bytes {start}-{} of {total}, got `{advertised}`",
            end - 1
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| VendorError::Http(e.to_string()))?;
    if bytes.len() as u64 != end - start {
        return Err(VendorError::Http(format!(
            "range of {} bytes answered with {}",
            end - start,
            bytes.len()
        )));
    }
    Ok(bytes.to_vec())
}

/// Whether a `Content-Range` header says exactly what was asked for.
///
/// Parsed rather than string-compared: the header's spacing is not something to depend on, and
/// the one thing that matters - that these are the bytes requested, out of the object whose
/// length the index was read against - is worth checking on every single request.
fn range_matches(header: &str, start: u64, end: u64, total: u64) -> bool {
    let Some(rest) = header.trim().strip_prefix("bytes") else {
        return false;
    };
    let Some((span, size)) = rest.trim_start().split_once('/') else {
        return false;
    };
    let Some((first, last)) = span.split_once('-') else {
        return false;
    };
    first.trim().parse::<u64>() == Ok(start)
        && last.trim().parse::<u64>() == Ok(end - 1)
        && size.trim().parse::<u64>() == Ok(total)
}

#[cfg(test)]
mod tests {
    use super::{RangeReader, RangeSource, extract_members, range_matches};
    use crate::error::Result;
    use std::cell::RefCell;
    use std::io::{Read, Seek, SeekFrom, Write};

    /// A source over bytes already in hand, which records every range asked of it.
    struct Recording {
        bytes: Vec<u8>,
        requests: RefCell<Vec<(u64, u64)>>,
    }

    impl Recording {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                bytes,
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl RangeSource for &Recording {
        fn size(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn fetch(&self, start: u64, end: u64) -> Result<Vec<u8>> {
            self.requests.borrow_mut().push((start, end));
            let start = usize::try_from(start).unwrap();
            let end = usize::try_from(end).unwrap();
            Ok(self.bytes[start..end].to_vec())
        }
    }

    /// A ZIP holding `wanted` plus `padding` bytes of a second, incompressible-ish member.
    fn zip_with_padding(wanted: &str, bulk: &str, padding: usize) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut out);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file::<_, ()>(wanted, options).unwrap();
            writer.write_all(b"hi").unwrap();
            // Stored rather than deflated: the padding is here to make the archive big, and
            // anything compressible would collapse to nothing and prove nothing.
            writer
                .start_file::<_, ()>(
                    bulk,
                    options.compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(&vec![0u8; padding]).unwrap();
            writer.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn a_member_is_pulled_out_without_reading_the_rest_of_the_archive() {
        let bytes = zip_with_padding("bin/tool", "bulk.bin", 4 * 1024 * 1024);
        let total = bytes.len() as u64;
        let source = Recording::new(bytes);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");

        let found = extract_members(&source, &dest, &["bin/tool".to_owned()]).unwrap();

        assert!(found, "the member is in the archive");
        assert_eq!(
            std::fs::read_to_string(dest.join("bin/tool")).unwrap(),
            "hi"
        );
        assert!(!dest.join("bulk.bin").exists());

        // The whole point: a fraction of the archive crossed the wire. The index is in the tail
        // and the member is at the front, so two windows are all this can take.
        let requests = source.requests.borrow();
        let transferred: u64 = requests.iter().map(|(start, end)| end - start).sum();
        assert_eq!(
            requests.len(),
            2,
            "one tail for the index, one window for the member: {requests:?}"
        );
        assert!(
            transferred < total / 10,
            "transferred {transferred} of {total} bytes"
        );
    }

    #[test]
    fn a_member_the_archive_does_not_hold_is_reported_rather_than_failing() {
        let bytes = zip_with_padding("bin/tool", "bulk.bin", 1024);
        let source = Recording::new(bytes);
        let dir = tempfile::tempdir().unwrap();

        let found =
            extract_members(&source, &dir.path().join("out"), &["bin/other".to_owned()]).unwrap();
        assert!(!found, "the caller has to fall back, not see an error");
    }

    /// Proves the whole route against GitHub's real release storage: the redirect to a signed
    /// URL, the `Accept-Ranges` probe, and a member read out of a 13 MB asset for a few
    /// kilobytes of traffic.
    ///
    /// Pinned to a tag rather than a latest release, so the member and its offsets cannot move
    /// underneath the assertions. Ignored by default, like the other tests here that go out to
    /// the network.
    ///
    /// Deliberately on the single-threaded runtime `#[tokio::test]` gives by default: the
    /// blocking side of [`HttpRangeSource`] hands its requests to the runtime and waits on a
    /// channel precisely so it works on either flavour, and this is what proves it.
    #[tokio::test]
    #[ignore = "reads part of a release asset over the network"]
    async fn a_member_is_read_out_of_a_real_release_asset() {
        const MEMBER: &str = "nvim-win64/share/nvim/runtime/indent/quarto.vim";

        let github = crate::GitHubClient::new(None).expect("a client");
        let repo = crate::Repository {
            owner: "neovim".to_owned(),
            name: "neovim".to_owned(),
        };

        let asset = github
            .asset_range_source(
                &repo,
                "nvim-win64.zip",
                "v0.10.0",
                None,
                super::WORTH_RANGING,
            )
            .await
            .expect("the asset resolves")
            .expect("release storage serves ranges");
        assert!(
            asset.size > super::WORTH_RANGING,
            "a {} byte asset is not worth ranging",
            asset.size
        );

        let (arrivals, mut arrived) = tokio::sync::mpsc::unbounded_channel();
        let source = github.range_source(asset.clone(), Some(arrivals));

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out");
        let target = dest.clone();
        let found = tokio::task::spawn_blocking(move || {
            extract_members(source, &target, &[MEMBER.to_owned()])
        })
        .await
        .unwrap()
        .expect("the ranges are served");

        assert!(found, "the archive holds the member");
        assert!(dest.join(MEMBER).is_file());

        let mut transferred = 0;
        while let Some(bytes) = arrived.recv().await {
            transferred += bytes;
        }
        // The index alone is a quarter of a megabyte here, and that is the bulk of it.
        assert!(
            transferred < asset.size / 10,
            "transferred {transferred} of {} bytes",
            asset.size
        );
    }

    #[test]
    fn content_ranges_are_accepted_only_when_they_are_the_bytes_asked_for() {
        assert!(range_matches("bytes 0-99/500", 0, 100, 500));
        assert!(range_matches("bytes 100-199/500", 100, 200, 500));
        // A different span, or a different object size than the index was read against.
        assert!(!range_matches("bytes 0-98/500", 0, 100, 500));
        assert!(!range_matches("bytes 1-99/500", 0, 100, 500));
        assert!(!range_matches("bytes 0-99/501", 0, 100, 500));
        // Not a byte range at all, or absent entirely.
        assert!(!range_matches("items 0-99/500", 0, 100, 500));
        assert!(!range_matches("", 0, 100, 500));
    }

    #[test]
    fn seeking_lands_where_a_file_would_and_reading_past_the_end_stops() {
        let source = Recording::new((0..=255u8).collect());
        let mut reader = RangeReader::new(&source);

        assert_eq!(reader.seek(SeekFrom::Start(10)).unwrap(), 10);
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], 10);

        assert_eq!(reader.seek(SeekFrom::End(-1)).unwrap(), 255);
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], 255);

        // At the end there is nothing left, and past it is not an error.
        assert_eq!(reader.read(&mut byte).unwrap(), 0);
        assert_eq!(reader.seek(SeekFrom::Current(100)).unwrap(), 356);
        assert_eq!(reader.read(&mut byte).unwrap(), 0);
        // Before the start is.
        assert!(reader.seek(SeekFrom::Start(0)).is_ok());
        assert!(reader.seek(SeekFrom::Current(-1)).is_err());
    }
}
