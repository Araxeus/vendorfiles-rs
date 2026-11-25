//! The data model shared by the config file, the lockfile and the install pipeline.
//!
//! Config files are user-authored documents that the reference tool validates late and
//! leniently: a key of the wrong type is ignored until the command that needs it complains.
//! That is reproduced here by deserialising every field through [`de_lenient`], which turns a
//! type mismatch into `None` instead of a hard parse failure, and by keeping validation in
//! [`RawDependency::validate`] where the reference puts it.

use std::borrow::Cow;

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Result, VendorError};
use crate::installer::{InstallerKey, InstallerOptions};
use crate::lockfile::{config_files_to_lock_files, flat_files};
use crate::template::{
    basename, browsable_url, expand_url, is_github_url, is_http_url, owner_and_name_from_repo_url,
    uses_version,
};
use crate::url_source::page::VersionSelector;
use crate::url_source::{UrlSource, is_git_url};

/// `owner`/`name` pair extracted from a GitHub repository URL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Repository {
    pub owner: String,
    pub name: String,
}

impl std::fmt::Display for Repository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Global `vendorConfig` block.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VendorConfig {
    #[serde(rename = "vendorFolder")]
    pub vendor_folder: String,
    /// Folder holding the one `vendor-lock.json` shared by every dependency (`true` is `"."`).
    #[serde(rename = "singleLockfile", default)]
    pub single_lockfile: Option<String>,
}

impl Default for VendorConfig {
    fn default() -> Self {
        Self {
            vendor_folder: "./vendor".to_owned(),
            single_lockfile: None,
        }
    }
}

/// Where a downloaded input lands.
///
/// A plain string renames the file; a list or map means the input is an archive and the
/// entries name paths *inside* it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FileTarget {
    /// `"LICENSE": "COPYING"` - save the input under a different name.
    Rename(String),
    /// `"archive.zip": ["a", "b"]` - extract these entries, keeping their names.
    ExtractList(Vec<String>),
    /// `"archive.zip": { "a": "b" }` - extract these entries under new names.
    ExtractMap(IndexMap<String, String>),
}

impl FileTarget {
    /// The archive members this target selects, as input→output pairs.
    ///
    /// Returns `None` for [`FileTarget::Rename`], which is not an extraction.
    #[must_use]
    pub fn extraction_pairs(&self) -> Option<Vec<(String, String)>> {
        match self {
            Self::Rename(_) => None,
            Self::ExtractList(list) => Some(list.iter().map(|e| (e.clone(), e.clone())).collect()),
            Self::ExtractMap(map) => {
                Some(map.iter().map(|(i, o)| (i.clone(), o.clone())).collect())
            }
        }
    }
}

/// One element of a dependency's `files` array.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FileEntry {
    /// `"dist/coloris.min.js"` - save under its basename.
    Simple(String),
    /// `{ "dist/coloris.min.js": "coloris.js" }` - one or more explicit input→output pairs.
    Mapped(IndexMap<String, FileTarget>),
}

/// A single input→output pair, after flattening the `files` array.
///
/// Mirrors the reference implementation's `flatFilesArray`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSpec {
    pub input: String,
    pub output: FileTarget,
}

/// Flattens a `files` array into ordered input→output pairs.
#[must_use]
pub fn flatten_files(files: &[FileEntry]) -> Vec<FileSpec> {
    let mut out = Vec::with_capacity(files.len());
    for entry in files {
        match entry {
            FileEntry::Simple(input) => out.push(FileSpec {
                output: FileTarget::Rename(basename(input)),
                input: input.clone(),
            }),
            FileEntry::Mapped(map) => out.extend(map.iter().map(|(input, output)| FileSpec {
                input: input.clone(),
                output: output.clone(),
            })),
        }
    }
    out
}

/// Whether an output name or archive member holds `{version}` or `{tag}`.
fn target_uses_version(target: &FileTarget) -> bool {
    match target {
        FileTarget::Rename(output) => uses_version(output),
        FileTarget::ExtractList(members) => members.iter().any(|m| uses_version(m)),
        FileTarget::ExtractMap(members) => members
            .iter()
            .any(|(member, output)| uses_version(member) || uses_version(output)),
    }
}

/// `hashVersionFile`: either `true` (use the first declared file) or an explicit path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum HashVersionFile {
    Flag(bool),
    Path(String),
}

/// Deserialises a field, treating a type mismatch as "absent".
///
/// The input is always a `serde_json::Value` tree (every config format is normalised to one
/// at load time), so round-tripping through `Value` here is exact, not lossy.
fn de_lenient<'de, D, T>(deserializer: D) -> std::result::Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    Ok(serde_json::from_value(value).ok())
}

