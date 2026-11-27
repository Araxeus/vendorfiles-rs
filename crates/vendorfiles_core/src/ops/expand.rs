//! Turning a dependency's folders and patterns into concrete files, before anything is written.
//!
//! Runs between `prepare` and `clear_previous`, so the pending lock entry can list every output
//! an install may write. Only dependencies with a folder or pattern, and work to do, list
//! anything. A repository is listed through the Trees API. An archive is downloaded here and
//! listed, and the download stage extracts from that same copy.

use std::path::{Path, PathBuf};

use crate::archive;
use crate::error::{Result, VendorError};
use crate::fsx::{anchor, stream_to_file};
use crate::github::{GitHubClient, TreeEntry};
use crate::lockfile::{LockFiles, expanded_lock};
use crate::model::{Dependency, FileTarget, Repository, Source};
use crate::ops::Session;
use crate::ops::install::{Prepared, archive_name, cannot_extract, place_members, save};
use crate::progress;
use crate::selection::{ArchiveSelection, Matched, Member, Selection, Selections};
use crate::template::{expand_url, replace_version, strip_release_prefix};
use crate::url_source;

/// A dependency's folders and patterns, expanded against one version.
#[derive(Debug)]
pub struct Planned {
    /// The ref every repository listing and download uses.
    git_ref: String,
    /// The lock `files`, with folders and patterns expanded.
    pub files: LockFiles,
    /// Source path → output, for every repository file a folder or pattern selected.
    downloads: Vec<(String, String)>,
    /// The archives that were downloaded to be listed.
    archives: Vec<Fetched>,
}

impl Planned {
    /// The downloaded copy of the archive under `files` key `key`, if it was listed.
    pub(super) fn archive(&self, key: &str) -> Option<&Fetched> {
        self.archives.iter().find(|fetched| fetched.key == key)
    }
}

/// An archive downloaded to list its members, kept for the extraction that follows.
#[derive(Debug)]
pub(super) struct Fetched {
    key: String,
    name: String,
    path: PathBuf,
    temp: tempfile::TempDir,
}

impl Fetched {
    /// Extracts `pairs` from the archive and moves each member to its output in `folder`.
    pub(super) async fn place(
        &self,
        pairs: &[(String, String)],
        version: &str,
        folder: &Path,
        progress: &progress::Dependency,
    ) -> Result<Vec<PathBuf>> {
        let extracted = self.temp.path().join("extracted");
        let wanted: Vec<String> = pairs.iter().map(|(member, _)| member.clone()).collect();
        let (archive, target, name) = (self.path.clone(), extracted.clone(), self.name.clone());
        progress.status(format!("extracting {name}"));
        tokio::task::spawn_blocking(move || archive::extract_members(&archive, &target, &wanted))
            .await
            .map_err(|e| VendorError::Http(e.to_string()))?
            .map_err(|source| cannot_extract(&name, source))?;
        place_members(&extracted, pairs, version, folder).await
    }
}

impl Session {
    /// Lists what `prepared`'s folders and patterns select, and records it.
    ///
    /// A failure is kept on `prepared` and reported by `clear_previous`, so `sync` still meets
    /// errors in config order.
    pub async fn expand(&self, prepared: &mut Prepared) {
        if !prepared.has_work() || prepared.dependency.installer.is_some() {
            return;
        }
        let selections = match prepared.dependency.selections(&prepared.version) {
            Ok(selections) if selections.is_empty() => return,
            Ok(selections) => selections,
            Err(error) => {
                prepared.blocked = Some(error);
                return;
            }
        };
        prepared.progress.status("listing files");
        match plan(
            &self.github,
            &prepared.dependency,
            &selections,
            &prepared.version,
            &prepared.progress,
        )
        .await
        {
            Ok(planned) => prepared.plan = Some(planned),
            Err(error) => prepared.blocked = Some(error),
        }
    }
}

