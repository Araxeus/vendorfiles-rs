//! Archive extraction with magic-byte sniffing.
//!
//! Mirrors the `unarchive` package the reference depends on: the *content* decides the format,
//! not the file name, and Chrome/Firefox extension containers are unwrapped as ZIPs.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{Result, VendorError};

/// Archive containers this tool understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    /// A Chrome extension: a signed header followed by a ZIP.
    Crx,
    /// Gzip: a `.tar.gz` if the decompressed stream is a tar, otherwise a single file.
    Gzip,
    /// Xz: likewise a `.tar.xz`, or a single compressed file.
    Xz,
    Tar,
}

const TAR_MAGIC_OFFSET: usize = 257;
const TAR_HEADER_LEN: usize = 512;

/// Identifies an archive from its leading bytes.
#[must_use]
pub fn sniff(header: &[u8]) -> Option<ArchiveKind> {
    if header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06") {
        return Some(ArchiveKind::Zip);
    }
    if header.starts_with(b"Cr24") {
        return Some(ArchiveKind::Crx);
    }
    if header.starts_with(&[0x1f, 0x8b]) {
        return Some(ArchiveKind::Gzip);
    }
    if header.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
        return Some(ArchiveKind::Xz);
    }
    if is_tar(header) {
        return Some(ArchiveKind::Tar);
    }
    None
}

fn is_tar(header: &[u8]) -> bool {
    header.len() >= TAR_MAGIC_OFFSET + 5
        && &header[TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + 5] == b"ustar"
}

/// Extracts `archive` into `dest`, creating it if needed.
///
/// Blocking; call from [`tokio::task::spawn_blocking`].
///
/// # Errors
///
/// Returns an error if the container cannot be read or is not a supported archive; callers
/// map that to [`VendorError::CannotExtract`].
pub fn extract(archive: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;

    let mut file = File::open(archive)?;
    let mut header = vec![0u8; TAR_HEADER_LEN];
    let read = read_up_to(&mut file, &mut header)?;
    header.truncate(read);
    file.seek(SeekFrom::Start(0))?;

    match sniff(&header) {
        Some(ArchiveKind::Zip) | None => unzip(file, dest),
        Some(ArchiveKind::Crx) => unzip_crx(archive, dest),
        Some(ArchiveKind::Tar) => untar(file, dest),
        Some(ArchiveKind::Gzip) => ungzip(archive, file, dest),
        Some(ArchiveKind::Xz) => unxz(archive, file, dest),
    }
}

fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// The name a lone compressed file extracts to: its own, minus the compression suffix.
///
/// Shared by extraction and listing so the two cannot disagree about what a `.gz` or `.xz`
/// that is not a tar actually produces.
fn lone_file_name(archive: &Path, suffix: &str) -> String {
    let name = archive.file_name().map_or_else(
        || "archive".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    name.strip_suffix(suffix).unwrap_or(&name).to_owned()
}

fn unzip(file: File, dest: &Path) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(BufReader::new(file)).map_err(|e| VendorError::Http(e.to_string()))?;
    archive
        .extract(dest)
        .map_err(|e| VendorError::Http(e.to_string()))
}

fn untar(file: File, dest: &Path) -> Result<()> {
    tar::Archive::new(BufReader::new(file)).unpack(dest)?;
    Ok(())
}

/// A gzip stream is a `.tar.gz` when it decompresses to a tar; otherwise it is a lone file.
fn ungzip(archive: &Path, file: File, dest: &Path) -> Result<()> {
    let mut decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    let mut head = vec![0u8; TAR_HEADER_LEN];
    let read = read_up_to(&mut decoder, &mut head)?;
    head.truncate(read);

    if is_tar(&head) {
        // Restart from the beginning: the tar reader needs the header bytes back.
        let file = File::open(archive)?;
        let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
        tar::Archive::new(decoder).unpack(dest)?;
        return Ok(());
    }

    let mut out = File::create(dest.join(lone_file_name(archive, ".gz")))?;
    out.write_all(&head)?;
    std::io::copy(&mut decoder, &mut out)?;
    Ok(())
}

