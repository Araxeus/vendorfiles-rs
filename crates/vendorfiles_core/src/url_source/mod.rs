//! Versions and requests for dependencies that download from a plain URL.
//!
//! A `versionUrl` ending in `.git` names a git repository, and its latest tag is the version.
//! Any other `versionUrl` is a page whose body, selected element or regex match is the version.
//!
//! Every request here goes through the plain HTTP client, so a GitHub token never reaches
//! another host.

pub mod page;
pub mod tags;

use std::time::Duration;

use reqwest::header::CONTENT_TYPE;

use crate::error::{Result, VendorError};

/// A protocol v2 `ls-refs` request for tags alone.
const LS_TAGS: &[u8] = b"0014command=ls-refs\n0001001aref-prefix refs/tags/\n0000";

/// The content type of a protocol v2 answer.
const LS_TAGS_RESULT: &str = "application/x-git-upload-pack-result";

/// The content type of a full ref advertisement. Anything else at `info/refs` is a host that
/// does not speak git, or a login page.
const ADVERTISEMENT: &str = "application/x-git-upload-pack-advertisement";

/// How long a whole version lookup may take, every request included. A host that hangs would
/// otherwise stall the whole run.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(30);

/// How much of a version page is read. A version sits near the top of anything sane.
const MAX_VERSION_BODY: usize = 1 << 20;

/// How much of a tag listing is read. The full advertisement of a large repository runs to tens
/// of megabytes, and a host that never stops sending is cut off here.
const MAX_REFS_BODY: usize = 64 << 20;

/// The longest version accepted.
const MAX_VERSION_LEN: usize = 128;

/// Where a URL dependency downloads from and how its version is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlSource {
    pub url: String,
    pub version_url: Option<String>,
    pub version_regex: Option<String>,
    pub version_selector: Option<String>,
}

/// Whether `version_url` names a git repository, read for its tags.
#[must_use]
pub fn is_git_url(version_url: &str) -> bool {
    let url = version_url.trim_end_matches('/');
    url.get(url.len().saturating_sub(4)..)
        .is_some_and(|end| end.eq_ignore_ascii_case(".git"))
}

/// The latest version of `source`, or `None` when it has no `versionUrl`.
///
/// # Errors
///
/// [`VendorError::NoVersionAt`] when the answer holds no version or is a 404,
/// [`VendorError::RequestFailed`] for any other refusal, and [`VendorError::Http`] when the host
/// cannot be reached or takes longer than [`LOOKUP_TIMEOUT`].
pub async fn latest_version(http: &reqwest::Client, source: &UrlSource) -> Result<Option<String>> {
    let Some(version_url) = source.version_url.as_deref() else {
        return Ok(None);
    };
    tokio::time::timeout(LOOKUP_TIMEOUT, look_up(http, source, version_url))
        .await
        .unwrap_or_else(|_| {
            Err(VendorError::Http(format!(
                "looking up the version at {version_url} took over {} seconds",
                LOOKUP_TIMEOUT.as_secs()
            )))
        })
}

/// [`latest_version`] without its deadline.
async fn look_up(
    http: &reqwest::Client,
    source: &UrlSource,
    version_url: &str,
) -> Result<Option<String>> {
    let miss = || VendorError::NoVersionAt {
        url: version_url.to_owned(),
    };
    // A 404 is an answer: nothing is there. Any other refusal, like a 503, says nothing about
    // versions, so it is reported as itself.
    let refused = |e| match e {
        VendorError::RequestFailed { status: 404, .. } => miss(),
        other => other,
    };
    // Validation already compiled both, so a failure here cannot happen in practice.
    let regex = source
        .version_regex
        .as_deref()
        .map(fancy_regex::Regex::new)
        .transpose()
        .map_err(|_| miss())?;

    let found = if is_git_url(version_url) {
        let mut tags = git_tags(http, version_url)
            .await
            .map_err(refused)?
            .ok_or_else(miss)?;
        tags.retain(|tag| is_plausible_version(tag));
        let latest = tags::latest_tag(&tags, regex.as_ref());
        // A monorepo tags only its packages, which count only when a regex picks one.
        if latest.is_none()
            && regex.is_none()
            && let Some(example) = tags
                .first()
                .filter(|_| tags.iter().all(|t| t.contains('@')))
        {
            return Err(VendorError::OnlyPackageTags {
                url: version_url.to_owned(),
                example: example.clone(),
            });
        }
        latest
    } else {
        let response = send(http.get(version_url)).await.map_err(refused)?;

        // A redirect can carry the version in its target, like `/latest` to `/v1.2.3/tool.zip`.
        // Without one, the URL is the configured `versionUrl` and says nothing new.
        let redirected = reqwest::Url::parse(version_url).ok().as_ref() != Some(response.url());
        if redirected
            && source.version_selector.is_none()
            && let Some(version) = regex
                .as_ref()
                .and_then(|re| page::extract_version(response.url().as_str(), Some(re)))
                .filter(|version| is_plausible_version(version))
        {
            return Ok(Some(version));
        }

        let body = read_capped(response, MAX_VERSION_BODY).await?;
        let body = String::from_utf8_lossy(&body);
        let selector = source
            .version_selector
            .as_deref()
            .map(scraper::Selector::parse)
            .transpose()
            .map_err(|_| miss())?;
        page::find_version(&body, selector.as_ref(), regex.as_ref())
    };
    found
        .filter(|version| is_plausible_version(version))
        .map(Some)
        .ok_or_else(miss)
}