/// Like [`de_lenient`], but accepts a bare string or mapping as a single-element array.
fn de_files<'de, D>(deserializer: D) -> std::result::Result<Option<Vec<FileEntry>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(FileEntry),
        Many(Vec<FileEntry>),
    }

    let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
        return Ok(None);
    };
    Ok(match serde_json::from_value::<OneOrMany>(value).ok() {
        None => None,
        Some(OneOrMany::One(single)) => Some(vec![single]),
        Some(OneOrMany::Many(list)) => Some(list),
    })
}

/// Options shared by every dependency via the `default` / `defaultVendorOptions` block.
///
/// The reference merges *whatever keys are present*, so every dependency key is mirrored here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultOptions {
    #[serde(default, deserialize_with = "de_lenient")]
    pub repository: Option<String>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub url: Option<String>,
    #[serde(rename = "versionUrl", default, deserialize_with = "de_lenient")]
    pub version_url: Option<String>,
    #[serde(rename = "versionRegex", default, deserialize_with = "de_lenient")]
    pub version_regex: Option<String>,
    #[serde(rename = "versionSelector", default, deserialize_with = "de_lenient")]
    pub version_selector: Option<String>,
    #[serde(
        rename = "versionJsonPointer",
        default,
        deserialize_with = "de_lenient"
    )]
    pub version_json_pointer: Option<String>,
    #[serde(default, deserialize_with = "de_files")]
    pub files: Option<Vec<FileEntry>>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub version: Option<String>,
    #[serde(rename = "hashVersionFile", default, deserialize_with = "de_lenient")]
    pub hash_version_file: Option<HashVersionFile>,
    #[serde(rename = "vendorFolder", default, deserialize_with = "de_lenient")]
    pub vendor_folder: Option<String>,
    #[serde(rename = "releaseRegex", default, deserialize_with = "de_lenient")]
    pub release_regex: Option<String>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub locked: Option<bool>,
    #[serde(default, deserialize_with = "de_lenient")]
    pub name: Option<String>,
}

/// A dependency exactly as written in the config file.
///
/// Every field is optional because the reference tool only validates on `sync`; other code
/// paths tolerate - and sometimes rely on - missing keys.
///
/// Field order is the order a newly written dependency appears in the config file, and is
/// chosen to match the reference project's own examples (`version`, `repository`, `files`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RawDependency {
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub version: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub repository: Option<String>,
    /// Where a dependency that is not on GitHub downloads from. Files reference it as `{url}`.
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub url: Option<String>,
    #[serde(
        rename = "versionUrl",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub version_url: Option<String>,
    #[serde(
        rename = "versionRegex",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub version_regex: Option<String>,
    #[serde(
        rename = "versionSelector",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub version_selector: Option<String>,
    #[serde(
        rename = "versionJsonPointer",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub version_json_pointer: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_files",
        skip_serializing_if = "Option::is_none"
    )]
    pub files: Option<Vec<FileEntry>>,
    #[serde(
        rename = "hashVersionFile",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub hash_version_file: Option<HashVersionFile>,
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub name: Option<String>,
    #[serde(
        rename = "vendorFolder",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub vendor_folder: Option<String>,
    #[serde(
        rename = "releaseRegex",
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub release_regex: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub locked: Option<bool>,
    /// Runs the one downloaded file as an installer instead of keeping it. Kept as written and
    /// checked by [`RawDependency::installer`], so a mistyped key fails rather than vanishing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installer: Option<serde_json::Value>,
}

impl RawDependency {
    /// What the `installer` key asks for, or `None` when it is absent or `false`.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::InvalidInstallerKey`] when the key holds anything else.
    pub fn installer(&self, name: &str) -> Result<Option<InstallerOptions>> {
        let Some(value) = &self.installer else {
            return Ok(None);
        };
        serde_json::from_value::<InstallerKey>(value.clone())
            .map(|key| key.options())
            .map_err(|_| VendorError::InvalidInstallerKey {
                name: name.to_owned(),
            })
    }

    /// Checks that an installer entry downloads exactly one file, the one that runs.
    fn validate_installer(&self, name: &str) -> Result<()> {
        if self.installer(name)?.is_none() {
            return Ok(());
        }
        let files = config_files_to_lock_files(self.files.as_deref().unwrap_or_default(), "");
        let count = flat_files(&files).len();
        if count == 1 {
            Ok(())
        } else {
            Err(VendorError::InstallerNeedsOneFile {
                name: name.to_owned(),
                count,
            })
        }
    }

    /// Fills unset fields from the config's `default` block, matching the reference's `??=`.
    ///
    /// Keys that belong to one kind of dependency fill only that kind, so a default
    /// `repository` never turns a `url` dependency into an invalid mix of both.
    pub fn apply_defaults(&mut self, defaults: &DefaultOptions) {
        macro_rules! fill {
            ($($field:ident),+ $(,)?) => {$(
                if self.$field.is_none() {
                    self.$field.clone_from(&defaults.$field);
                }
            )+};
        }
        if self.repository.is_none() {
            fill!(url);
        }
        if self.url.is_none() {
            fill!(repository, hash_version_file, release_regex);
        }
        if self.repository.is_none() {
            fill!(
                version_url,
                version_regex,
                version_selector,
                version_json_pointer
            );
        }
        fill!(files, version, vendor_folder, locked, name);
    }