/// Expands every selection: the repository's against one listing per distinct base folder, an
/// archive's against its member list.
async fn plan(
    github: &GitHubClient,
    dependency: &Dependency,
    selections: &Selections,
    version: &str,
    progress: &progress::Dependency,
) -> Result<Planned> {
    let mut git_ref = version.to_owned();
    let mut matched = Matched::default();
    if let Source::GitHub(repo) = &dependency.source
        && !selections.repo.is_empty()
    {
        // A branch's raw files are cached for minutes, so the default branch is pinned to the
        // commit it points at. Tags and commit SHAs are used as they are.
        if version.is_empty() {
            git_ref = github.commit_sha(repo, "HEAD").await?;
        }
        let at = if version.is_empty() {
            "the default branch"
        } else {
            version
        };
        matched.repo =
            match_repository(github, dependency, repo, &selections.repo, &git_ref, at).await?;
    }

    let mut archives = Vec::with_capacity(selections.archives.len());
    for archive in &selections.archives {
        let fetched = fetch(github, dependency, archive, version, progress).await?;
        let path = fetched.path.clone();
        let members = tokio::task::spawn_blocking(move || archive::members(&path))
            .await
            .map_err(|e| VendorError::Http(e.to_string()))?
            .map_err(|source| cannot_extract(&fetched.name, source))?;
        matched
            .archives
            .push(match_members(dependency, archive, &fetched.name, &members)?);
        archives.push(fetched);
    }

    let files = expanded_lock(
        &dependency.name,
        &dependency.files,
        selections,
        &matched,
        version,
    )?;
    let downloads = selections
        .repo
        .iter()
        .flat_map(|selection| match files.get(&selection.key) {
            Some(FileTarget::ExtractMap(placed)) => placed.clone().into_iter().collect(),
            _ => Vec::new(),
        })
        .collect();
    Ok(Planned {
        git_ref,
        files,
        downloads,
        archives,
    })
}

/// What each repository selection matches, from one listing per distinct base folder.
///
/// `at` names the version in a "matched nothing" error.
async fn match_repository(
    github: &GitHubClient,
    dependency: &Dependency,
    repo: &Repository,
    selections: &[Selection],
    git_ref: &str,
    at: &str,
) -> Result<Vec<Vec<String>>> {
    let mut listings: Vec<(String, Vec<TreeEntry>)> = Vec::new();
    for base in listed_bases(selections) {
        let entries = match github.list_tree(repo, git_ref, &base).await {
            Ok(entries) => entries,
            // A missing folder, or a path that is a file, holds nothing to select.
            Err(VendorError::RequestFailed {
                status: 404 | 422, ..
            }) => Vec::new(),
            Err(error) => return Err(error),
        };
        listings.push((base, entries));
    }

    let mut matched = Vec::with_capacity(selections.len());
    for selection in selections {
        let entries = listings
            .iter()
            .find(|(base, _)| selection.base.starts_with(base.as_str()))
            .map_or(&[][..], |(_, entries)| entries.as_slice());
        let sources = select(dependency, selection, entries)?;
        if sources.is_empty() {
            let reason = if selection.is_pattern() {
                format!("matched no files in {} at {at}", dependency.repository)
            } else {
                format!(
                    "names no folder with files in {} at {at}",
                    dependency.repository
                )
            };
            return Err(VendorError::InvalidSelection {
                name: dependency.name.clone(),
                key: selection.key.clone(),
                reason,
            });
        }
        matched.push(sources);
    }
    Ok(matched)
}

/// What each declared member of `archive` matches among `members`, sorted.
fn match_members(
    dependency: &Dependency,
    archive: &ArchiveSelection,
    name: &str,
    members: &[String],
) -> Result<Vec<Vec<String>>> {
    archive
        .members
        .iter()
        .map(|(key, member)| {
            let Member::Selected(selection) = member else {
                return Ok(Vec::new());
            };
            let mut found: Vec<String> = members
                .iter()
                .filter(|m| selection.matches(m))
                .cloned()
                .collect();
            found.sort();
            if found.is_empty() {
                return Err(VendorError::InvalidSelection {
                    name: dependency.name.clone(),
                    key: key.clone(),
                    reason: format!("matched no members of {name}"),
                });
            }
            Ok(found)
        })
        .collect()
}

