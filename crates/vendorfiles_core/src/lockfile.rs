//! `vendor-lock.json` - the record of what was actually written into a dependency folder.
//!
//! Field order (`repository`, `version`, `files`) and key order inside `files` are part of the
//! on-disk format, so every map here is an [`IndexMap`].

use std::path::Path;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::error::{Result, VendorError};
use crate::model::{FileEntry, FileTarget};
use crate::template::{basename, replace_version};

/// The `files` map of a single locked dependency: config input → what landed on disk.
pub type LockFiles = IndexMap<String, FileTarget>;

/// One dependency's entry in the lockfile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct VendorLock {
    pub repository: String,
    pub version: String,
    pub files: LockFiles,
}

/// The whole lockfile: dependency name → lock entry.
pub type Lockfile = IndexMap<String, VendorLock>;

/// Substitutes `{version}` in every string *value* of a target.
///
/// Keys are deliberately left alone: the reference's `replaceVersionInObject` only rewrites
/// values, which is why archive names keep their `{version}` placeholder in the lockfile.
fn replace_version_in_target(target: &FileTarget, version: &str) -> FileTarget {
    match target {
        FileTarget::Rename(s) => FileTarget::Rename(replace_version(s, version)),
        FileTarget::ExtractList(list) => {
            FileTarget::ExtractList(list.iter().map(|s| replace_version(s, version)).collect())
        }
        FileTarget::ExtractMap(map) => FileTarget::ExtractMap(
            map.iter()
                .map(|(k, v)| (k.clone(), replace_version(v, version)))
                .collect(),
        ),
    }
}

/// Projects a config `files` array into the lockfile's `files` map.
///
/// Later entries overwrite earlier ones by key while keeping the earlier key's position -
/// the semantics of repeated `Object.assign` in the reference.
#[must_use]
pub fn config_files_to_lock_files(files: &[FileEntry], version: &str) -> LockFiles {
    let mut out = LockFiles::new();
    for entry in files {
        match entry {
            FileEntry::Simple(input) => {
                out.insert(
                    input.clone(),
                    FileTarget::Rename(replace_version(&basename(input), version)),
                );
            }
            FileEntry::Mapped(map) => {
                for (input, target) in map {
                    out.insert(input.clone(), replace_version_in_target(target, version));
                }
            }
        }
    }
    out
}

/// Every path written into the dependency folder, relative to it.
#[must_use]
pub fn flat_files(files: &LockFiles) -> Vec<String> {
    files
        .values()
        .flat_map(|target| match target {
            FileTarget::Rename(s) => vec![s.clone()],
            FileTarget::ExtractList(list) => list.clone(),
            FileTarget::ExtractMap(map) => map.values().cloned().collect(),
        })
        .collect()
}

/// Reads and parses a lockfile.
///
/// # Errors
///
/// Returns [`VendorError::ReadFile`] if the file is missing, or [`VendorError::ParseConfig`]
/// if it is not a valid lockfile.
pub async fn read_lockfile(path: &Path) -> Result<Lockfile> {
    let data = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| VendorError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
    serde_json::from_str(&data).map_err(|e| VendorError::ParseConfig {
        path: path.display().to_string(),
        message: e.to_string(),
    })
}

/// Serialises a lockfile the way `JSON.stringify(value, null, 2)` does.
///
/// # Errors
///
/// Returns [`VendorError::SerializeConfig`] if the lockfile cannot be encoded.
pub fn to_json(lockfile: &Lockfile) -> Result<String> {
    serde_json::to_string_pretty(lockfile).map_err(|e| VendorError::SerializeConfig {
        path: "vendor-lock.json".to_owned(),
        message: e.to_string(),
    })
}

/// Upserts one dependency into the lockfile at `path`, creating it if needed.
///
/// Written with a trailing newline, matching the reference's install path.
///
/// # Errors
///
/// Returns [`VendorError::SerializeConfig`] or [`VendorError::WriteFile`].
pub async fn write_lockfile(name: &str, lock: VendorLock, path: &Path) -> Result<()> {
    let mut lockfile = read_lockfile(path).await.unwrap_or_default();
    lockfile.insert(name.to_owned(), lock);
    let data = format!("{}\n", to_json(&lockfile)?);
    write_string(path, &data).await
}

