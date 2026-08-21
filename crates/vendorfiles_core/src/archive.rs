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

    let name = archive.file_name().map_or_else(
        || "archive".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let stem = name.strip_suffix(".gz").unwrap_or(name.as_str());
    let mut out = File::create(dest.join(stem))?;
    out.write_all(&head)?;
    std::io::copy(&mut decoder, &mut out)?;
    Ok(())
}

/// An xz stream is a `.tar.xz` when it decompresses to a tar; otherwise it is a lone file.
///
/// Decompressed whole rather than streamed: `lzma-rs` decodes into a writer, and an archive is
/// already read into memory to be sniffed.
fn unxz(archive: &Path, file: File, dest: &Path) -> Result<()> {
    let mut decoded = Vec::new();
    lzma_rs::xz_decompress(&mut BufReader::new(file), &mut decoded)
        .map_err(|source| VendorError::Http(source.to_string()))?;

    if is_tar(&decoded) {
        tar::Archive::new(std::io::Cursor::new(decoded)).unpack(dest)?;
        return Ok(());
    }

    let name = archive.file_name().map_or_else(
        || "archive".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    let stem = name.strip_suffix(".xz").unwrap_or(name.as_str());
    std::fs::write(dest.join(stem), decoded)?;
    Ok(())
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