/// Downloads `archive` into a temporary folder of its own, under its own name.
///
/// The name matters, because a lone `.gz` or `.xz` extracts to its name minus the suffix.
async fn fetch(
    github: &GitHubClient,
    dependency: &Dependency,
    archive: &ArchiveSelection,
    version: &str,
    progress: &progress::Dependency,
) -> Result<Fetched> {
    let failed = |source: VendorError| VendorError::FileDownloadFailed {
        file: archive.key.clone(),
        repository: dependency.repository.clone(),
        version: version.to_owned(),
        source: Box::new(source),
    };
    let (response, name) = match &dependency.source {
        Source::GitHub(repo) => {
            let asset = strip_release_prefix(&replace_version(&archive.key, version));
            let response = github
                .download_release_asset(repo, &asset, version, dependency.release_regex.as_deref())
                .await?;
            (response, asset)
        }
        Source::Url(source) => {
            let url = replace_version(&expand_url(&archive.key, &source.url), version);
            let response = url_source::fetch(github.http(), &url)
                .await
                .map_err(failed)?;
            // After redirects, so `/latest` saves under the versioned name it points at.
            let name = archive_name(response.url().as_str());
            (response, name)
        }
    };

    let temp = tempfile::Builder::new()
        .prefix("vendorfiles-")
        .tempdir()
        .map_err(VendorError::from)?;
    let folder = temp.path().join("archive");
    tokio::fs::create_dir_all(&folder).await?;
    let path = folder.join(&name);
    progress.status(format!("downloading {name}"));
    save(response, &path, progress)
        .await
        .map_err(|source| cannot_extract(&name, source))?;
    Ok(Fetched {
        key: archive.key.clone(),
        name,
        path,
        temp,
    })
}

/// The folders to list: each selection's base, minus any inside another one listed.
fn listed_bases(selections: &[Selection]) -> Vec<String> {
    let mut bases: Vec<&str> = selections.iter().map(|s| s.base.as_str()).collect();
    bases.sort_by_key(|base| base.len());
    let mut kept: Vec<String> = Vec::new();
    for base in bases {
        if !kept.iter().any(|outer| base.starts_with(outer.as_str())) {
            kept.push(base.to_owned());
        }
    }
    kept
}

/// The regular files `selection` takes from `entries`, sorted by path.
///
/// Folders inside a match are walked by the listing itself, so a matching folder entry is
/// skipped. A symlink or submodule that matches is an error.
fn select(
    dependency: &Dependency,
    selection: &Selection,
    entries: &[TreeEntry],
) -> Result<Vec<String>> {
    let mut sources = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| selection.matches(&entry.path))
    {
        if entry.kind == "tree" {
            continue;
        }
        if !entry.is_file() {
            let what = if entry.kind == "commit" {
                "a submodule"
            } else {
                "a symlink"
            };
            return Err(VendorError::InvalidSelection {
                name: dependency.name.clone(),
                key: selection.key.clone(),
                reason: format!(
                    "selects '{}', which is {what}. vendor copies only regular files",
                    entry.path
                ),
            });
        }
        sources.push(entry.path.clone());
    }
    sources.sort();
    Ok(sources)
}