/// Whether `version` can be one: letters, digits and `.`, `_`, `+`, `-`, `@`, starting with a
/// letter or digit, short, and holding a digit unless it is a full 40-character commit hash.
///
/// A version lands in file names through `{tag}`, and a remote host chooses it. So a path like
/// `../x`, a JSON body or a stray word like `Forbidden` is refused rather than used. `@` is in for
/// monorepo tags like `astro@5.0.0`.
fn is_plausible_version(version: &str) -> bool {
    let commit = version.len() == 40 && version.chars().all(|c| c.is_ascii_hexdigit());
    version.len() <= MAX_VERSION_LEN
        && version.starts_with(|c: char| c.is_ascii_alphanumeric())
        && (commit || version.contains(|c: char| c.is_ascii_digit()))
        && !version.contains("..")
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '@'))
}

/// The tag names of the git repository at `repository`, or `None` when no git host answers.
///
/// Protocol v2 is asked for tags alone first. The older full advertisement lists every branch
/// and merge request too, which runs to tens of megabytes on a large repository, so it is only
/// the fallback for a host that does not speak v2.
async fn git_tags(http: &reqwest::Client, repository: &str) -> Result<Option<Vec<String>>> {
    let repository = repository.trim_end_matches('/');
    let listing = http
        .post(format!("{repository}/git-upload-pack"))
        .header("Git-Protocol", "version=2")
        .header(CONTENT_TYPE, "application/x-git-upload-pack-request")
        .body(LS_TAGS);
    if let Ok(response) = send(listing).await
        && has_type(&response, LS_TAGS_RESULT)
    {
        let tags = tags::parse_advertisement(&read_capped(response, MAX_REFS_BODY).await?);
        if !tags.is_empty() {
            return Ok(Some(tags));
        }
    }

    let advertisement = http.get(format!("{repository}/info/refs?service=git-upload-pack"));
    let response = send(advertisement).await?;
    if !has_type(&response, ADVERTISEMENT) {
        return Ok(None);
    }
    Ok(Some(tags::parse_advertisement(
        &read_capped(response, MAX_REFS_BODY).await?,
    )))
}

/// Whether `response` declares the content type `expected`.
fn has_type(response: &reqwest::Response, expected: &str) -> bool {
    response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|t| t.as_bytes().starts_with(expected.as_bytes()))
}

/// Sends a GET to `url`, following redirects.
///
/// # Errors
///
/// [`VendorError::RequestFailed`] for a non-2xx answer, and [`VendorError::Http`] when the host
/// cannot be reached.
pub async fn fetch(http: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    send(http.get(url)).await
}

/// Sends `request`, turning a non-2xx answer into [`VendorError::RequestFailed`].
async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let response = request.send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(VendorError::RequestFailed {
        status: status.as_u16(),
        message: status.canonical_reason().unwrap_or_default().to_owned(),
    })
}

