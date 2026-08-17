//! The GitHub API surface the tool needs, plus streaming downloads.

pub mod auth;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, Once};

use indexmap::IndexMap;
use octocrab::models::repos::Release;
use octocrab::Octocrab;
use serde::Deserialize;

use crate::error::{Result, VendorError};
use crate::model::Repository;
use crate::ui;

pub use auth::Token;

/// Sent on every request; the GitHub API rejects requests without one.
pub const USER_AGENT: &str = concat!("vendorfiles/", env!("CARGO_PKG_VERSION"));

const API_ROOT: &str = "https://api.github.com";

/// Cache key for a resolved release.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ReleaseKey {
    Tag { repo: String, tag: String },
    Latest { repo: String },
    Regex { repo: String, regex: String },
}

impl ReleaseKey {
    fn repo(&self) -> &str {
        match self {
            Self::Tag { repo, .. } | Self::Latest { repo } | Self::Regex { repo, .. } => repo,
        }
    }
}

/// Client for everything the tool does against github.com.
///
/// Releases are cached for the process lifetime so two dependencies tracking the same
/// repository cost one request, as in the reference.
pub struct GitHubClient {
    api: Octocrab,
    http: reqwest::Client,
    token: Option<Token>,
    releases: Mutex<IndexMap<ReleaseKey, Arc<Release>>>,
    warned: Once,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("authenticated", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl GitHubClient {
    /// Builds a client from the resolved token (or anonymously).
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::Http`] if the underlying HTTP clients cannot be built.
    pub fn new(token: Option<Token>) -> Result<Self> {
        let mut builder = Octocrab::builder();
        if let Some(token) = &token {
            builder = builder.personal_token(token.expose().to_owned());
        }
        let api = builder.build().map_err(VendorError::from)?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .map_err(|e| VendorError::Http(e.to_string()))?;

        Ok(Self {
            api,
            http,
            token,
            releases: Mutex::new(IndexMap::new()),
            warned: Once::new(),
        })
    }

    /// Warns once, on first use, that anonymous requests are rate limited.
    fn warn_if_anonymous(&self) {
        if self.token.is_none() {
            self.warned.call_once(|| {
                ui::warning(
                    "You may be rate limited, run `vendor login` or use a GITHUB_TOKEN env variable",
                );
            });
        }
    }

    fn cached(&self, key: &ReleaseKey) -> Option<Arc<Release>> {
        let releases = self.releases.lock().ok()?;
        releases.get(key).cloned()
    }

    fn store(&self, key: ReleaseKey, release: Arc<Release>) {
        if let Ok(mut releases) = self.releases.lock() {
            releases.insert(key, release);
        }
    }

    /// Looks a release up by tag, reusing any cached release of the same repo with that tag.
    ///
    /// The second half of that rule is the reference's `startsWith` scan; it is what makes a
    /// `getLatestRelease` result serve a later `getReleaseFromTag` for the same tag.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::RequestFailed`] if the release does not exist or the API rejects
    /// the request.
    pub async fn release_by_tag(&self, repo: &Repository, tag: &str) -> Result<Arc<Release>> {
        let repo_key = repo.to_string();
        let key = ReleaseKey::Tag {
            repo: repo_key.clone(),
            tag: tag.to_owned(),
        };
        if let Ok(releases) = self.releases.lock() {
            for (cached_key, release) in releases.iter() {
                if *cached_key == key || (cached_key.repo() == repo_key && release.tag_name == tag)
                {
                    return Ok(release.clone());
                }
            }
        }

        self.warn_if_anonymous();
        let release = Arc::new(
            self.api
                .repos(&repo.owner, &repo.name)
                .releases()
                .get_by_tag(tag)
                .await
                .map_err(VendorError::from)?,
        );
        self.store(key, release.clone());
        Ok(release)
    }

    /// The latest release, or the newest one matching `release_regex` when given.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::RequestFailed`] if the API rejects the request, or
    /// [`VendorError::NoMatchingRelease`] when `release_regex` matches nothing.
    pub async fn latest_release(
        &self,
        repo: &Repository,
        release_regex: Option<&str>,
    ) -> Result<Arc<Release>> {
        if let Some(regex) = release_regex {
            return self.release_by_regex(repo, regex).await;
        }
        let key = ReleaseKey::Latest {
            repo: repo.to_string(),
        };
        if let Some(cached) = self.cached(&key) {
            return Ok(cached);
        }

        self.warn_if_anonymous();
        let release = Arc::new(
            self.api
                .repos(&repo.owner, &repo.name)
                .releases()
                .get_latest()
                .await
                .map_err(VendorError::from)?,
        );
        self.store(key, release.clone());
        Ok(release)
    }

    /// The first release (newest first) whose tag or title matches `release_regex`.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::NoMatchingRelease`] when nothing matches, or
    /// [`VendorError::RequestFailed`] if the release list cannot be fetched.
    pub async fn release_by_regex(&self, repo: &Repository, regex: &str) -> Result<Arc<Release>> {
        let key = ReleaseKey::Regex {
            repo: repo.to_string(),
            regex: regex.to_owned(),
        };
        if let Some(cached) = self.cached(&key) {
            return Ok(cached);
        }

        self.warn_if_anonymous();
        // `fancy_regex` accepts the JavaScript patterns users already have, lookaround included.
        let compiled =
            fancy_regex::Regex::new(regex).map_err(|e| VendorError::Http(e.to_string()))?;
        let page = self
            .api
            .repos(&repo.owner, &repo.name)
            .releases()
            .list()
            .per_page(100)
            .send()
            .await
            .map_err(VendorError::from)?;

        let matches = |text: &str| compiled.is_match(text).unwrap_or(false);
        let matched = page.items.into_iter().find(|release| {
            matches(&release.tag_name) || matches(release.name.as_deref().unwrap_or(""))
        });

        let Some(release) = matched else {
            return Err(VendorError::NoMatchingRelease {
                regex: regex.to_owned(),
                owner: repo.owner.clone(),
                repo: repo.name.clone(),
            });
        };
        let release = Arc::new(release);
        self.store(key, release.clone());
        Ok(release)
    }

    /// The SHA of the most recent commit touching `path`.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::NoCommitsFound`] when the path has no history, or
    /// [`VendorError::RequestFailed`] if the API rejects the request.
    pub async fn file_commit_sha(&self, repo: &Repository, path: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Commit {
            sha: String,
        }

        self.warn_if_anonymous();
        let route = format!(
            "/repos/{}/{}/commits?path={}&per_page=1",
            repo.owner,
            repo.name,
            encode_query(path)
        );
        let commits: Vec<Commit> = self
            .api
            .get(route, None::<&()>)
            .await
            .map_err(VendorError::from)?;

        commits
            .into_iter()
            .next()
            .map(|c| c.sha)
            .ok_or_else(|| VendorError::NoCommitsFound {
                owner: repo.owner.clone(),
                repo: repo.name.clone(),
                path: path.to_owned(),
            })
    }

    /// Starts a streaming download of a file from the repository tree.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::RequestFailed`] for any non-2xx response, including a missing file.
    pub async fn download_file(
        &self,
        repo: &Repository,
        path: &str,
        git_ref: Option<&str>,
    ) -> Result<reqwest::Response> {
        self.warn_if_anonymous();
        let mut url = format!(
            "{API_ROOT}/repos/{}/{}/contents/{}",
            repo.owner,
            repo.name,
            encode_path(path)
        );
        if let Some(git_ref) = git_ref.filter(|r| !r.is_empty()) {
            url.push_str("?ref=");
            url.push_str(&encode_query(git_ref));
        }
        self.send(url, "application/vnd.github.raw").await
    }

    /// Starts a streaming download of a named release asset.
    ///
    /// `version` selects the release by tag; when empty, the latest (or regex-matched) release
    /// is used instead.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::ReleaseNotFound`], [`VendorError::ReleaseAssetsMissing`],
    /// [`VendorError::ReleaseAssetNotFound`] or [`VendorError::ReleaseAssetDownloadFailed`]
    /// depending on which step fails.
    pub async fn download_release_asset(
        &self,
        repo: &Repository,
        asset_name: &str,
        version: &str,
        release_regex: Option<&str>,
    ) -> Result<reqwest::Response> {
        let release = if version.is_empty() {
            self.latest_release(repo, release_regex).await?
        } else {
            self.release_by_tag(repo, version)
                .await
                .map_err(|_| VendorError::ReleaseNotFound {
                    version: version.to_owned(),
                    owner: repo.owner.clone(),
                    repo: repo.name.clone(),
                })?
        };

        let release_url = release.url.to_string();
        if release.assets.is_empty() {
            return Err(VendorError::ReleaseAssetsMissing(release_url));
        }

        let asset_id = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .map(|asset| asset.id)
            .ok_or_else(|| VendorError::ReleaseAssetNotFound {
                asset: asset_name.to_owned(),
                url: release_url.clone(),
            })?;

        let url = format!(
            "{API_ROOT}/repos/{}/{}/releases/assets/{asset_id}",
            repo.owner, repo.name
        );
        self.send(url, "application/octet-stream")
            .await
            .map_err(|_| VendorError::ReleaseAssetDownloadFailed {
                asset: asset_name.to_owned(),
                url: release_url,
            })
    }

    /// Resolves a repository name to its URL via code search.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::NoSearchResults`] when the search is empty, or
    /// [`VendorError::NoSearchResultsDidYouMean`] when the best hit has a different name.
    pub async fn find_repo_url(&self, name: &str) -> Result<String> {
        self.warn_if_anonymous();
        let page = self
            .api
            .search()
            .repositories(name)
            .per_page(1)
            .send()
            .await
            .map_err(VendorError::from)?;

        let item = page
            .items
            .into_iter()
            .next()
            .ok_or_else(|| VendorError::NoSearchResults(name.to_owned()))?;

        if item.name.to_lowercase() != name.to_lowercase() {
            return Err(VendorError::NoSearchResultsDidYouMean {
                name: name.to_owned(),
                suggestion: item.name,
            });
        }
        Ok(item.html_url.map_or_else(
            || format!("https://github.com/{}", item.full_name.unwrap_or(item.name)),
            |url| url.to_string(),
        ))
    }

    /// Issues an authenticated GET and rejects non-2xx responses.
    async fn send(&self, url: String, accept: &str) -> Result<reqwest::Response> {
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token.expose()),
            );
        }
        let response = request
            .send()
            .await
            .map_err(|e| VendorError::Http(e.to_string()))?;
        if !response.status().is_success() {
            return Err(VendorError::RequestFailed(response.status().as_u16()));
        }
        Ok(response)
    }
}

/// Percent-encodes a path, keeping `/` separators intact.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_query)
        .collect::<Vec<_>>()
        .join("/")
}

/// Percent-encodes a single URL component.
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{encode_path, encode_query};

    #[test]
    fn path_encoding_keeps_separators() {
        assert_eq!(encode_path("dist/coloris.min.js"), "dist/coloris.min.js");
        assert_eq!(encode_path("a b/c#d"), "a%20b/c%23d");
    }

    #[test]
    fn query_encoding_escapes_everything_unsafe() {
        assert_eq!(encode_query("v1.0.0"), "v1.0.0");
        assert_eq!(encode_query("feature/x"), "feature%2Fx");
    }
}