/// Downloads every file a folder or pattern selected, from the raw host.
pub(super) async fn download_selected(
    github: &GitHubClient,
    dependency: &Dependency,
    repo: &Repository,
    planned: &Planned,
    folder: &Path,
    progress: &progress::Dependency,
) -> Result<Vec<PathBuf>> {
    if planned.downloads.is_empty() {
        return Ok(Vec::new());
    }
    progress.status(format!("downloading {} file(s)", planned.downloads.len()));
    futures_util::future::try_join_all(planned.downloads.iter().map(|(source, output)| async {
        let _permit = github.raw_permit().await;
        let response = github
            .download_raw(repo, &planned.git_ref, source)
            .await
            .map_err(|error| VendorError::FileDownloadFailed {
                file: source.clone(),
                repository: dependency.repository.clone(),
                version: planned.git_ref.clone(),
                source: Box::new(error),
            })?;
        let path = anchor(folder, output);
        let transfer = progress.transfer(response.content_length());
        stream_to_file(response, &path, Some(&transfer)).await?;
        drop(transfer);
        Ok::<_, VendorError>(path)
    }))
    .await
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::Ordering;

    use serde_json::json;

    use super::listed_bases;
    use crate::config::Workspace;
    use crate::github::{GitHubClient, Token};
    use crate::lockfile::read_lockfile;
    use crate::model::FileEntry;
    use crate::ops::{Session, SyncOptions};
    use crate::progress::Reporter;
    use crate::selection::selections;
    use crate::test_server::{Route, route, serve_counted};

    const SKILLS_TREE: &str = r#"{"truncated": false, "tree": [
        {"path": "pdf", "mode": "040000", "type": "tree"},
        {"path": "pdf/SKILL.md", "mode": "100644", "type": "blob"},
        {"path": "pdf/notes.txt", "mode": "100644", "type": "blob"},
        {"path": "xlsx/SKILL.md", "mode": "100644", "type": "blob"}
    ]}"#;

    const FONTS_TREE: &str = r#"{"truncated": false, "tree": [
        {"path": "a.ttf", "mode": "100644", "type": "blob"},
        {"path": "sub", "mode": "040000", "type": "tree"},
        {"path": "sub/b.ttf", "mode": "100644", "type": "blob"}
    ]}"#;

    fn repository() -> Vec<Route> {
        vec![
            route(
                "/repos/a/b/git/trees/v1%3Askills?recursive=1",
                200,
                "",
                SKILLS_TREE.as_bytes(),
            ),
            route(
                "/repos/a/b/git/trees/v1%3Afonts?recursive=1",
                200,
                "",
                FONTS_TREE.as_bytes(),
            ),
            route("/a/b/v1/skills/pdf/SKILL.md", 200, "", b"pdf skill"),
            route("/a/b/v1/skills/xlsx/SKILL.md", 200, "", b"xlsx skill"),
            route("/a/b/v1/fonts/a.ttf", 200, "", b"a"),
            route("/a/b/v1/fonts/sub/b.ttf", 200, "", b"b"),
        ]
    }

    fn config(pattern: &serde_json::Value) -> String {
        json!({ "vendorDependencies": { "skills": {
            "repository": "https://github.com/a/b",
            "version": "v1",
            "files": [{ "fonts/": "myfonts/" }, pattern],
        }}})
        .to_string()
    }

    async fn sync(dir: &Path, config: &str, base: &str) -> crate::error::Result<()> {
        std::fs::write(dir.join("vendor.json"), config).unwrap();
        let workspace = Workspace::load(dir.to_str()).await.unwrap();
        let github = GitHubClient::new(Some(Token::new("test")))
            .unwrap()
            .with_roots(base, base);
        Session::with_reporter(github, workspace, Reporter::new(false))
            .sync(SyncOptions::default())
            .await
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|_| panic!("{} exists", path.display()))
    }

    #[tokio::test]
    async fn folders_and_patterns_install_and_then_stay_in_sync_without_a_request() {
        let (base, hits) = serve_counted(repository());
        let dir = tempfile::tempdir().unwrap();
        let config = config(&json!({ "skills/**/SKILL.md": "{parent}.md" }));
        sync(dir.path(), &config, &base).await.unwrap();

        let folder = dir.path().join("vendor/skills");
        assert_eq!(read(&folder.join("pdf.md")), "pdf skill");
        assert_eq!(read(&folder.join("xlsx.md")), "xlsx skill");
        assert_eq!(read(&folder.join("myfonts/a.ttf")), "a");
        assert_eq!(read(&folder.join("myfonts/sub/b.ttf")), "b");
        assert!(!folder.join("notes.txt").exists());

        let lock = read_lockfile(&folder.join("vendor-lock.json"))
            .await
            .unwrap();
        let entry = serde_json::to_value(&lock["skills"]).unwrap();
        assert_eq!(
            entry["files"],
            json!({
                "fonts/": { "fonts/a.ttf": "myfonts/a.ttf", "fonts/sub/b.ttf": "myfonts/sub/b.ttf" },
                "skills/**/SKILL.md": {
                    "skills/pdf/SKILL.md": "pdf.md",
                    "skills/xlsx/SKILL.md": "xlsx.md",
                },
            })
        );

        let before = hits.load(Ordering::SeqCst);
        sync(dir.path(), &config, &base).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            before,
            "an unchanged install asks nothing"
        );
    }

    #[tokio::test]
    async fn a_changed_destination_moves_the_files() {
        let (base, _) = serve_counted(repository());
        let dir = tempfile::tempdir().unwrap();
        sync(
            dir.path(),
            &config(&json!({ "skills/**/SKILL.md": "{parent}.md" })),
            &base,
        )
        .await
        .unwrap();
        sync(
            dir.path(),
            &config(&json!({ "skills/**/SKILL.md": "docs/{parent}.md" })),
            &base,
        )
        .await
        .unwrap();

        let folder = dir.path().join("vendor/skills");
        assert_eq!(read(&folder.join("docs/pdf.md")), "pdf skill");
        assert!(!folder.join("pdf.md").exists(), "the old output is removed");
        assert_eq!(read(&folder.join("myfonts/a.ttf")), "a");
    }

    #[tokio::test]
    async fn a_pattern_that_matches_nothing_fails_and_writes_nothing() {
        let (base, _) = serve_counted(repository());
        let dir = tempfile::tempdir().unwrap();
        let error = sync(dir.path(), &config(&json!("skills/**/*.py")), &base)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("matched no files"), "{error}");
        assert!(!dir.path().join("vendor/skills/myfonts").exists());
    }

    #[tokio::test]
    async fn two_sources_on_one_output_fail_before_any_download() {
        let (base, _) = serve_counted(repository());
        let dir = tempfile::tempdir().unwrap();
        let error = sync(
            dir.path(),
            &config(&json!({ "skills/**/SKILL.md": "SKILL.md" })),
            &base,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("writes SKILL.md"), "{error}");
        assert!(!dir.path().join("vendor/skills/myfonts").exists());
    }

    #[tokio::test]
    async fn a_selected_symlink_is_refused() {
        let tree = r#"{"truncated": false, "tree": [
            {"path": "link", "mode": "120000", "type": "blob"}
        ]}"#;
        let mut routes = repository();
        routes.push(route(
            "/repos/a/b/git/trees/v1%3Alinks?recursive=1",
            200,
            "",
            tree.as_bytes(),
        ));
        let (base, _) = serve_counted(routes);
        let dir = tempfile::tempdir().unwrap();
        let error = sync(dir.path(), &config(&json!("links/*")), &base)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("is a symlink"), "{error}");
    }

    #[test]
    fn a_base_inside_another_is_listed_once() {
        let files: Vec<FileEntry> =
            serde_json::from_str(r#"["skills/**/SKILL.md", "skills/pdf/*.py", "docs/*.md"]"#)
                .unwrap();
        let found = selections(&files, "v1").unwrap();
        assert_eq!(listed_bases(&found), ["docs/", "skills/"]);
    }

    fn zip_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in members {
            writer
                .start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn tool_zip() -> Vec<u8> {
        zip_bytes(&[
            ("tool-1.2.3/bin/a", b"a"),
            ("tool-1.2.3/bin/b", b"b"),
            ("tool-1.2.3/docs/guide.md", b"guide"),
            ("tool-1.2.3/docs/api/ref.md", b"ref"),
            ("tool-1.2.3/LICENSE", b"license"),
        ])
    }

    fn url_config(base: &str, files: &serde_json::Value) -> String {
        json!({ "vendorDependencies": { "tool": {
            "url": format!("{base}/tool.zip"),
            "version": "v1.2.3",
            "files": files,
        }}})
        .to_string()
    }

    #[tokio::test]
    async fn folders_and_patterns_select_archive_members() {
        let (base, hits) = serve_counted(vec![route("/tool.zip", 200, "", &tool_zip())]);
        let dir = tempfile::tempdir().unwrap();
        let config = url_config(
            &base,
            &json!({
                "tool-{version}/bin/*": "bin/",
                "tool-{version}/docs/": "manual/",
                "tool-{version}/LICENSE": "LICENSE",
            }),
        );
        sync(dir.path(), &config, &base).await.unwrap();

        let folder = dir.path().join("vendor/tool");
        assert_eq!(read(&folder.join("bin/a")), "a");
        assert_eq!(read(&folder.join("bin/b")), "b");
        assert_eq!(read(&folder.join("manual/guide.md")), "guide");
        assert_eq!(read(&folder.join("manual/api/ref.md")), "ref");
        assert_eq!(read(&folder.join("LICENSE")), "license");

        let lock = read_lockfile(&folder.join("vendor-lock.json"))
            .await
            .unwrap();
        let entry = serde_json::to_value(&lock["tool"]).unwrap();
        assert_eq!(
            entry["files"]["{url}"],
            json!({
                "tool-{version}/bin/*": {
                    "tool-1.2.3/bin/a": "bin/a",
                    "tool-1.2.3/bin/b": "bin/b",
                },
                "tool-{version}/docs/": {
                    "tool-1.2.3/docs/api/ref.md": "manual/api/ref.md",
                    "tool-1.2.3/docs/guide.md": "manual/guide.md",
                },
                "tool-{version}/LICENSE": { "tool-1.2.3/LICENSE": "LICENSE" },
            })
        );

        let before = hits.load(Ordering::SeqCst);
        sync(dir.path(), &config, &base).await.unwrap();
        assert_eq!(
            hits.load(Ordering::SeqCst),
            before,
            "an unchanged install asks nothing"
        );
    }

    #[tokio::test]
    async fn a_changed_member_pattern_reinstalls() {
        let (base, _) = serve_counted(vec![route("/tool.zip", 200, "", &tool_zip())]);
        let dir = tempfile::tempdir().unwrap();
        sync(
            dir.path(),
            &url_config(&base, &json!({ "tool-{version}/bin/a": "a" })),
            &base,
        )
        .await
        .unwrap();
        sync(
            dir.path(),
            &url_config(&base, &json!({ "tool-{version}/bin/*": "bin/" })),
            &base,
        )
        .await
        .unwrap();

        let folder = dir.path().join("vendor/tool");
        assert!(!folder.join("a").exists(), "the old output is removed");
        assert_eq!(read(&folder.join("bin/a")), "a");
        assert_eq!(read(&folder.join("bin/b")), "b");
    }

    #[tokio::test]
    async fn a_member_pattern_that_matches_nothing_fails_and_writes_nothing() {
        let (base, _) = serve_counted(vec![route("/tool.zip", 200, "", &tool_zip())]);
        let dir = tempfile::tempdir().unwrap();
        let error = sync(
            dir.path(),
            &url_config(&base, &json!({ "tool-{version}/lib/*": "lib/" })),
            &base,
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("matched no members of tool.zip"),
            "{error}"
        );
        assert!(!dir.path().join("vendor/tool").exists());
    }
}
