//! The GitHub API surface the tool needs, plus streaming downloads.

pub mod auth;
pub mod credentials;
pub mod http;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::OnceCell;

use indexmap::IndexMap;
use octocrab::Octocrab;
use octocrab::models::repos::Release;
use serde::Deserialize;

use crate::error::{Result, VendorError, is_rate_limited, one_line};
use crate::model::Repository;
use crate::remote_zip::HttpRangeSource;
use crate::ui;

pub use auth::Token;
pub use http::USER_AGENT;

const API_ROOT: &str = "https://api.github.com";

/// The quota below which a run is worth mentioning.
///
/// Anonymous requests get 60 an hour and resolving a dependency costs about one, so with two
/// thirds of the budget still there, saying anything would be noise. Below this it stops being
/// theoretical.
const LOW_QUOTA: u64 = 40;

/// A reading of the core rate limit.
#[derive(Debug, Clone, Copy, Deserialize)]
struct Quota {
    limit: u64,
    remaining: u64,
    /// When the window rolls over, as a Unix timestamp.
    reset: u64,
}

#[derive(Deserialize)]
struct RateLimitBody {
    resources: RateLimitResources,
}

#[derive(Deserialize)]
struct RateLimitResources {
    core: Quota,
}

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
    /// The quota lookup of an anonymous run, from the first request onwards.
    quota: Mutex<Option<tokio::task::JoinHandle<Option<Quota>>>>,
    /// Requests charged to the core limit since that lookup was started.
    spent: AtomicU64,
    probing: Once,
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
            quota: Mutex::new(None),
            spent: AtomicU64::new(0),
            probing: Once::new(),
        })
    }

    /// Notes a request charged to the core rate limit, and starts the quota lookup if this is
    /// the first of the run.
    fn note_api_request(&self) {
        self.spent.fetch_add(1, Ordering::Relaxed);
        self.begin_quota_lookup();
    }

    /// Notes a request billed somewhere other than the core limit.
    ///
    /// Search has a limit of its own, so counting one against the core budget would report a run
    /// as more expensive than it was.
    fn note_search_request(&self) {
        self.begin_quota_lookup();
    }

    /// Puts the quota lookup in flight, once per run, for an anonymous client only.
    ///
    /// Spawned rather than awaited: nothing needs the answer until the run is over, and waiting
    /// for it here would put a round trip in front of the first request that actually does work.
    ///
    /// Anonymous only for two reasons. It is the only limit that bites - a token gets 5000 an
    /// hour - and it is the only one `/rate_limit` reports truthfully: measured against a
    /// `gh`-issued token, that endpoint answered `0 of 5000 used` while the `x-ratelimit-used`
    /// header on real responses said 210. The headers are authoritative, but `octocrab`'s typed
    /// calls - the ones that spend the quota - deserialise the body and drop them.
    fn begin_quota_lookup(&self) {
        if self.token.is_some() {
            return;
        }
        self.probing.call_once(|| {
            let client = self.http.clone();
            let handle = tokio::spawn(async move { fetch_quota(&client).await });
            if let Ok(mut slot) = self.quota.lock() {
                *slot = Some(handle);
            }
        });
    }

    /// Silent unless there is something to say: a healthy budget, an authenticated run, a run
    /// that made no API requests at all, or a lookup that failed all say nothing. A diagnostic
    /// is never worth interrupting a run over.
    pub async fn report_quota(&self) {
        if let Some(warning) = self.quota_report().await {
            ui::warning(warning);
        }
    }

    /// What [`report_quota`](Self::report_quota) would say, so the decision can be tested
    /// against a live reading rather than only against numbers made up here.
    async fn quota_report(&self) -> Option<String> {
        let lookup = self.quota.lock().ok().and_then(|mut slot| slot.take())?;
        let quota = lookup.await.ok()??;
        // The reading was taken alongside the run's first request, so what has been spent since
        // is everything counted after that one - give or take the one that raced the lookup.
        let spent = self.spent.load(Ordering::Relaxed).saturating_sub(1);
        quota_warning(&quota, spent, now())
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
            self.note_api_request();
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
            self.note_api_request();
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
            self.note_api_request();
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

        self.note_api_request();
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

    /// Whether the repository exists and the token in use can see it.
    ///
    /// `None` when the question could not be answered - a refused token, an exhausted quota, a
    /// `503`. Only a `404` is an answer, for the reason §6.18 exists: reporting "no such
    /// repository" because a request failed would be the same mistake in a new place.
    ///
    /// Asked only once a download has already failed, so it costs one request on a path that is
    /// failing anyway and nothing at all on the ordinary one.
    pub async fn repository_exists(&self, repo: &Repository) -> Option<bool> {
        self.note_api_request();
        match self.api.repos(&repo.owner, &repo.name).get().await {
            Ok(_) => Some(true),
            Err(error) => match VendorError::from(error) {
                VendorError::RequestFailed { status: 404, .. } => Some(false),
                _ => None,
            },
        }
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
        self.note_api_request();
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
            .map_err(|error| {
                unless_the_request_failed(error, || VendorError::ReleaseAssetDownloadFailed {
                    asset: asset_name.to_owned(),
                    url: release_url,
                })
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
            self.release_by_tag(repo, version).await.map_err(|error| {
                unless_the_request_failed(error, || VendorError::ReleaseNotFound {
                    version: version.to_owned(),
                    owner: repo.owner.clone(),
                    repo: repo.name.clone(),
                })
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
        self.note_search_request();
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
            let status = response.status().as_u16();
            // Headers first, and on their own when they settle it: an exhausted primary limit
            // says so in `x-ratelimit-remaining`, and this route reads its own responses, so
            // unlike the `octocrab` one it can say how much was left and when it comes back.
            // Returning here also leaves the body unread, which is the point - reading it would
            // consume the response for a message nothing goes on to use.
            if let Some(detail) = rate_limit_detail(&response) {
                return Err(VendorError::RateLimited(detail));
            }
            let message = error_message(response).await;
            // A *secondary* limit is a `403` or `429` that leaves the remaining count alone, so
            // the headers above cannot see it and only the wording gives it away. Now that the
            // body is in hand, the same test the `octocrab` route uses catches those here too.
            if is_rate_limited(status, &message) {
                return Err(VendorError::RateLimited(String::new()));
            }
            if status == 401 {
                return Err(VendorError::BadCredentials);
            }
            return Err(VendorError::RequestFailed { status, message });
        }
        Ok(response)
    }
}

/// Rewrites a failure as the friendlier one it was written for, but only when GitHub actually
/// answered that the thing is not there.
///
/// The lookups here translate their failures into something a user can act on - "release not
/// found", "could not download the asset". That reading is right for a `404` and wrong for
/// everything else: a `401` means the credentials were refused, a `429` means the quota ran out,
/// a `503` means GitHub is unwell. Reported as "release not found", any of those sends the
/// reader off to check a tag that is perfectly fine - which is exactly what a bad `GITHUB_TOKEN`
/// used to do.
///
/// So the test is the status, not a list of errors worth protecting: only absence is rewritten,
/// and every other failure is reported as itself.
fn unless_the_request_failed(
    error: VendorError,
    absent: impl FnOnce() -> VendorError,
) -> VendorError {
    match error {
        VendorError::RequestFailed { status: 404, .. } => absent(),
        other => other,
    }
}

/// Asks GitHub what is left of the core limit.
///
/// Unauthenticated on purpose - see [`GitHubClient::begin_quota_lookup`] - and free: this
/// endpoint does not itself count against the limit it reports.
async fn fetch_quota(client: &reqwest::Client) -> Option<Quota> {
    let response = client
        .get(format!("{API_ROOT}/rate_limit"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    // Parsed from the bytes rather than through `Response::json`: reqwest is built without its
    // `json` feature, and `serde_json` is already here.
    let body: RateLimitBody = serde_json::from_slice(&response.bytes().await.ok()?).ok()?;
    Some(body.resources.core)
}

/// Seconds since the Unix epoch, or zero if the clock is before it.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// What to tell the user about a quota reading, if anything.
///
/// `spent` is what the run has charged since the reading was taken, which is what makes the
/// figure current rather than a snapshot from the start of the run.
fn quota_warning(quota: &Quota, spent: u64, now: u64) -> Option<String> {
    // A window that has already rolled over makes the reading meaningless - and generous, since
    // the budget is back to full.
    if quota.reset <= now {
        return None;
    }
    let left = quota.remaining.saturating_sub(spent);
    if left >= LOW_QUOTA {
        return None;
    }
    Some(format!(
        "{left} of {} GitHub API requests left this hour ({}). Run `vendor login` or use a \
         GITHUB_TOKEN env variable to raise the limit to 5000",
        quota.limit,
        resets_in(quota.reset, now)
    ))
}

/// How long until a limit window rolls over, in words.
fn resets_in(reset: u64, now: u64) -> String {
    let seconds = reset.saturating_sub(now);
    if seconds < 60 {
        return "resets in under a minute".to_owned();
    }
    format!("resets in {} min", seconds / 60)
}

/// How much of an error body is read before it is given up on.
///
/// Generous next to any refusal GitHub actually sends, which is why exceeding it is taken as
/// evidence that whatever answered was not GitHub.
const MAX_ERROR_BODY: usize = 64 * 1024;

/// What GitHub said about a refusal, as one short line, or nothing if it said nothing usable.
///
/// Refusals come back as `{"message": ..., "documentation_url": ...}`, and the message is the half
/// worth showing - it names the problem the status code only numbers. Anything else yields an
/// empty string rather than a wall of markup: a proxy's HTML error page, a truncated body, a `502`
/// from something that is not GitHub at all.
async fn error_message(mut response: reqwest::Response) -> String {
    // Read a chunk at a time up to the cap rather than whole: a refusal from GitHub is a couple
    // of hundred bytes, and a body vastly larger than that is not one - an HTML error page from
    // something in between, most likely - so there is no reason to hold all of it in memory to
    // look for a `message` it does not have.
    let mut body: Vec<u8> = Vec::new();
    while body.len() < MAX_ERROR_BODY {
        let Ok(Some(chunk)) = response.chunk().await else {
            break;
        };
        let room = MAX_ERROR_BODY - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
    }
    // Parsed from the bytes rather than through `Response::json`, which reqwest is built without -
    // the same reason `fetch_quota` does it by hand. A body cut off at the cap will not parse,
    // which lands on the same empty string as any other unusable one.
    serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .as_ref()
        .and_then(|body| body.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(one_line)
        .unwrap_or_default()
}

/// The specifics for a [`VendorError::RateLimited`], if that is what a refusal is.
///
/// The headers are what decide it: an exhausted limit answers with `x-ratelimit-remaining: 0`,
/// and a `403` without that is some other refusal entirely - a bad token, most likely.
fn rate_limit_detail(response: &reqwest::Response) -> Option<String> {
    // The status alone is checked here rather than through `is_rate_limited`: that one has to
    // read the body's wording because it is all the `octocrab` route gets, whereas the header
    // below is a stronger test than any message.
    if !matches!(response.status().as_u16(), 403 | 429) {
        return None;
    }
    let number =
        |name: &str| -> Option<u64> { response.headers().get(name)?.to_str().ok()?.parse().ok() };
    if number("x-ratelimit-remaining")? > 0 {
        return None;
    }
    Some(format!(
        " - 0 of {} left, {}",
        number("x-ratelimit-limit")?,
        resets_in(number("x-ratelimit-reset")?, now())
    ))
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
    use super::{
        LOW_QUOTA, Quota, VendorError, encode_path, encode_query, quota_warning, resets_in,
        unless_the_request_failed,
    };

    /// A reading with `remaining` left and a window an hour out from `now`.
    fn reading(remaining: u64) -> Quota {
        Quota {
            limit: 60,
            remaining,
            reset: 10_000 + 3600,
        }
    }

    #[test]
    fn only_a_404_is_rewritten_as_the_thing_not_being_there() {
        let absent = || VendorError::ReleaseNotFound {
            version: "v1.0.0".to_owned(),
            owner: "o".to_owned(),
            repo: "r".to_owned(),
        };

        // GitHub answered "no such release", so that is what the user is told.
        let missing = unless_the_request_failed(
            VendorError::RequestFailed {
                status: 404,
                message: "Not Found".to_owned(),
            },
            absent,
        );
        assert!(
            matches!(missing, VendorError::ReleaseNotFound { .. }),
            "{missing}"
        );

        // Everything else is a failed request and has to survive intact. A refused token
        // reported as a missing release is what sent the reader to check a tag that was fine.
        for failure in [
            VendorError::BadCredentials,
            VendorError::RateLimited(" - 0 of 60 left".to_owned()),
            VendorError::RequestFailed {
                status: 503,
                message: String::new(),
            },
            VendorError::Http("connection reset".to_owned()),
        ] {
            let rendered = failure.to_string();
            let kept = unless_the_request_failed(failure, absent);
            assert!(
                !matches!(kept, VendorError::ReleaseNotFound { .. }),
                "{rendered} was rewritten as a missing release"
            );
        }
    }

    #[test]
    fn a_healthy_budget_says_nothing() {
        assert!(quota_warning(&reading(60), 0, 10_000).is_none());
        // The threshold is a floor, not a trigger: exactly at it is still healthy.
        assert!(quota_warning(&reading(LOW_QUOTA), 0, 10_000).is_none());
    }

    #[test]
    fn a_low_budget_reports_what_is_left_and_how_to_raise_it() {
        // Pinned whole, the way the rest of the tool's wording is: this is what the user reads,
        // and it should only ever change on purpose.
        assert_eq!(
            quota_warning(&reading(LOW_QUOTA - 1), 0, 10_000).expect("a warning"),
            concat!(
                "39 of 60 GitHub API requests left this hour (resets in 60 min). ",
                "Run `vendor login` or use a GITHUB_TOKEN env variable to raise the limit to 5000"
            )
        );
    }

    #[test]
    fn what_the_run_has_spent_since_the_reading_comes_off_the_figure() {
        // 45 left at the start is healthy, but not after the run has spent 10 of them.
        assert!(quota_warning(&reading(45), 0, 10_000).is_none());
        let warning = quota_warning(&reading(45), 10, 10_000).expect("a warning");
        assert!(warning.starts_with("35 of 60"), "{warning}");
        // Spending more than the reading knew about cannot underflow into a huge number.
        let warning = quota_warning(&reading(45), 999, 10_000).expect("a warning");
        assert!(warning.starts_with("0 of 60"), "{warning}");
    }

    #[test]
    fn a_reading_from_a_window_that_has_since_rolled_over_is_not_reported() {
        // The budget is back to full, so the old figure would only mislead.
        let stale = Quota {
            limit: 60,
            remaining: 1,
            reset: 9_000,
        };
        assert!(quota_warning(&stale, 0, 10_000).is_none());
    }

    /// Proves the lookup against the shape GitHub actually answers with, rather than a fixture
    /// of it, and proves the anonymous path end to end: a spawned lookup, collected later, put
    /// through the same decision a real run uses.
    ///
    /// Costs no quota - `/rate_limit` does not count against the limit it reports - and is
    /// anonymous, which is the only way the client ever calls it.
    #[tokio::test]
    #[ignore = "queries the GitHub API"]
    async fn an_anonymous_run_reads_its_real_quota() {
        let github = crate::GitHubClient::new(None).expect("a client");
        // Stand in for a run that made one request, which is what starts the lookup.
        github.note_api_request();

        // A budget nobody has eaten into has nothing to report.
        assert!(
            github.quota_report().await.is_none(),
            "a healthy anonymous budget should stay quiet - rerun in an hour if this machine              has been hammering the API"
        );

        // And one that has been spent does. A second client, since the first consumed its
        // lookup; the spend is forced past the threshold rather than waited for.
        let github = crate::GitHubClient::new(None).expect("a client");
        github.note_api_request();
        github.spent.fetch_add(60, super::Ordering::Relaxed);
        let warning = github.quota_report().await.expect("a warning");
        assert!(
            warning.contains("of 60 GitHub API requests left"),
            "{warning}"
        );
        assert!(warning.contains("vendor login"), "{warning}");
    }

    #[test]
    fn reset_times_read_as_english() {
        assert_eq!(resets_in(10_030, 10_000), "resets in under a minute");
        assert_eq!(resets_in(10_060, 10_000), "resets in 1 min");
        assert_eq!(resets_in(10_800, 10_000), "resets in 13 min");
        // A window already past is not a negative duration.
        assert_eq!(resets_in(9_000, 10_000), "resets in under a minute");
    }

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
