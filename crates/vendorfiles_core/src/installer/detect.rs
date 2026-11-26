//! Telling installer technologies apart by their bytes.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use super::InstallerKind;
use crate::error::Result;

/// The OLE compound file signature every MSI starts with.
const OLE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The manifests that mark a zip as an MSIX package or bundle.
const MSIX_MANIFESTS: [&str; 2] = ["AppxManifest.xml", "AppxMetadata/AppxBundleManifest.xml"];

/// Inno writes its setup data header after the loader, and NSIS signs its first header with
/// `0xDEADBEEF` followed by `NullsoftInst`.
static EXE_MARKERS: LazyLock<regex::bytes::Regex> = LazyLock::new(|| {
    regex::bytes::Regex::new(r"(?-u)(Inno Setup Setup Data)|(\xEF\xBE\xAD\xDENullsoftInst)")
        .expect("valid pattern")
});

/// The installer technology of the file at `path`, or `None` for anything else.
///
/// # Errors
///
/// Returns an I/O error when the file cannot be read.
pub fn detect(path: &Path) -> Result<Option<InstallerKind>> {
    let mut head = [0u8; 8];
    let read = File::open(path)?.read(&mut head)?;
    let head = &head[..read];
    if head.starts_with(&OLE) {
        return Ok(Some(InstallerKind::Msi));
    }
    if head.starts_with(b"PK\x03\x04") {
        return Ok(is_msix(path).then_some(InstallerKind::Msix));
    }
    if head.starts_with(b"MZ") {
        let bytes = std::fs::read(path)?;
        return Ok(EXE_MARKERS.captures(&bytes).map(|found| {
            if found.get(1).is_some() {
                InstallerKind::Inno
            } else {
                InstallerKind::Nsis
            }
        }));
    }
    Ok(None)
}

fn is_msix(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    archive
        .file_names()
        .any(|name| name.is_ok_and(|name| MSIX_MANIFESTS.contains(&name.as_ref())))
}

/// The identity name an MSIX package or bundle declares, like `28488.ImageGlass`.
///
/// This is what `Get-AppxPackage -Name` matches, so it is what tracks the install.
#[must_use]
pub fn msix_identity(path: &Path) -> Option<String> {
    static IDENTITY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"<Identity\b[^>]*?\sName="([^"]+)""#).expect("valid pattern")
    });
    let mut archive = zip::ZipArchive::new(File::open(path).ok()?).ok()?;
    for manifest in MSIX_MANIFESTS {
        let Ok(mut entry) = archive.by_name(manifest) else {
            continue;
        };
        let mut text = String::new();
        entry.read_to_string(&mut text).ok()?;
        return IDENTITY.captures(&text).map(|found| found[1].to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{InstallerKind, detect, msix_identity};

    fn file(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file
    }

    fn zip(members: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in members {
            writer
                .start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(text.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn each_technology_is_told_apart_by_its_bytes() {
        let detected = |bytes: &[u8]| detect(file(bytes).path()).unwrap();
        assert_eq!(
            detected(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0]),
            Some(InstallerKind::Msi)
        );
        assert_eq!(
            detected(b"MZ\x90\0 loader ... Inno Setup Setup Data (6.7.0)"),
            Some(InstallerKind::Inno)
        );
        assert_eq!(
            detected(b"MZ\x90\0 stub \xEF\xBE\xAD\xDENullsoftInst"),
            Some(InstallerKind::Nsis)
        );
        assert_eq!(detected(b"MZ\x90\0 just a program"), None);
        assert_eq!(detected(b"#!/bin/sh"), None);
        assert_eq!(detected(b""), None);
    }

    #[test]
    fn a_zip_is_an_msix_only_with_its_manifest() {
        let manifest = r#"<Package><Identity Name="28488.ImageGlass" Version="1.0.0.0" Publisher="CN=x"/></Package>"#;
        let package = file(&zip(&[("AppxManifest.xml", manifest)]));
        assert_eq!(detect(package.path()).unwrap(), Some(InstallerKind::Msix));
        assert_eq!(
            msix_identity(package.path()).as_deref(),
            Some("28488.ImageGlass")
        );

        let plain = file(&zip(&[("readme.txt", "hi")]));
        assert_eq!(detect(plain.path()).unwrap(), None);
        assert_eq!(msix_identity(plain.path()), None);
    }
}