    /// Clears every field whose value the `default` block already supplies.
    ///
    /// Used before writing a newly installed dependency back to the config, so the entry
    /// carries only what is actually specific to it rather than restating the defaults.
    pub fn strip_defaults(&mut self, defaults: &DefaultOptions) {
        macro_rules! clear_if_default {
            ($($field:ident),+ $(,)?) => {$(
                if self.$field.is_some() && self.$field == defaults.$field {
                    self.$field = None;
                }
            )+};
        }
        clear_if_default!(
            repository,
            url,
            version_url,
            version_regex,
            version_selector,
            version_json_pointer,
            files,
            version,
            hash_version_file,
            vendor_folder,
            release_regex,
            locked,
            name,
        );
    }

    /// Validates the fields needed to sync a dependency.
    ///
    /// # Errors
    ///
    /// Returns the `Invalid*Key` variant naming the offending config key.
    pub fn validate(&self, name: &str) -> Result<()> {
        if let Some(url) = &self.url {
            if self.repository.is_some() {
                return Err(VendorError::RepositoryAndUrl {
                    name: name.to_owned(),
                });
            }
            return self.validate_url(name, url);
        }
        if !self.repository.as_deref().is_some_and(is_github_url) {
            return Err(VendorError::InvalidRepositoryKey {
                name: name.to_owned(),
            });
        }
        if self.files.as_ref().is_none_or(Vec::is_empty) {
            return Err(VendorError::InvalidFilesKey {
                name: name.to_owned(),
            });
        }
        // `fancy_regex`, not `regex`: users write JavaScript patterns, which may use
        // lookaround (the reference README suggests `^v(?!.*-(?:alpha|beta)).*`).
        if let Some(regex) = self.release_regex.as_deref()
            && !regex.is_empty()
            && fancy_regex::Regex::new(regex).is_err()
        {
            return Err(VendorError::InvalidReleaseRegexKey {
                name: name.to_owned(),
            });
        }
        let url_only = [
            ("versionUrl", &self.version_url),
            ("versionRegex", &self.version_regex),
            ("versionSelector", &self.version_selector),
            ("versionJsonPointer", &self.version_json_pointer),
        ];
        if let Some((key, _)) = url_only.iter().find(|(_, value)| value.is_some()) {
            return Err(VendorError::KeyNeedsOtherMode {
                name: name.to_owned(),
                key,
                needs: "url",
            });
        }
        self.validate_installer(name)
    }

    /// The checks for a `url` dependency, whose files are URLs rather than repository paths.
    fn validate_url(&self, name: &str, url: &str) -> Result<()> {
        let name = || name.to_owned();
        if !is_http_url(url) {
            return Err(VendorError::InvalidUrlKey { name: name() });
        }
        let Some(files) = self.files.as_ref().filter(|f| !f.is_empty()) else {
            return Err(VendorError::InvalidFilesKey { name: name() });
        };
        let hashes = match &self.hash_version_file {
            None | Some(HashVersionFile::Flag(false)) => false,
            Some(HashVersionFile::Path(path)) => !path.is_empty(),
            Some(HashVersionFile::Flag(true)) => true,
        };
        let release_regex = self.release_regex.as_deref().is_some_and(|r| !r.is_empty());
        for (key, set) in [("hashVersionFile", hashes), ("releaseRegex", release_regex)] {
            if set {
                return Err(VendorError::KeyNeedsOtherMode {
                    name: name(),
                    key,
                    needs: "repository",
                });
            }
        }
        // `{url}` is expanded first, so the version can come from the downloaded file itself.
        let version_url = self.version_url.as_deref().map(|v| expand_url(v, url));
        if version_url.as_deref().is_some_and(|v| !is_http_url(v)) {
            return Err(VendorError::InvalidVersionUrlKey { name: name() });
        }
        if version_url.as_deref().is_some_and(uses_version) {
            return Err(VendorError::VersionUrlUsesVersion { name: name() });
        }
        if let Some(regex) = self.version_regex.as_deref()
            && fancy_regex::Regex::new(regex).is_err()
        {
            return Err(VendorError::InvalidVersionRegexKey { name: name() });
        }
        if self.version_regex.is_some() && version_url.is_none() {
            return Err(VendorError::RegexNeedsVersionUrl { name: name() });
        }
        if self
            .version_selector
            .as_deref()
            .is_some_and(|selector| VersionSelector::parse(selector).is_none())
        {
            return Err(VendorError::InvalidVersionSelectorKey { name: name() });
        }
        if self.version_selector.is_some() && self.version_json_pointer.is_some() {
            return Err(VendorError::SelectorAndJsonPointer { name: name() });
        }
        let page_only = [
            ("versionSelector", self.version_selector.is_some()),
            ("versionJsonPointer", self.version_json_pointer.is_some()),
        ];
        if let Some((key, _)) = page_only.iter().find(|(_, set)| *set)
            && version_url.as_deref().is_none_or(is_git_url)
        {
            return Err(VendorError::KeyNeedsPage { name: name(), key });
        }

        // The lockfile names every file before anything is downloaded, so each input needs an
        // explicit output name. A URL's last segment is often a version or a bare `download`.
        let mut mentions_version = uses_version(url);
        for entry in files {
            let map = match entry {
                FileEntry::Simple(input) => {
                    return Err(VendorError::UrlFileNeedsName {
                        name: name(),
                        input: input.clone(),
                    });
                }
                FileEntry::Mapped(map) => map,
            };
            for (input, target) in map {
                if !is_http_url(&expand_url(input, url)) {
                    return Err(VendorError::UrlFileNotUrl {
                        name: name(),
                        input: input.clone(),
                    });
                }
                mentions_version |= uses_version(input) || target_uses_version(target);
            }
        }
        let versioned =
            self.version.as_deref().is_some_and(|v| !v.is_empty()) || self.version_url.is_some();
        if mentions_version && !versioned {
            return Err(VendorError::UrlNeedsVersion { name: name() });
        }
        self.validate_installer(&name())
    }

