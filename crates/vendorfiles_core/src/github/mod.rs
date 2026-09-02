//! The GitHub API surface the tool needs, plus streaming downloads.

pub mod auth;
pub mod credentials;
pub mod http;

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, Once};

use tokio::sync::OnceCell;

use indexmap::IndexMap;
use octocrab::Octocrab;
use octocrab::models::repos::Release;
use serde::Deserialize;

use crate::error::{Result, VendorError};
use crate::model::Repository;
use crate::remote_zip::HttpRangeSource;
use crate::ui;

pub use auth::Token;
pub use http::USER_AGENT;

const API_ROOT: &str = "https://api.github.com";

/// An asset whose storage will serve byte ranges: where to ask, and how much there is.
#[derive(Debug, Clone)]
pub struct AssetRange {
    pub url: String,
    pub size: u64,
}

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

/// A release lookup that resolves at most once, however many callers await it.
type ReleaseSlot = Arc<OnceCell<Arc<Release>>>;

/// Client for everything the tool does against github.com.
///
/// Releases are cached for the process lifetime so two dependencies tracking the same
/// repository cost one request, as in the reference. The cache stores a
/// [`OnceCell`] per key rather than a value, so concurrent lookups of the same release
/// collapse into a single request instead of racing - which matters because the anonymous
/// rate limit is 60 requests an hour.
pub struct GitHubClient {
    api: Octocrab,
    http: reqwest::Client,
    token: Option<Token>,
    releases: Mutex<IndexMap<ReleaseKey, ReleaseSlot>>,
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