/// An xz stream is a `.tar.xz` when it decompresses to a tar; otherwise it is a lone file.
///
/// `lzma-rs` decodes into a writer rather than offering a reader, so unlike the gzip path this
/// cannot be a pure stream. It decodes to a **temporary file** instead of a buffer: compression
/// ratios are unbounded, so a modest asset can hold a payload far larger than memory.
fn unxz(archive: &Path, file: File, dest: &Path) -> Result<()> {
    let mut decoded = tempfile::Builder::new()
        .prefix("vendorfiles-xz-")
        .tempfile()?;
    {
        let mut writer = std::io::BufWriter::new(decoded.as_file_mut());
        lzma_rs::xz_decompress(&mut BufReader::new(file), &mut writer)
            .map_err(|source| VendorError::Http(source.to_string()))?;
        writer.flush()?;
    }

    if decompresses_to_a_tar(&mut decoded.reopen()?)? {
        tar::Archive::new(BufReader::new(decoded.reopen()?)).unpack(dest)?;
        return Ok(());
    }

    let mut out = File::create(dest.join(lone_file_name(archive, ".xz")))?;
    std::io::copy(&mut decoded.reopen()?, &mut out)?;
    Ok(())
}

/// The *file* paths an archive contains, without writing any of them out.
///
/// Listing rather than extracting: checking that an archive holds a named file never costs the
/// disk space of everything else in it, whatever the payload decompresses to.
///
/// Directory entries are left out. A member is something installation *moves into place*, and
/// [`crate::ops`] copies it when a rename cannot cross filesystems — which fails on a directory.
/// Listing `tool/bin/` would let a `member` of `tool/bin` look present when installing it could
/// not work.
///
/// # Errors
///
/// Returns an error if the container cannot be read or is not a supported archive.
pub fn members(archive: &Path) -> Result<Vec<String>> {
    let mut file = File::open(archive)?;
    let mut header = vec![0u8; TAR_HEADER_LEN];
    let read = read_up_to(&mut file, &mut header)?;
    header.truncate(read);
    file.seek(SeekFrom::Start(0))?;

    match sniff(&header) {
        Some(ArchiveKind::Zip) | None => zip_names(BufReader::new(file)),
        Some(ArchiveKind::Crx) => {
            let bytes = std::fs::read(archive)?;
            let start = crx_payload_offset(&bytes)
                .ok_or_else(|| VendorError::Http("unsupported CRX header".to_owned()))?;
            zip_names(std::io::Cursor::new(&bytes[start..]))
        }
        Some(ArchiveKind::Tar) => tar_names(BufReader::new(file)),
        Some(ArchiveKind::Gzip) => {
            let mut decoder = flate2::read::GzDecoder::new(BufReader::new(file));
            if decompresses_to_a_tar(&mut decoder)? {
                // Restart from the beginning: the tar reader needs the header bytes back.
                let file = File::open(archive)?;
                tar_names(flate2::read::GzDecoder::new(BufReader::new(file)))
            } else {
                // A lone compressed file, which extraction writes out under this one name. None of
                // the payload is needed to say that, but read it through anyway, as the xz branch
                // does: a stream that stops decoding — or whose checksum does not match, which
                // only shows up at the very end — should be an error here rather than a surprise
                // at install time.
                std::io::copy(&mut decoder, &mut std::io::sink())?;
                Ok(vec![lone_file_name(archive, ".gz")])
            }
        }
        Some(ArchiveKind::Xz) => xz_names(archive, file),
    }
}

/// The names inside an xz payload, without ever storing the payload.
///
/// `lzma-rs` decodes into a writer rather than offering a reader, which is why [`unxz`] decodes to
/// a temporary file. Listing cannot afford that: it exists to avoid paying for an archive's
/// contents. So a thread pushes the decompressed stream down a pipe and the tar reader pulls it
/// out here — the bytes pass through memory a pipe buffer at a time and are never kept.
fn xz_names(archive: &Path, file: File) -> Result<Vec<String>> {
    let (mut reader, writer) = std::io::pipe()?;
    let decoding = std::thread::spawn(move || -> Result<()> {
        let mut writer = std::io::BufWriter::new(writer);
        lzma_rs::xz_decompress(&mut BufReader::new(file), &mut writer)
            .map_err(|source| VendorError::Http(source.to_string()))?;
        writer.flush()?;
        Ok(())
    });

    let mut head = vec![0u8; TAR_HEADER_LEN];
    let read = read_up_to(&mut reader, &mut head)?;
    head.truncate(read);

    if !is_tar(&head) {
        // A lone compressed file: its name comes from the archive's own, so none of the payload is
        // needed. Drain it anyway — a stream that does not decode should be an error here rather
        // than a surprise at install time — which costs time but still no disk.
        std::io::copy(&mut reader, &mut std::io::sink())?;
        finish_decoding(decoding)?;
        return Ok(vec![lone_file_name(archive, ".xz")]);
    }

    // The header is already out of the pipe and the tar reader needs it back.
    let mut tarball = tar::Archive::new(std::io::Cursor::new(head).chain(reader));
    let names = names_in(&mut tarball)?;
    // A tar reader stops at the end-of-archive marker, but archives are padded well past it and
    // the decoding thread is still pushing those blocks. Read them out so it can finish: closing
    // the pipe under it would turn a sound archive into a broken-pipe error.
    std::io::copy(&mut tarball.into_inner(), &mut std::io::sink())?;
    // Only now is the decode's own verdict worth having. The whole stream was pulled through, so a
    // failure here is the archive's rather than this reader having stopped early. A listing that
    // failed instead reports its own error, which is what a half-decoded stream looks like from
    // the tar side.
    finish_decoding(decoding)?;
    Ok(names)
}