    /// Whether this is a `url` dependency rather than a GitHub one.
    #[must_use]
    pub const fn is_url(&self) -> bool {
        self.url.is_some()
    }

    /// Turns a config entry into an installable [`Dependency`].
    ///
    /// `fallback_name` is the config key; the reference prefers it over the repository name
    /// and only consults `name` when the entry did not come from the dependency map. A `url`
    /// dependency with neither is named after the url's host.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::InvalidGitHubUrl`] when `repository` is missing or unparseable.
    pub fn resolve(&self, fallback_name: &str) -> Result<Dependency> {
        let (source, repository, own_name) = if let Some(url) = &self.url {
            let host = reqwest::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| url.clone());
            let source = UrlSource {
                url: url.clone(),
                version_url: self.version_url.clone(),
                version_regex: self.version_regex.clone().filter(|r| !r.is_empty()),
                version_selector: self.version_selector.clone(),
                version_json_pointer: self.version_json_pointer.clone(),
            };
            (Source::Url(source), url.clone(), host)
        } else {
            let repository = self
                .repository
                .clone()
                .ok_or_else(|| VendorError::InvalidGitHubUrl("undefined".to_owned()))?;
            let repo = owner_and_name_from_repo_url(&repository)?;
            let own_name = repo.name.clone();
            (Source::GitHub(repo), repository, own_name)
        };
        let name = if fallback_name.is_empty() {
            self.name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or(own_name)
        } else {
            fallback_name.to_owned()
        };

        let installer = self.installer(&name)?;
        Ok(Dependency {
            name,
            source,
            repository,
            files: self.files.clone().unwrap_or_default(),
            version: self.version.clone().filter(|v| !v.is_empty()),
            hash_version_file: self.hash_version_file.clone(),
            vendor_folder: self.vendor_folder.clone(),
            release_regex: self.release_regex.clone().filter(|r| !r.is_empty()),
            locked: self.locked.unwrap_or(false),
            installer,
        })
    }
}

/// Where a dependency's files come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Repository files and release assets.
    GitHub(Repository),
    /// Plain URLs.
    Url(UrlSource),
}

/// A dependency with everything the install pipeline needs, owned outright.
#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub source: Source,
    /// The GitHub repository URL, or the `url` value. Used for links and the lockfile.
    pub repository: String,
    pub files: Vec<FileEntry>,
    pub version: Option<String>,
    pub hash_version_file: Option<HashVersionFile>,
    pub vendor_folder: Option<String>,
    pub release_regex: Option<String>,
    pub locked: bool,
    /// Run the downloaded file as an installer, with these options.
    pub installer: Option<InstallerOptions>,
}