/// Writes raw lockfile contents, creating parent directories as needed.
///
/// # Errors
///
/// Returns [`VendorError::WriteFile`] if the file or its parent directories cannot be created.
pub async fn write_string(path: &Path, data: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| VendorError::WriteFile {
                path: parent.to_path_buf(),
                source,
            })?;
    }
    tokio::fs::write(path, data)
        .await
        .map_err(|source| VendorError::WriteFile {
            path: path.to_path_buf(),
            source,
        })
}

/// The files a dependency owns according to the lockfile, or an empty list if unreadable.
pub async fn files_from_lockfile(path: &Path, name: &str) -> Vec<String> {
    read_lockfile(path).await.map_or_else(
        |_| Vec::new(),
        |lockfile| {
            lockfile
                .get(name)
                .map(|entry| flat_files(&entry.files))
                .unwrap_or_default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{Lockfile, VendorLock, config_files_to_lock_files, flat_files, to_json};
    use crate::model::{FileEntry, FileTarget};

    fn files(json: &str) -> Vec<FileEntry> {
        serde_json::from_str(json).expect("valid files array")
    }

    #[test]
    fn simple_entries_map_to_their_basename() {
        let lock = config_files_to_lock_files(&files(r#"["dist/coloris.min.js"]"#), "v1.0.0");
        assert_eq!(
            lock["dist/coloris.min.js"],
            FileTarget::Rename("coloris.min.js".to_owned())
        );
    }

    #[test]
    fn later_entries_overwrite_but_keep_the_original_position() {
        let lock = config_files_to_lock_files(
            &files(r#"["LICENSE", {"{release}/a.zip": ["x"], "LICENSE": "LICENSE2"}]"#),
            "0.38.0",
        );
        let keys: Vec<_> = lock.keys().map(String::as_str).collect();
        assert_eq!(keys, ["LICENSE", "{release}/a.zip"]);
        assert_eq!(lock["LICENSE"], FileTarget::Rename("LICENSE2".to_owned()));
    }

    #[test]
    fn version_is_substituted_in_values_but_not_keys() {
        let lock = config_files_to_lock_files(
            &files(r#"[{"{release}/fzf-{version}.zip": {"fzf.exe": "fzf-{version}.exe"}}]"#),
            "v0.38.0",
        );
        let (key, value) = lock.first().unwrap();
        assert_eq!(key, "{release}/fzf-{version}.zip");
        let FileTarget::ExtractMap(map) = value else {
            panic!("expected an extract map");
        };
        assert_eq!(map["fzf.exe"], "fzf-0.38.0.exe");
    }

    #[test]
    fn flat_files_collects_every_output_path() {
        let lock = config_files_to_lock_files(
            &files(r#"["a/one", {"b.zip": ["x", "y"], "c.zip": {"i": "o"}}]"#),
            "",
        );
        assert_eq!(flat_files(&lock), ["one", "x", "y", "o"]);
    }

    #[test]
    fn serialisation_matches_json_stringify_with_two_spaces() {
        let mut lockfile = Lockfile::new();
        lockfile.insert(
            "fzf".to_owned(),
            VendorLock {
                repository: "https://github.com/junegunn/fzf".to_owned(),
                version: "0.38.0".to_owned(),
                files: config_files_to_lock_files(&files(r#"["LICENSE"]"#), "0.38.0"),
            },
        );
        assert_eq!(
            to_json(&lockfile).unwrap(),
            "{\n  \"fzf\": {\n    \"repository\": \"https://github.com/junegunn/fzf\",\n    \
             \"version\": \"0.38.0\",\n    \"files\": {\n      \"LICENSE\": \"LICENSE\"\n    \
             }\n  }\n}"
        );
    }

    #[test]
    fn map_equality_ignores_key_order_but_arrays_stay_ordered() {
        let a = config_files_to_lock_files(&files(r#"[{"x": "1", "y": "2"}]"#), "");
        let b = config_files_to_lock_files(&files(r#"[{"y": "2", "x": "1"}]"#), "");
        assert_eq!(a, b);

        let c = config_files_to_lock_files(&files(r#"[{"z.zip": ["a", "b"]}]"#), "");
        let d = config_files_to_lock_files(&files(r#"[{"z.zip": ["b", "a"]}]"#), "");
        assert_ne!(c, d);
    }
}