        Ok(Self {
            api,
            http: http::client()?,
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

    /// The slot for `key`, creating an empty one if this is the first request for it.
    fn slot(&self, key: ReleaseKey) -> ReleaseSlot {
        // A poisoned mutex only costs us the cache, never correctness.
        self.releases.lock().map_or_else(
            |_| ReleaseSlot::default(),
            |mut releases| releases.entry(key).or_default().clone(),
        )
    }

    /// An already-resolved release for `repo` whose tag is `tag`, if one was fetched earlier.
    ///
    /// This is the reference's `startsWith` scan: a `getLatestRelease` result serves a later
    /// lookup by tag when the tags happen to agree.
    fn resolved_by_tag(&self, repo_key: &str, tag: &str) -> Option<Arc<Release>> {
        let releases = self.releases.lock().ok()?;
        releases.iter().find_map(|(key, slot)| {
            let release = slot.get()?;
            (key.repo() == repo_key && release.tag_name == tag).then(|| release.clone())
        })
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
        if let Some(release) = self.resolved_by_tag(&repo_key, tag) {
            return Ok(release);
        }

        let slot = self.slot(ReleaseKey::Tag {
            repo: repo_key,
            tag: tag.to_owned(),
        });
        slot.get_or_try_init(|| async {
            self.warn_if_anonymous();
            self.api
                .repos(&repo.owner, &repo.name)
                .releases()
                .get_by_tag(tag)
                .await
                .map(Arc::new)
                .map_err(VendorError::from)
        })
        .await
        .cloned()
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
        let slot = self.slot(ReleaseKey::Latest {
            repo: repo.to_string(),
        });
        slot.get_or_try_init(|| async {
            self.warn_if_anonymous();
            self.api
                .repos(&repo.owner, &repo.name)
                .releases()
                .get_latest()
                .await
                .map(Arc::new)
                .map_err(VendorError::from)
        })
        .await
        .cloned()
    }

    /// The first release (newest first) whose tag or title matches `release_regex`.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::NoMatchingRelease`] when nothing matches, or
    /// [`VendorError::RequestFailed`] if the release list cannot be fetched.
    pub async fn release_by_regex(&self, repo: &Repository, regex: &str) -> Result<Arc<Release>> {
        let slot = self.slot(ReleaseKey::Regex {
            repo: repo.to_string(),
            regex: regex.to_owned(),
        });
        slot.get_or_try_init(|| async {
            self.warn_if_anonymous();
            // `fancy_regex` accepts the JavaScript patterns users already have, lookaround
            // included.
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
            page.items
                .into_iter()
                .find(|release| {
                    matches(&release.tag_name) || matches(release.name.as_deref().unwrap_or(""))
                })
                .map(Arc::new)
                .ok_or_else(|| VendorError::NoMatchingRelease {
                    regex: regex.to_owned(),
                    owner: repo.owner.clone(),
                    repo: repo.name.clone(),
                })
        })
        .await
        .cloned()
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
        let (url, release_url, _) = self
            .release_asset_url(repo, asset_name, version, release_regex)
            .await?;
        self.send(url, "application/octet-stream")
            .await
            .map_err(|_| VendorError::ReleaseAssetDownloadFailed {
                asset: asset_name.to_owned(),
                url: release_url,
            })
    }

    /// The API endpoint for a named release asset, the release it belongs to, and its size.
    ///
    /// Split out so a download and a range probe agree on which asset they mean, and so
    /// resolving one costs a single release lookup either way. The size comes from the release
    /// JSON, which is already in hand - so a decision that turns on how big an asset is need
    /// not ask the network how big it is.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::ReleaseNotFound`], [`VendorError::ReleaseAssetsMissing`] or
    /// [`VendorError::ReleaseAssetNotFound`] depending on which step fails.
    async fn release_asset_url(
        &self,
        repo: &Repository,
        asset_name: &str,
        version: &str,
        release_regex: Option<&str>,
    ) -> Result<(String, String, u64)> {
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

        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| VendorError::ReleaseAssetNotFound {
                asset: asset_name.to_owned(),
                url: release_url.clone(),
            })?;
        let asset_id = asset.id;

        Ok((
            format!(
                "{API_ROOT}/repos/{}/{}/releases/assets/{asset_id}",
                repo.owner, repo.name
            ),
            release_url,
            u64::try_from(asset.size).unwrap_or(0),
        ))
    }

    /// Where an asset's bytes can be range-fetched from, when its storage will serve ranges.
    ///
    /// The API's asset endpoint redirects to a signed URL on GitHub's release storage, and it is
    /// *that* URL the ranges go to rather than the endpoint itself. Two reasons, both of which
    /// would otherwise sink the idea:
    ///
    /// - Every request through `api.github.com` counts against the rate limit, which is 60 an
    ///   hour without a token. Reading an asset in four ranges instead of downloading it once
    ///   would cost four times the quota to save bandwidth, which is the wrong trade.
    /// - The signed URL carries its own authorisation and rejects a request that also bears a
    ///   bearer token. Nothing is sent to it: reqwest drops the `Authorization` header when a
    ///   redirect crosses to another host, which is exactly what is wanted here.
    ///
    /// An asset smaller than `minimum` is refused before anything is sent: the release JSON
    /// already records every asset's size, so an asset too small to be worth ranging costs no
    /// request at all to rule out.
    ///
    /// `None` means ranges are not on offer - too small, no `Accept-Ranges`, no length to read
    /// the index against - and the caller should download the asset instead. An error means the
    /// asset could not be resolved at all, which is the caller's problem either way.
    ///
    /// # Errors
    ///
    /// Returns whatever [`release_asset_url`](Self::release_asset_url) produced when the asset
    /// cannot be resolved.
    pub async fn asset_range_source(
        &self,
        repo: &Repository,
        asset_name: &str,
        version: &str,
        release_regex: Option<&str>,
        minimum: u64,
    ) -> Result<Option<AssetRange>> {
        let (url, _, recorded) = self
            .release_asset_url(repo, asset_name, version, release_regex)
            .await?;
        if recorded < minimum {
            return Ok(None);
        }

        let mut request = self
            .http
            .head(url)
            .header(reqwest::header::ACCEPT, "application/octet-stream")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token.expose()),
            );
        }
        let Ok(response) = request.send().await else {
            return Ok(None);
        };
        if !response.status().is_success() {
            return Ok(None);
        }

        let serves_ranges = response
            .headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("bytes"));
        let size = response.content_length().unwrap_or(0);
        if !serves_ranges || size < minimum {
            return Ok(None);
        }

        Ok(Some(AssetRange {
            url: response.url().to_string(),
            size,
        }))
    }

    /// A range source over `asset` on this client's HTTP stack.
    ///
    /// Built here so the client's `reqwest` handle - its user agent, its TLS, its proxy settings
    /// - stays the one thing that talks to the network, without being handed out.
    #[must_use]
    pub fn range_source(
        &self,
        asset: AssetRange,
        arrivals: Option<tokio::sync::mpsc::UnboundedSender<u64>>,
    ) -> HttpRangeSource {
        HttpRangeSource::new(
            self.http.clone(),
            asset.url,
            asset.size,
            tokio::runtime::Handle::current(),
            arrivals,
        )
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