impl Dependency {
    /// Where a person can look the dependency up: its repository, or its `url`'s site while the
    /// `url` still holds a placeholder.
    #[must_use]
    pub fn link(&self) -> Cow<'_, str> {
        browsable_url(&self.repository)
    }

    /// Whether this dependency downloads from a `url` rather than GitHub.
    #[must_use]
    pub const fn is_url(&self) -> bool {
        matches!(self.source, Source::Url(_))
    }

    /// The `hashVersionFile` path to track, resolving `true` to the first declared file.
    ///
    /// `None` means "track releases instead".
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::InvalidHashVersionFile`] or
    /// [`VendorError::InvalidHashVersionFileTarget`] when `true` cannot be resolved to a file.
    pub fn hash_version_target(&self) -> Result<Option<String>> {
        match &self.hash_version_file {
            None | Some(HashVersionFile::Flag(false)) => Ok(None),
            Some(HashVersionFile::Path(path)) if path.is_empty() => Ok(None),
            Some(HashVersionFile::Path(path)) => Ok(Some(path.clone())),
            Some(HashVersionFile::Flag(true)) => match self.files.first() {
                Some(FileEntry::Simple(first)) => Ok(Some(first.clone())),
                Some(FileEntry::Mapped(map)) => map
                    .keys()
                    .next()
                    .cloned()
                    .map(Some)
                    .ok_or(VendorError::InvalidHashVersionFile),
                None => Err(VendorError::InvalidHashVersionFileTarget("undefined")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileEntry, FileTarget, HashVersionFile, RawDependency, flatten_files};

    fn dep(json: &str) -> RawDependency {
        serde_json::from_str(json).expect("valid dependency")
    }

    #[test]
    fn files_accepts_a_bare_string() {
        let d = dep(r#"{"files": "a/b.txt"}"#);
        assert_eq!(
            d.files.as_deref(),
            Some(&[FileEntry::Simple("a/b.txt".to_owned())][..])
        );
    }

    #[test]
    fn file_shorthands_match_arrays_and_produce_the_same_lock() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../vendorfiles.schema.json")).unwrap();
        let validator = jsonschema::draft202012::new(&schema).unwrap();
        for files in [
            serde_json::json!("LICENSE"),
            serde_json::json!({"LICENSE": "../licenses/COLORIS_LICENSE"}),
            serde_json::json!({
                "LICENSE": "COPYING",
                "{release}/a.zip": ["one"],
                "{release}/b.zip": {"in": "out"}
            }),
        ] {
            let raw = serde_json::json!({
                "repository": "https://github.com/a/b", "files": files,
                "hashVersionFile": true
            });
            let single: RawDependency = serde_json::from_value(raw.clone()).unwrap();
            let mut wrapped = raw.clone();
            wrapped["files"] = serde_json::json!([files]);
            let array: RawDependency = serde_json::from_value(wrapped.clone()).unwrap();
            single.validate("b").unwrap();
            assert_eq!(single.files, array.files);
            let single = single.resolve("b").unwrap();
            let array = array.resolve("b").unwrap();
            assert_eq!(
                single.hash_version_target().unwrap(),
                Some("LICENSE".into())
            );
            assert_eq!(
                crate::lockfile::config_files_to_lock_files(&single.files, "v1"),
                crate::lockfile::config_files_to_lock_files(&array.files, "v1")
            );
            for entry in [raw, wrapped] {
                assert!(
                    validator.is_valid(&serde_json::json!({"vendorDependencies": {"b": entry}}))
                );
                for key in ["default", "defaultVendorOptions"] {
                    assert!(validator.is_valid(&serde_json::json!({
                        "vendorDependencies": {}, key: {"files": entry["files"]}
                    })));
                }
            }
        }
    }

    #[test]
    fn schema_and_validation_reject_malformed_file_selections() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../vendorfiles.schema.json")).unwrap();
        let validator = jsonschema::draft202012::new(&schema).unwrap();
        for files in [
            "null",
            "true",
            "42",
            "[]",
            "[null]",
            "[[\"LICENSE\"]]",
            r#"{"LICENSE": null}"#,
            r#"{"LICENSE": false}"#,
            r#"{"LICENSE": 42}"#,
            r#"{"LICENSE": "COPYING", "bad": 42}"#,
            r#"{"{release}/a.zip": ["one", {"two": "out"}]}"#,
            r#"{"{release}/a.zip": {"one": ["out"]}}"#,
        ] {
            let value: serde_json::Value = serde_json::from_str(files).unwrap();
            for selection in [value.clone(), serde_json::json!([value])] {
                let entry = serde_json::json!({
                    "repository": "https://github.com/a/b", "files": selection
                });
                let raw: RawDependency = serde_json::from_value(entry.clone()).unwrap();
                assert!(raw.validate("b").is_err(), "accepted {selection}");
                assert!(
                    !validator.is_valid(&serde_json::json!({"vendorDependencies": {"b": entry}})),
                    "schema accepted {selection}"
                );
                // Defaults use the same schema, without the required dependency fields.
                assert!(
                    !validator.is_valid(&serde_json::json!({
                        "vendorDependencies": {}, "default": {"files": selection}
                    })),
                    "defaults schema accepted {selection}"
                );
            }
        }
    }

    #[test]
    fn empty_mappings_and_archive_selections_remain_valid() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../vendorfiles.schema.json")).unwrap();
        let validator = jsonschema::draft202012::new(&schema).unwrap();
        for files in [
            serde_json::json!({}),
            serde_json::json!({"{release}/a.zip": []}),
            serde_json::json!({"{release}/a.zip": {}}),
        ] {
            for selection in [files.clone(), serde_json::json!([files])] {
                let entry = serde_json::json!({
                    "repository": "https://github.com/a/b", "files": selection
                });
                let raw: RawDependency = serde_json::from_value(entry.clone()).unwrap();
                raw.validate("b").unwrap();
                let locked = crate::lockfile::config_files_to_lock_files(&raw.files.unwrap(), "v1");
                assert_eq!(crate::lockfile::flat_files(&locked), Vec::<String>::new());
                assert!(
                    validator.is_valid(&serde_json::json!({"vendorDependencies": {"b": entry}}))
                );
            }
        }
    }

    #[test]
    fn mapping_defaults_work_in_every_config_format() {
        use crate::config::ConfigFormat;
        for (format, text) in [
            (
                ConfigFormat::Json,
                r#"{"files":{"zeta":"COPYING","alpha":"a"}}"#,
            ),
            (ConfigFormat::Yml, "files:\n  zeta: COPYING\n  alpha: a\n"),
            (
                ConfigFormat::Toml,
                "files = { zeta = 'COPYING', alpha = 'a' }\n",
            ),
        ] {
            let value = format.parse(text, std::path::Path::new("vendor")).unwrap();
            let defaults: super::DefaultOptions = serde_json::from_value(value.clone()).unwrap();
            let direct: RawDependency = serde_json::from_value(value).unwrap();
            let mut inherited = RawDependency::default();
            inherited.apply_defaults(&defaults);
            assert_eq!(inherited.files, direct.files);
            assert_eq!(
                flatten_files(inherited.files.as_ref().unwrap())[0].output,
                FileTarget::Rename("COPYING".into())
            );
            let flat = flatten_files(inherited.files.as_ref().unwrap());
            assert_eq!(
                flat.iter()
                    .map(|file| file.input.as_str())
                    .collect::<Vec<_>>(),
                ["zeta", "alpha"]
            );
            inherited.repository = Some("https://github.com/a/b".into());
            inherited.hash_version_file = Some(HashVersionFile::Flag(true));
            assert_eq!(
                inherited
                    .resolve("b")
                    .unwrap()
                    .hash_version_target()
                    .unwrap(),
                Some("zeta".into())
            );
            let mut explicit = dep(r#"{"files":"README.md"}"#);
            explicit.apply_defaults(&defaults);
            assert_eq!(explicit.files, dep(r#"{"files":["README.md"]}"#).files);
        }
    }

    #[test]
    fn file_targets_discriminate_by_shape() {
        let d = dep(r#"{"files": [{"x": "y", "a.zip": ["one"], "b.zip": {"in": "out"}}]}"#);
        let FileEntry::Mapped(map) = &d.files.as_ref().unwrap()[0] else {
            panic!("expected a mapped entry");
        };
        assert_eq!(map["x"], FileTarget::Rename("y".to_owned()));
        assert_eq!(
            map["a.zip"],
            FileTarget::ExtractList(vec!["one".to_owned()])
        );
        let FileTarget::ExtractMap(inner) = &map["b.zip"] else {
            panic!("expected an extract map");
        };
        assert_eq!(inner["in"], "out");
    }

    #[test]
    fn flatten_preserves_declaration_order() {
        let d = dep(r#"{"files": ["a/one.txt", {"b": "c", "d": "e"}]}"#);
        let flat = flatten_files(d.files.as_ref().unwrap());
        let inputs: Vec<_> = flat.iter().map(|f| f.input.as_str()).collect();
        assert_eq!(inputs, ["a/one.txt", "b", "d"]);
        assert_eq!(flat[0].output, FileTarget::Rename("one.txt".to_owned()));
    }

    #[test]
    fn wrong_types_degrade_to_absent_rather_than_failing_to_parse() {
        let d = dep(r#"{"repository": 42, "files": {"LICENSE": 42}, "locked": "yes"}"#);
        assert!(d.repository.is_none());
        assert!(d.files.is_none());
        assert!(d.locked.is_none());
    }

    #[test]
    fn validate_reports_invalid_keys() {
        let d = dep(r#"{"files": ["a"]}"#);
        assert_eq!(
            d.validate("Foo").unwrap_err().to_string(),
            "config key 'vendorDependencies.Foo.repository' is not a valid github url"
        );

        let d = dep(r#"{"repository": "https://github.com/a/b", "files": []}"#);
        assert_eq!(
            d.validate("Foo").unwrap_err().to_string(),
            "config key 'vendorDependencies.Foo.files' is not a valid file path, mapping, or array"
        );

        let d =
            dep(r#"{"repository": "https://github.com/a/b", "files": ["x"], "releaseRegex": "("}"#);
        assert_eq!(
            d.validate("Foo").unwrap_err().to_string(),
            "config key 'vendorDependencies.Foo.releaseRegex' must be a valid regex string"
        );
    }

    fn err(json: &str) -> String {
        dep(json).validate("Foo").unwrap_err().to_string()
    }

    #[test]
    fn url_mode_validates_its_own_keys() {
        assert_eq!(
            err(
                r#"{"repository":"https://github.com/a/b","url":"https://x.io","files":[{"{url}":"a"}]}"#
            ),
            "vendorDependencies.Foo sets both 'repository' and 'url'; keep one"
        );
        assert_eq!(
            err(r#"{"url":"x.io","files":[{"{url}":"a"}]}"#),
            "config key 'vendorDependencies.Foo.url' is not a valid http(s) url"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","versionUrl":"x","files":[{"{url}":"a"}]}"#),
            "config key 'vendorDependencies.Foo.versionUrl' is not a valid http(s) url"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","versionRegex":"(","files":[{"{url}":"a"}]}"#),
            "config key 'vendorDependencies.Foo.versionRegex' must be a valid regex string"
        );
        assert_eq!(
            err(
                r#"{"url":"https://x.io","versionUrl":"https://x.io/v","versionSelector":"h2[","files":[{"{url}":"a"}]}"#
            ),
            "config key 'vendorDependencies.Foo.versionSelector' must be a valid CSS selector, optionally ending in ::attr(name)"
        );
        assert_eq!(
            err(
                r#"{"url":"https://x.io","versionUrl":"https://x.io/r.git","versionSelector":"h2","files":[{"{url}":"a"}]}"#
            ),
            "config key 'vendorDependencies.Foo.versionSelector' needs a 'versionUrl' that isn't a .git URL"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","versionSelector":"h2","files":[{"{url}":"a"}]}"#),
            "config key 'vendorDependencies.Foo.versionSelector' needs a 'versionUrl' that isn't a .git URL"
        );
        assert_eq!(
            err(
                r#"{"url":"https://x.io","versionUrl":"https://x.io/r.git","versionJsonPointer":"/v","files":[{"{url}":"a"}]}"#
            ),
            "config key 'vendorDependencies.Foo.versionJsonPointer' needs a 'versionUrl' that isn't a .git URL"
        );
        assert_eq!(
            err(
                r#"{"url":"https://x.io","versionUrl":"https://x.io/v","versionSelector":"h2","versionJsonPointer":"/v","files":[{"{url}":"a"}]}"#
            ),
            "config keys 'vendorDependencies.Foo.versionSelector' and 'versionJsonPointer' can't both be set; keep one"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","versionRegex":"v(.+)","files":[{"{url}":"a"}]}"#),
            "config key 'vendorDependencies.Foo.versionRegex' needs a 'versionUrl' to read"
        );
        assert_eq!(
            err(
                r#"{"url":"https://x.io/v{version}","version":"1.0.0","versionUrl":"{url}","files":[{"{url}":"a"}]}"#
            ),
            "config key 'vendorDependencies.Foo.versionUrl' can't hold {version} or {tag}, since it is what finds the version"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","hashVersionFile":true,"files":[{"{url}":"a"}]}"#),
            "config key 'vendorDependencies.Foo.hashVersionFile' only applies to a 'repository' dependency"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","files":["{url}/a.js"]}"#),
            r#"'{url}/a.js' in vendorDependencies.Foo.files needs an output name, like { "{url}/a.js": "file" }"#
        );
        assert_eq!(
            err(r#"{"url":"https://x.io","files":[{"{release}/a.zip":"a"}]}"#),
            "'{release}/a.zip' in vendorDependencies.Foo.files is not a URL"
        );
        assert_eq!(
            err(r#"{"url":"https://x.io/v{version}","files":[{"{url}":"a"}]}"#),
            "vendorDependencies.Foo uses {version} or {tag} but sets neither 'version' nor 'versionUrl'"
        );
    }

    #[test]
    fn an_installer_runs_exactly_one_file_and_its_key_is_checked() {
        let base = r#""repository":"https://github.com/a/b","installer":true"#;
        dep(&format!(r#"{{{base},"files":"{{release}}/setup.exe"}}"#))
            .validate("Foo")
            .unwrap();
        // One member out of an archive is one file too.
        dep(&format!(
            r#"{{{base},"files":{{"{{release}}/a.zip":["setup.exe"]}}}}"#
        ))
        .validate("Foo")
        .unwrap();
        assert_eq!(
            err(&format!(r#"{{{base},"files":["a.exe","b.exe"]}}"#)),
            "vendorDependencies.Foo sets 'installer', so its files must name exactly one file to run (they name 2)"
        );
        assert_eq!(
            err(
                r#"{"repository":"https://github.com/a/b","files":"a","installer":{"silent":true}}"#
            ),
            "config key 'vendorDependencies.Foo.installer' must be true, an installer type (nsis, inno, msi, msix, exe), or { type, args, scope }"
        );
        // `false` turns it off, so several files are fine again.
        dep(r#"{"repository":"https://github.com/a/b","files":["a","b"],"installer":false}"#)
            .validate("Foo")
            .unwrap();
    }

    #[test]
    fn github_mode_refuses_url_only_keys() {
        assert_eq!(
            err(
                r#"{"repository":"https://github.com/a/b","versionUrl":"https://x.io","files":["a"]}"#
            ),
            "config key 'vendorDependencies.Foo.versionUrl' only applies to a 'url' dependency"
        );
        assert_eq!(
            err(r#"{"repository":"https://github.com/a/b","versionSelector":"h2","files":["a"]}"#),
            "config key 'vendorDependencies.Foo.versionSelector' only applies to a 'url' dependency"
        );
        assert_eq!(
            err(
                r#"{"repository":"https://github.com/a/b","versionJsonPointer":"/v","files":["a"]}"#
            ),
            "config key 'vendorDependencies.Foo.versionJsonPointer' only applies to a 'url' dependency"
        );
    }

    #[test]
    fn valid_url_dependencies_pass() {
        for ok in [
            r#"{"url":"https://x.io/v{version}","versionUrl":"https://x.io/latest","files":[{"{url}":"tool.exe"}]}"#,
            r#"{"url":"https://x.io/v{tag}","version":"v1.0.0","files":[{"{url}/a.zip":["a"]}]}"#,
            r#"{"url":"https://x.io","files":[{"https://cdn.x.io/b.js":"b.js","{url}/c.zip":{"c":"d"}}]}"#,
            r#"{"url":"https://x.io/tool.exe","files":{"{url}":"tool.exe"}}"#,
            r#"{"url":"https://x.io/a.user.js","versionUrl":"{url}","versionRegex":"@version +([0-9.]+)","files":{"{url}":"a.js"}}"#,
            r##"{"url":"https://x.io/v{tag}","versionUrl":"https://x.io/changes","versionSelector":"#latest","files":[{"{url}":"t"}]}"##,
            r#"{"url":"https://x.io/{version}","versionUrl":"https://x.io/index.json","versionJsonPointer":"versions","files":[{"{url}":"t"}]}"#,
            r#"{"url":"https://x.io/{version}","versionUrl":"https://x.io/list.json","versionJsonPointer":"","files":[{"{url}":"t"}]}"#,
        ] {
            dep(ok).validate("Foo").unwrap();
        }
    }

    #[test]
    fn an_empty_version_regex_is_unset() {
        let d = dep(r#"{"url":"https://x.io","versionRegex":"","files":[{"{url}":"a"}]}"#);
        let super::Source::Url(source) = d.resolve("tool").unwrap().source else {
            panic!("a url dependency")
        };
        assert_eq!(source.version_regex, None);
    }

    #[test]
    fn defaults_fill_only_the_matching_mode() {
        let defaults: super::DefaultOptions = serde_json::from_str(
            r#"{"repository":"https://github.com/a/b","releaseRegex":"^v","hashVersionFile":true,"versionUrl":"https://x.io/v"}"#,
        )
        .unwrap();
        let mut url_dep = dep(r#"{"url":"https://x.io","files":[{"{url}":"a"}]}"#);
        url_dep.apply_defaults(&defaults);
        assert!(
            url_dep.repository.is_none()
                && url_dep.release_regex.is_none()
                && url_dep.hash_version_file.is_none()
        );
        assert_eq!(url_dep.version_url.as_deref(), Some("https://x.io/v"));
        url_dep.validate("Foo").unwrap();

        let mut gh_dep = dep(r#"{"files":["a"]}"#);
        gh_dep.apply_defaults(&defaults);
        assert!(gh_dep.version_url.is_none());
        assert_eq!(gh_dep.repository.as_deref(), Some("https://github.com/a/b"));
    }

    #[test]
    fn hash_version_target_resolves_true_to_the_first_file() {
        let d = dep(
            r#"{"repository":"https://github.com/a/b","files":["dist/x.js","y"],"hashVersionFile":true}"#,
        );
        let resolved = d.resolve("b").unwrap();
        assert_eq!(
            resolved.hash_version_target().unwrap().as_deref(),
            Some("dist/x.js")
        );

        let d = dep(
            r#"{"repository":"https://github.com/a/b","files":[{"k":"v"}],"hashVersionFile":true}"#,
        );
        let resolved = d.resolve("b").unwrap();
        assert_eq!(
            resolved.hash_version_target().unwrap().as_deref(),
            Some("k")
        );
    }

    #[test]
    fn hash_version_flag_false_means_track_releases() {
        let d =
            dep(r#"{"repository":"https://github.com/a/b","files":["x"],"hashVersionFile":false}"#);
        assert_eq!(d.hash_version_file, Some(HashVersionFile::Flag(false)));
        assert!(
            d.resolve("b")
                .unwrap()
                .hash_version_target()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn defaults_only_fill_absent_fields() {
        let mut d = dep(r#"{"version": "v1"}"#);
        let defaults: super::DefaultOptions = serde_json::from_str(
            r#"{"repository": "https://github.com/a/b", "version": "v9", "hashVersionFile": true}"#,
        )
        .unwrap();
        d.apply_defaults(&defaults);
        assert_eq!(d.version.as_deref(), Some("v1"));
        assert_eq!(d.repository.as_deref(), Some("https://github.com/a/b"));
        assert_eq!(d.hash_version_file, Some(HashVersionFile::Flag(true)));
    }
}