/// The first `limit` bytes of the body.
async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    while body.len() < limit
        && let Some(chunk) = response.chunk().await?
    {
        body.extend_from_slice(&chunk);
    }
    body.truncate(limit);
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::{LOOKUP_TIMEOUT, UrlSource, fetch, latest_version, tags};
    use crate::error::VendorError;
    use crate::github::http::client;
    use crate::test_server::{route, serve, serve_silence};

    #[tokio::test]
    async fn a_git_url_resolves_to_its_latest_tag() {
        let base = serve(vec![route(
            "/o/r.git/info/refs?service=git-upload-pack",
            200,
            "Content-Type: application/x-git-upload-pack-advertisement\r\n",
            &tags::tests::advertisement(&["refs/tags/v0.9.0", "refs/tags/v1.0.0"]),
        )]);
        let source = UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/o/r.git/")),
            version_regex: None,
            version_selector: None,
        };
        assert_eq!(
            latest_version(&client().unwrap(), &source)
                .await
                .unwrap()
                .as_deref(),
            Some("v1.0.0")
        );
    }

    #[tokio::test]
    async fn a_git_host_speaking_protocol_v2_lists_only_its_tags() {
        // No `info/refs` route: the full advertisement must not be needed.
        let base = serve(vec![route(
            "/o/r.git/git-upload-pack",
            200,
            "Content-Type: application/x-git-upload-pack-result\r\n",
            &tags::tests::ls_refs(&["refs/tags/v1.0.0", "refs/tags/v1.2.0"]),
        )]);
        let source = UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/o/r.git")),
            version_regex: None,
            version_selector: None,
        };
        assert_eq!(
            latest_version(&client().unwrap(), &source)
                .await
                .unwrap()
                .as_deref(),
            Some("v1.2.0")
        );
    }

    #[tokio::test]
    async fn a_redirect_target_is_matched_before_the_body() {
        let base = serve(vec![
            route("/latest", 302, "Location: /files/tool-v4.5.6.zip\r\n", b""),
            route("/files/tool-v4.5.6.zip", 200, "", b"PK\x03\x04 binary"),
        ]);
        let source = UrlSource {
            url: base.clone(),
            version_url: Some(format!("{base}/latest")),
            version_regex: Some(r"v(\d+\.\d+\.\d+)".into()),
            version_selector: None,
        };
        assert_eq!(
            latest_version(&client().unwrap(), &source)
                .await
                .unwrap()
                .as_deref(),
            Some("4.5.6")
        );
    }

    #[tokio::test]
    async fn no_version_url_means_no_lookup_and_a_miss_is_an_error() {
        let http = client().unwrap();
        let none = UrlSource {
            url: "https://x.io".into(),
            version_url: None,
            version_regex: None,
            version_selector: None,
        };
        assert_eq!(latest_version(&http, &none).await.unwrap(), None);

        let base = serve(vec![route("/v", 200, "", b"<html>\n</html>")]);
        let page = UrlSource {
            version_url: Some(format!("{base}/v")),
            ..none
        };
        let error = latest_version(&http, &page).await.unwrap_err();
        assert_eq!(error.to_string(), format!("found no version at {base}/v"));
    }

    #[tokio::test]
    async fn a_selector_reads_a_served_page() {
        let page = br#"<h1>Changes since 2019</h1><h2 id="latest">v2.1.0</h2>"#;
        let base = serve(vec![route(
            "/changelog",
            200,
            "Content-Type: text/html\r\n",
            page,
        )]);
        let source = UrlSource {
            url: base.clone(),
            version_url: Some(format!("{base}/changelog")),
            version_regex: None,
            version_selector: Some("#latest".into()),
        };
        assert_eq!(
            latest_version(&client().unwrap(), &source)
                .await
                .unwrap()
                .as_deref(),
            Some("v2.1.0")
        );
    }

    #[tokio::test]
    async fn a_git_url_that_is_not_a_git_host_has_no_version() {
        let base = serve(vec![route(
            "/login.git/info/refs?service=git-upload-pack",
            200,
            "Content-Type: text/html\r\n",
            b"<html>sign in</html>",
        )]);
        let http = client().unwrap();
        for path in ["login.git", "gone.git"] {
            let source = UrlSource {
                url: "https://x.io".into(),
                version_url: Some(format!("{base}/{path}")),
                version_regex: None,
                version_selector: None,
            };
            let error = latest_version(&http, &source).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("found no version at {base}/{path}")
            );
        }
    }

    #[test]
    fn a_version_lookup_can_run_on_any_thread() {
        // `scraper`'s parsed page is not `Send`; it must never be held across an `.await`.
        fn send<T: Send>(_: T) {}
        let http = client().unwrap();
        let source = UrlSource {
            url: String::new(),
            version_url: None,
            version_regex: None,
            version_selector: None,
        };
        send(latest_version(&http, &source));
    }

    #[tokio::test]
    async fn a_version_url_that_refuses_reports_why() {
        let base = serve(vec![route("/busy", 503, "", b"")]);
        let source = UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/busy")),
            version_regex: None,
            version_selector: None,
        };
        let error = latest_version(&client().unwrap(), &source)
            .await
            .unwrap_err();
        assert!(
            matches!(error, VendorError::RequestFailed { status: 503, .. }),
            "{error}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_version_url_times_out() {
        // The clock is paused, so the wait costs nothing: tokio jumps to the next timer. One
        // deadline covers the whole lookup, including a git host's second request.
        let http = client().unwrap();
        for version_url in [
            format!("{}/v", serve_silence()),
            format!("{}/r.git", serve_silence()),
        ] {
            let source = UrlSource {
                url: "https://x.io".into(),
                version_url: Some(version_url),
                version_regex: None,
                version_selector: None,
            };
            let outcome = tokio::time::timeout(
                LOOKUP_TIMEOUT + LOOKUP_TIMEOUT / 10,
                latest_version(&http, &source),
            )
            .await;
            assert!(matches!(outcome, Ok(Err(_))), "{outcome:?}");
        }
    }

    #[tokio::test]
    async fn an_answer_that_cannot_be_a_version_is_refused() {
        // Whatever a host answers ends up in file names through `{tag}`, so it has to look like
        // a version: no path, no JSON, no word without a digit.
        let base = serve(vec![
            route("/climb", 200, "", b"../../../v1"),
            route("/json", 200, "", br#"{"tag_name":"v1.2.3"}"#),
            route("/word", 200, "", b"Forbidden"),
            route("/fine", 200, "", b"v1.2.3+build.4"),
        ]);
        let http = client().unwrap();
        let lookup = |path: &str| UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/{path}")),
            version_regex: None,
            version_selector: None,
        };
        for path in ["climb", "json", "word"] {
            let error = latest_version(&http, &lookup(path)).await.unwrap_err();
            assert!(
                matches!(error, VendorError::NoVersionAt { .. }),
                "{path}: {error}"
            );
        }
        assert_eq!(
            latest_version(&http, &lookup("fine"))
                .await
                .unwrap()
                .as_deref(),
            Some("v1.2.3+build.4")
        );
    }

    #[tokio::test]
    async fn a_redirect_that_cannot_be_a_version_is_refused() {
        let base = serve(vec![
            route("/latest", 302, "Location: /dl/x?v=../../v1\r\n", b""),
            route("/dl/x?v=../../v1", 200, "", b"<html>\n</html>"),
        ]);
        let source = UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/latest")),
            version_regex: Some(r"[?&]v=([^&]+)".into()),
            version_selector: None,
        };
        let error = latest_version(&client().unwrap(), &source)
            .await
            .unwrap_err();
        assert!(matches!(error, VendorError::NoVersionAt { .. }), "{error}");
    }

    #[test]
    fn versions_people_use_are_plausible() {
        for version in [
            "v1.2.3",
            "1.2.3-rc.1",
            "2024.01.15",
            "release-1.0",
            "curl-8_10_0",
            "v1.2.3+build.4",
            "astro@5.0.0",
            "f6ec482ea395cead4fd849c05df6edd8da284a52",
            "abcdefabcdefabcdefabcdefabcdefabcdefabcd",
        ] {
            assert!(super::is_plausible_version(version), "{version}");
        }
        for version in ["../v1", "Forbidden", "", "a/1", r#"{"v":1}"#, "defaced"] {
            assert!(!super::is_plausible_version(version), "{version}");
        }
    }

    #[tokio::test]
    async fn a_repo_of_package_tags_says_to_set_a_regex() {
        let base = serve(vec![route(
            "/o/r.git/git-upload-pack",
            200,
            "Content-Type: application/x-git-upload-pack-result
",
            &tags::tests::ls_refs(&["refs/tags/astro@5.0.0", "refs/tags/react@4.0.0"]),
        )]);
        let source = UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/o/r.git")),
            version_regex: None,
            version_selector: None,
        };
        let error = latest_version(&client().unwrap(), &source)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "{base}/o/r.git only has package tags like 'astro@5.0.0'; set versionRegex to pick one"
            )
        );
    }

    #[tokio::test]
    async fn a_git_tag_that_cannot_be_a_version_is_skipped() {
        let base = serve(vec![route(
            "/o/r.git/git-upload-pack",
            200,
            "Content-Type: application/x-git-upload-pack-result\r\n",
            &tags::tests::ls_refs(&["refs/tags/v1.0.0", "refs/tags/../../v9.0.0"]),
        )]);
        let source = UrlSource {
            url: "https://x.io".into(),
            version_url: Some(format!("{base}/o/r.git")),
            version_regex: None,
            version_selector: None,
        };
        assert_eq!(
            latest_version(&client().unwrap(), &source)
                .await
                .unwrap()
                .as_deref(),
            Some("v1.0.0")
        );
    }

    #[tokio::test]
    async fn fetch_reports_the_status_of_a_refusal() {
        let base = serve(vec![]);
        let error = fetch(&client().unwrap(), &format!("{base}/missing"))
            .await
            .unwrap_err();
        assert!(
            matches!(error, VendorError::RequestFailed { status: 404, .. }),
            "{error}"
        );
    }
}