/// The decoding thread's own result, once the stream has been read to the end.
fn finish_decoding(decoding: std::thread::JoinHandle<Result<()>>) -> Result<()> {
    decoding
        .join()
        .map_err(|_| VendorError::Http("xz decoding panicked".to_owned()))?
}

/// Whether a decompressed stream is a tar, read from its leading bytes.
///
/// Consumes the header it inspects, so callers that go on to read the tar restart the stream.
fn decompresses_to_a_tar(reader: &mut impl Read) -> Result<bool> {
    let mut head = vec![0u8; TAR_HEADER_LEN];
    let read = read_up_to(reader, &mut head)?;
    head.truncate(read);
    Ok(is_tar(&head))
}

fn zip_names(reader: impl Read + Seek) -> Result<Vec<String>> {
    let archive = zip::ZipArchive::new(reader).map_err(|e| VendorError::Http(e.to_string()))?;
    Ok(archive
        .file_names()
        // What `ZipFile::is_dir` itself tests, without paying to parse every local header: a ZIP
        // marks a directory by the trailing separator on its name.
        .filter(|name| !name.ends_with(['/', '\\']))
        .map(str::to_owned)
        .collect())
}

fn tar_names(reader: impl Read) -> Result<Vec<String>> {
    names_in(&mut tar::Archive::new(reader))
}

fn names_in(archive: &mut tar::Archive<impl Read>) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in archive.entries()? {
        let entry = entry?;
        // Tar says so in the header's type flag rather than in the name.
        if entry.header().entry_type().is_dir() {
            continue;
        }
        names.push(entry.path()?.to_string_lossy().into_owned());
    }
    Ok(names)
}

/// Unwraps a CRX container and extracts the ZIP inside it.
fn unzip_crx(archive: &Path, dest: &Path) -> Result<()> {
    let bytes = std::fs::read(archive)?;
    let zip_start = crx_payload_offset(&bytes)
        .ok_or_else(|| VendorError::Http("unsupported CRX header".to_owned()))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes[zip_start..]))
        .map_err(|e| VendorError::Http(e.to_string()))?;
    zip.extract(dest)
        .map_err(|e| VendorError::Http(e.to_string()))
}

/// Byte offset of the ZIP payload inside a CRX file, for both CRX2 and CRX3 layouts.
#[must_use]
pub fn crx_payload_offset(bytes: &[u8]) -> Option<usize> {
    let le = |at: usize| -> Option<usize> {
        let slice: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(slice)).ok()
    };
    if !bytes.starts_with(b"Cr24") {
        return None;
    }
    match le(4)? {
        2 => Some(16 + le(8)? + le(12)?),
        3 => Some(12 + le(8)?),
        _ => None,
    }
    .filter(|offset| *offset <= bytes.len())
}

#[cfg(test)]
mod tests {
    use super::{ArchiveKind, TAR_MAGIC_OFFSET, crx_payload_offset, extract, sniff};
    use std::io::Write;

    #[test]
    fn sniffing_recognises_the_supported_containers() {
        assert_eq!(sniff(b"PK\x03\x04rest"), Some(ArchiveKind::Zip));
        assert_eq!(sniff(b"Cr24\x02\0\0\0"), Some(ArchiveKind::Crx));
        assert_eq!(sniff(&[0x1f, 0x8b, 0x08]), Some(ArchiveKind::Gzip));
        assert_eq!(sniff(b"not an archive"), None);

        let mut tar = vec![0u8; 512];
        tar[TAR_MAGIC_OFFSET..TAR_MAGIC_OFFSET + 5].copy_from_slice(b"ustar");
        assert_eq!(sniff(&tar), Some(ArchiveKind::Tar));
    }

