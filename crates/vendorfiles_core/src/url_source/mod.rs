//! Versions and requests for dependencies that download from a plain URL.
//!
//! A `versionUrl` ending in `.git` names a git repository, and its latest tag is the version.
//! Any other `versionUrl` is a page whose body, selected element or regex match is the version.
//!
//! Every request here goes through the plain HTTP client, so a GitHub token never reaches
//! another host.

pub mod page;
pub mod tags;

use reqwest::header::CONTENT_TYPE;

use crate::error::{Result, VendorError};

/// The content type of a smart-HTTP ref advertisement. Anything else at `info/refs` is a host
/// that does not speak the protocol, or a login page.
const ADVERTISEMENT: &str = "application/x-git-upload-pack-advertisement";

/// How much of a version page is read. A version sits near the top of anything sane.
const MAX_VERSION_BODY: usize = 1 << 20;

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
/// [`VendorError::NoVersionAt`] when the answer holds no version or the host refuses, and
/// [`VendorError::Http`] when the host cannot be reached.
pub async fn latest_version(http: &reqwest::Client, source: &UrlSource) -> Result<Option<String>> {
    let Some(version_url) = source.version_url.as_deref() else {
        return Ok(None);
    };
    let miss = || VendorError::NoVersionAt {
        url: version_url.to_owned(),
    };
    let refused = |e| match e {
        VendorError::RequestFailed { .. } => miss(),
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
        let refs = format!(
            "{}/info/refs?service=git-upload-pack",
            version_url.trim_end_matches('/')
        );
        let response = fetch(http, &refs).await.map_err(refused)?;
        let advertised = response
            .headers()
            .get(CONTENT_TYPE)
            .is_some_and(|t| t.as_bytes().starts_with(ADVERTISEMENT.as_bytes()));
        if !advertised {
            return Err(miss());
        }
        let body = response.bytes().await?;
        tags::latest_tag(&tags::parse_advertisement(&body), regex.as_ref())
    } else {
        let response = fetch(http, version_url).await.map_err(refused)?;

        // A redirect can carry the version in its target, like `/latest` to `/v1.2.3/tool.zip`.
        // Without one, the URL is the configured `versionUrl` and says nothing new.
        let redirected = reqwest::Url::parse(version_url).ok().as_ref() != Some(response.url());
        if redirected
            && source.version_selector.is_none()
            && let Some(version) = regex
                .as_ref()
                .and_then(|re| page::extract_version(response.url().as_str(), Some(re)))
        {
            return Ok(Some(version));
        }

        let body = read_capped(response, MAX_VERSION_BODY).await?;
        let selector = source
            .version_selector
            .as_deref()
            .map(scraper::Selector::parse)
            .transpose()
            .map_err(|_| miss())?;
        page::find_version(&body, selector.as_ref(), regex.as_ref())
    };
    found.map(Some).ok_or_else(miss)
}

/// Sends a GET to `url`, following redirects.
///
/// # Errors
///
/// [`VendorError::RequestFailed`] for a non-2xx answer, and [`VendorError::Http`] when the host
/// cannot be reached.
pub async fn fetch(http: &reqwest::Client, url: &str) -> Result<reqwest::Response> {
    let response = http.get(url).send().await?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(VendorError::RequestFailed {
        status: status.as_u16(),
        message: status.canonical_reason().unwrap_or_default().to_owned(),
    })
}

/// The first `limit` bytes of the body, decoded lossily.
async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<String> {
    let mut body = Vec::new();
    while body.len() < limit
        && let Some(chunk) = response.chunk().await?
    {
        body.extend_from_slice(&chunk);
    }
    body.truncate(limit);
    Ok(String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{UrlSource, fetch, latest_version, tags};
    use crate::error::VendorError;
    use crate::github::http::client;
    use crate::test_server::{route, serve};

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