    #[test]
    fn crx_offsets_follow_the_two_container_versions() {
        let mut crx2 = b"Cr24".to_vec();
        crx2.extend_from_slice(&2u32.to_le_bytes());
        crx2.extend_from_slice(&3u32.to_le_bytes()); // public key length
        crx2.extend_from_slice(&5u32.to_le_bytes()); // signature length
        crx2.extend_from_slice(&[0u8; 8]);
        assert_eq!(crx_payload_offset(&crx2), Some(24));

        let mut crx3 = b"Cr24".to_vec();
        crx3.extend_from_slice(&3u32.to_le_bytes());
        crx3.extend_from_slice(&4u32.to_le_bytes()); // header length
        crx3.extend_from_slice(&[0u8; 8]);
        assert_eq!(crx_payload_offset(&crx3), Some(16));
    }

    #[test]
    fn zip_archives_round_trip_through_extract() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("a.zip");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file::<_, ()>("nested/hello.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"hi").unwrap();
            writer.finish().unwrap();
        }
        let out = dir.path().join("out");
        extract(&archive_path, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("nested/hello.txt")).unwrap(),
            "hi"
        );
    }

    #[test]
    fn tar_gz_archives_round_trip_through_extract() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("a.tar.gz");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, "fzf", &b"hi"[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.path().join("out");
        extract(&archive_path, &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("fzf")).unwrap(), "hi");
    }

    #[test]
    fn tar_xz_archives_round_trip_through_extract() {
        // `sinelaw/fresh` and others publish `.tar.xz`, which used to be unopenable.
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("a.tar.xz");
        let mut tarred = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tarred);
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "fresh", &b"hi"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(&tarred), &mut compressed).unwrap();
        std::fs::write(&archive_path, &compressed).unwrap();

        // Sniffed from its magic bytes, like every other container.
        assert_eq!(sniff(&compressed), Some(ArchiveKind::Xz));
        let out = dir.path().join("out");
        extract(&archive_path, &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("fresh")).unwrap(), "hi");
    }

    #[test]
    fn plain_xz_becomes_a_single_file_named_without_the_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("notes.txt.xz");
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(b"plain content"), &mut compressed).unwrap();
        std::fs::write(&archive_path, compressed).unwrap();

        let out = dir.path().join("out");
        extract(&archive_path, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("notes.txt")).unwrap(),
            "plain content"
        );
    }

    #[test]
    fn members_are_listed_without_extracting_anything() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();

        // A tar.gz, as most Unix releases ship.
        let tgz = dir.path().join("a.tar.gz");
        {
            let file = std::fs::File::create(&tgz).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            for name in ["tool-1.0/LICENSE", "tool-1.0/tool"] {
                let mut header = tar::Header::new_gnu();
                header.set_size(2);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, name, &b"hi"[..]).unwrap();
            }
            builder.into_inner().unwrap().finish().unwrap();
        }
        let mut listed = super::members(&tgz).unwrap();
        listed.sort();
        assert_eq!(listed, ["tool-1.0/LICENSE", "tool-1.0/tool"]);
        // Nothing was written out: only the archive is in the directory.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

        // A zip, as most Windows releases ship.
        let zipped = dir.path().join("b.zip");
        {
            let file = std::fs::File::create(&zipped).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .start_file::<_, ()>("tool.exe", zip::write::FileOptions::default())
                .unwrap();
            writer.write_all(b"hi").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(super::members(&zipped).unwrap(), ["tool.exe"]);

        // A tar.xz, which has to be decompressed before it can be listed.
        let txz = dir.path().join("c.tar.xz");
        let mut tarred = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tarred);
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "nested/fresh", &b"hi"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(&tarred), &mut compressed).unwrap();
        std::fs::write(&txz, compressed).unwrap();
        assert_eq!(super::members(&txz).unwrap(), ["nested/fresh"]);
    }

    /// A ZIP holding one entry, as the payload of an extension container.
    fn zip_bytes(name: &str) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file::<_, ()>(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"hi").unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn crx_containers_are_listed_through_their_wrapped_zip() {
        let dir = tempfile::tempdir().unwrap();
        let payload = zip_bytes("manifest.json");

        // CRX2: the public key and signature sit between the header and the ZIP.
        let mut crx2 = b"Cr24".to_vec();
        crx2.extend_from_slice(&2u32.to_le_bytes());
        crx2.extend_from_slice(&3u32.to_le_bytes()); // public key length
        crx2.extend_from_slice(&5u32.to_le_bytes()); // signature length
        crx2.extend_from_slice(&[0u8; 8]); // the key and signature themselves
        crx2.extend_from_slice(&payload);
        let crx2_path = dir.path().join("ext2.crx");
        std::fs::write(&crx2_path, &crx2).unwrap();
        assert_eq!(sniff(&crx2), Some(ArchiveKind::Crx));
        assert_eq!(super::members(&crx2_path).unwrap(), ["manifest.json"]);

        // CRX3: one length covers the whole protobuf header.
        let mut crx3 = b"Cr24".to_vec();
        crx3.extend_from_slice(&3u32.to_le_bytes());
        crx3.extend_from_slice(&4u32.to_le_bytes()); // header length
        crx3.extend_from_slice(&[0u8; 4]);
        crx3.extend_from_slice(&payload);
        let crx3_path = dir.path().join("ext3.crx");
        std::fs::write(&crx3_path, &crx3).unwrap();
        assert_eq!(super::members(&crx3_path).unwrap(), ["manifest.json"]);

        // Nothing was written out: only the two containers are in the directory.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    /// A tar of `count` files of `size` bytes each, xz-compressed.
    fn tar_xz(count: usize, size: usize) -> Vec<u8> {
        let mut tarred = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tarred);
            for index in 0..count {
                let body = vec![b'a'; size];
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, format!("big/file-{index}"), &body[..])
                    .unwrap();
            }
            builder.finish().unwrap();
        }
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(&tarred), &mut compressed).unwrap();
        compressed
    }

    #[test]
    fn a_tar_xz_far_larger_than_a_pipe_buffer_is_listed_without_stalling() {
        // The payload passes from the decoding thread to the tar reader a pipe buffer at a time —
        // tens of kilobytes — so a payload thousands of times that size proves the two really do
        // hand off rather than one waiting for the other to finish.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.tar.xz");
        std::fs::write(&path, tar_xz(4, 2 * 1024 * 1024)).unwrap();

        let listed = super::members(&path).unwrap();
        assert_eq!(
            listed,
            ["big/file-0", "big/file-1", "big/file-2", "big/file-3"]
        );
        // 8 MiB decompressed, and not a byte of it written anywhere.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_truncated_xz_is_an_error_rather_than_a_made_up_name() {
        // Listing derives a lone file's name from the archive's own, so a stream that stops
        // decoding could quietly look like one. It has to stay an error, as it is for extraction.
        let dir = tempfile::tempdir().unwrap();

        // Truncated before anything decodes at all.
        let mut nothing = Vec::new();
        lzma_rs::xz_compress(
            &mut std::io::Cursor::new(&vec![b'z'; 400_000]),
            &mut nothing,
        )
        .unwrap();
        nothing.truncate(nothing.len() / 2);
        let nothing_path = dir.path().join("notes.txt.xz");
        std::fs::write(&nothing_path, nothing).unwrap();
        assert!(super::members(&nothing_path).is_err());

        // Truncated after a tar header has come through, so the listing starts and then runs out.
        let mut partial = tar_xz(4, 512 * 1024);
        partial.truncate(partial.len() * 2 / 3);
        let partial_path = dir.path().join("big.tar.xz");
        std::fs::write(&partial_path, partial).unwrap();
        assert!(super::members(&partial_path).is_err());
    }

    #[test]
    fn a_tar_xz_padded_past_its_end_marker_still_lists() {
        // What GNU tar really ships: zero blocks after the end-of-archive marker. A tar reader
        // stops at the marker and never reads them, so the decoder feeding it is still mid-write —
        // and a listing that walked away at that point would report a broken pipe on a sound
        // archive. Every padding here is larger than a pipe buffer.
        let dir = tempfile::tempdir().unwrap();
        for pad in [0usize, 8 * 1024, 64 * 1024, 1024 * 1024] {
            let mut tarred = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut tarred);
                let mut header = tar::Header::new_gnu();
                header.set_size(2);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, "tool", &b"hi"[..])
                    .unwrap();
                builder.finish().unwrap();
            }
            tarred.extend(std::iter::repeat_n(0u8, pad));
            let mut compressed = Vec::new();
            lzma_rs::xz_compress(&mut std::io::Cursor::new(&tarred), &mut compressed).unwrap();
            let path = dir.path().join(format!("pad-{pad}.tar.xz"));
            std::fs::write(&path, compressed).unwrap();
            assert_eq!(
                super::members(&path).unwrap(),
                ["tool"],
                "padded with {pad} trailing bytes"
            );
        }
    }

    #[test]
    fn a_damaged_lone_compressed_file_is_an_error_rather_than_a_made_up_name() {
        // Both branches derive a lone file's name from the archive's own, so a payload that does
        // not decode could quietly look like one. It has to stay an error, as it is for extraction.
        let dir = tempfile::tempdir().unwrap();
        let mut whole = Vec::new();
        {
            let mut encoder =
                flate2::write::GzEncoder::new(&mut whole, flate2::Compression::fast());
            encoder.write_all(&vec![b'q'; 400_000]).unwrap();
            encoder.finish().unwrap();
        }

        // Truncated well past the 512 bytes the tar sniff reads.
        let mut truncated = whole.clone();
        truncated.truncate(truncated.len() / 2);
        let truncated_path = dir.path().join("notes.txt.gz");
        std::fs::write(&truncated_path, truncated).unwrap();
        assert!(super::members(&truncated_path).is_err());
        assert!(extract(&truncated_path, &dir.path().join("out-truncated")).is_err());

        // Intact deflate data with a damaged trailer: only reading to the very end catches this.
        let mut checksum = whole;
        let trailer = checksum.len() - 5;
        checksum[trailer] ^= 0xff;
        let checksum_path = dir.path().join("checksum.txt.gz");
        std::fs::write(&checksum_path, checksum).unwrap();
        assert!(super::members(&checksum_path).is_err());
        assert!(extract(&checksum_path, &dir.path().join("out-checksum")).is_err());
    }

    #[test]
    fn directory_entries_are_left_out_of_the_listing() {
        // A `member` naming a directory cannot install — the move falls back to a file copy when
        // rename cannot cross filesystems — so listing one would let a wrong `member` look right.
        let dir = tempfile::tempdir().unwrap();

        let zipped = dir.path().join("with-dirs.zip");
        {
            let file = std::fs::File::create(&zipped).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            writer
                .add_directory::<_, ()>("tool/bin", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer
                .start_file::<_, ()>("tool/bin/tool", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"hi").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(super::members(&zipped).unwrap(), ["tool/bin/tool"]);

        let tarred = dir.path().join("with-dirs.tar");
        {
            let file = std::fs::File::create(&tarred).unwrap();
            let mut builder = tar::Builder::new(file);
            let mut directory = tar::Header::new_gnu();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_size(0);
            directory.set_mode(0o755);
            directory.set_cksum();
            builder
                .append_data(&mut directory, "tool/bin", std::io::empty())
                .unwrap();
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "tool/bin/tool", &b"hi"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        assert_eq!(super::members(&tarred).unwrap(), ["tool/bin/tool"]);
    }

    #[test]
    fn lone_compressed_files_are_listed_under_the_name_they_extract_to() {
        // The registry lets a `member` name a `.gz` or `.xz` that is not a tar, in which case
        // extraction writes exactly one file — so listing has to report that one name, not fail.
        let dir = tempfile::tempdir().unwrap();

        let gzipped = dir.path().join("yamlfmt.gz");
        {
            let file = std::fs::File::create(&gzipped).unwrap();
            let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            encoder.write_all(b"not a tar").unwrap();
            encoder.finish().unwrap();
        }
        assert_eq!(super::members(&gzipped).unwrap(), ["yamlfmt"]);

        let xzed = dir.path().join("yamlfmt.xz");
        let mut compressed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::Cursor::new(b"not a tar"), &mut compressed).unwrap();
        std::fs::write(&xzed, compressed).unwrap();
        assert_eq!(super::members(&xzed).unwrap(), ["yamlfmt"]);

        // And that is the name extraction really produces.
        let out = dir.path().join("out");
        extract(&gzipped, &out).unwrap();
        extract(&xzed, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("yamlfmt")).unwrap(),
            "not a tar"
        );
    }

    #[test]
    fn plain_gzip_becomes_a_single_file_named_without_the_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("notes.txt.gz");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            encoder.write_all(b"plain content").unwrap();
            encoder.finish().unwrap();
        }
        let out = dir.path().join("out");
        extract(&archive_path, &out).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("notes.txt")).unwrap(),
            "plain content"
        );
    }

    #[test]
    fn unrecognised_content_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.bin");
        std::fs::write(&path, b"definitely not an archive").unwrap();
        assert!(extract(&path, &dir.path().join("out")).is_err());
    }
}
