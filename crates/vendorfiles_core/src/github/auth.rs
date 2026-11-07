//! GitHub authentication: token resolution and the OAuth device flow.
//!
//! Resolution order is `GITHUB_TOKEN` → OS keyring → anonymous.

use std::io::{BufRead, Write};

use secrecy::ExposeSecret;

use crate::error::{Result, VendorError};
use crate::github::credentials;
use crate::ui;

/// Keyring service name - shared with the reference implementation.
const KEYRING_SERVICE: &str = "vendorfiles-cli";
/// Keyring entry name.
///
/// Deliberately *not* the reference's `github_token`. That entry holds an AES-CBC blob keyed
/// on the machine's hostname; writing a plaintext token there would leave the TypeScript tool
/// unable to decrypt its own credential. A distinct entry lets both tools stay logged in.
const KEYRING_USER: &str = "github_token_plain";
/// The reference tool's entry, read only so a stale value can be recognised and ignored.
const LEGACY_KEYRING_USER: &str = "github_token";
/// The OAuth app the device flow authenticates against.
pub const OAUTH_CLIENT_ID: &str = "39d3104ecbbfd876dfa5";

/// A GitHub token, kept out of `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Wraps a token string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw token, for use in an `Authorization` header.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Token(***)")
    }
}

/// Whether a stored secret can be a GitHub token.
///
/// The reference stored an AES-CBC blob under the same keyring entry. Base64 ciphertext
/// almost always contains characters no GitHub token uses, so treating such a value as
/// "absent" downgrades a stale entry to anonymous access instead of a confusing 401.
#[must_use]
pub fn is_plausible_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() >= 20
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Reads the token from the OS keyring, if one is stored and usable.
///
/// Falls back to the reference tool's entry so a machine that only ever used the TypeScript
/// CLI still works - unless the stored value is that tool's ciphertext, which cannot be a
/// token and is treated as "not logged in" rather than sent upstream to fail with a 401.
///
/// On Linux that fallback only reaches the reference's token when a Secret Service daemon is
/// running, since the keyutils fallback store is a different backend entirely.
#[must_use]
pub fn keyring_token() -> Option<Token> {
    stored(KEYRING_USER).or_else(|| stored(LEGACY_KEYRING_USER))
}

/// The usable token in one keyring entry, if there is one.
fn stored(user: &str) -> Option<Token> {
    credentials::entry(KEYRING_SERVICE, user)?
        .get_password()
        .ok()
        .filter(|value| is_plausible_token(value))
        .map(Token::new)
}

/// The token in `GITHUB_TOKEN`, unless it is unset or empty.
fn env_token() -> Option<Token> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
        .map(Token::new)
}

/// Resolves the token to use: `GITHUB_TOKEN`, then the keyring, then none.
#[must_use]
pub fn resolve_token() -> Option<Token> {
    env_token().or_else(keyring_token)
}

/// [`resolve_token`] without stalling the async runtime.
///
/// Reading the OS credential store is a blocking IPC call - on Linux it can even prompt the
/// user to unlock the keyring - so it belongs on the blocking pool.
pub async fn resolve_token_async() -> Option<Token> {
    tokio::task::spawn_blocking(resolve_token)
        .await
        .unwrap_or(None)
}

/// Stores a token in the OS keyring, warning (but not failing) if that is not possible.
///
/// A keyring that refuses the write is not fatal: the token still authenticates this run. A
/// store that accepts the write but will not keep it earns a warning of its own - see
/// [`credentials::transience_warning`].
pub fn save_token(token: &Token) {
    let stored = credentials::entry(KEYRING_SERVICE, KEYRING_USER)
        .is_some_and(|entry| entry.set_password(token.expose()).is_ok());
    if !stored {
        ui::warning("Failed to save token to keyring");
    } else if let Some(caveat) = credentials::transience_warning() {
        ui::warning(caveat);
    }
}

/// What the credential store had for `logout` to remove.
#[derive(Debug, PartialEq, Eq)]
enum Logout {
    Removed,
    NotStored,
}

/// Deletes a stored token, or reports that there was none.
fn forget(service: &str, user: &str) -> Result<Logout> {
    let Some(entry) = credentials::entry(service, user) else {
        return Ok(Logout::NotStored);
    };
    match entry.delete_credential() {
        Ok(()) => Ok(Logout::Removed),
        Err(keyring_core::Error::NoEntry) => Ok(Logout::NotStored),
        Err(error) => Err(VendorError::KeyringDelete(error.to_string())),
    }
}

/// Removes the token `login` saved, and warns about anything that still authenticates.
///
/// The reference tool's entry is not ours to delete, so a token still readable from it is
/// reported rather than removed.
///
/// # Errors
///
/// Returns [`VendorError::KeyringDelete`] if the store had a token but would not delete it.
pub async fn logout() -> Result<()> {
    let (outcome, leftover) = tokio::task::spawn_blocking(|| {
        let outcome = forget(KEYRING_SERVICE, KEYRING_USER);
        // Read after the delete, so this can only be the reference tool's entry.
        let leftover = outcome.is_ok() && keyring_token().is_some();
        (outcome, leftover)
    })
    .await
    .map_err(|_| VendorError::AuthUnknownFailure)?;

    match outcome? {
        Logout::Removed => ui::success("Logged out successfully"),
        Logout::NotStored => ui::info("Not logged in"),
    }
    if std::env::var("GITHUB_TOKEN").is_ok_and(|value| !value.is_empty()) {
        ui::warning("GITHUB_TOKEN is set, so requests are still authenticated");
    }
    if leftover {
        ui::warning(
            "a token from the vendorfiles npm CLI is still in the keyring, and will be used",
        );
    }
    Ok(())
}

/// Where the token a run uses comes from, in the order [`resolve_token`] tries them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Env,
    Keyring,
    NpmKeyring,
}

impl Origin {
    /// Completes "the token ..." in a sentence.
    const fn describe(self) -> &'static str {
        match self {
            Self::Env => "from GITHUB_TOKEN",
            Self::Keyring => "from the credential store",
            Self::NpmKeyring => "the vendorfiles npm CLI saved",
        }
    }

    /// What to do when GitHub rejects a token from here.
    ///
    /// `shadowed` means a keyring token sits behind `GITHUB_TOKEN`, so unsetting it is a fix.
    const fn fix(self, shadowed: bool) -> &'static str {
        match self {
            Self::Env if shadowed => {
                "Update or unset GITHUB_TOKEN - once it is unset, the token saved in the \
                 credential store is used instead"
            }
            Self::Env => "Update or unset GITHUB_TOKEN",
            Self::Keyring | Self::NpmKeyring => "Run `vendor login` to replace it",
        }
    }
}

/// Every place a token can come from, read at once so status can report shadowed ones.
struct Found {
    env: Option<Token>,
    saved: Option<Token>,
    npm: Option<Token>,
}

impl Found {
    fn read() -> Self {
        Self {
            env: env_token(),
            saved: stored(KEYRING_USER),
            npm: stored(LEGACY_KEYRING_USER),
        }
    }

    /// The token a run would use, with the same precedence as [`resolve_token`].
    fn active(self) -> Option<(Origin, Token, bool)> {
        let in_keyring = self.saved.is_some() || self.npm.is_some();
        if let Some(token) = self.env {
            return Some((Origin::Env, token, in_keyring));
        }
        self.saved
            .map(|token| (Origin::Keyring, token, false))
            .or_else(|| self.npm.map(|token| (Origin::NpmKeyring, token, false)))
    }
}

/// What GitHub said about a token.
#[derive(Debug, PartialEq, Eq)]
enum Check {
    Valid {
        login: Option<String>,
        quota: Option<String>,
        expires: Option<String>,
    },
    /// The token authenticated, but its quota is spent. Carries the rate-limit detail.
    RateLimited(String),
    Rejected,
    Failed {
        status: u16,
        message: String,
    },
}

/// Reads GitHub's answer to `GET /user`.
fn interpret(status: u16, headers: &reqwest::header::HeaderMap, body: &[u8], now: u64) -> Check {
    if (200..300).contains(&status) {
        let login = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|body| body.get("login")?.as_str().map(str::to_owned));
        let expires = headers
            .get("github-authentication-token-expiration")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        return Check::Valid {
            login,
            quota: quota_line(headers, now),
            expires,
        };
    }
    if status == 401 {
        return Check::Rejected;
    }
    if let Some(detail) = super::rate_limit_detail(status, headers, now) {
        return Check::RateLimited(detail);
    }
    let message = super::message_from(body);
    if crate::error::is_rate_limited(status, &message) {
        return Check::RateLimited(String::new());
    }
    Check::Failed { status, message }
}

/// The quota the `x-ratelimit-*` headers report, in words.
fn quota_line(headers: &reqwest::header::HeaderMap, now: u64) -> Option<String> {
    let number = |name: &str| super::header_number(headers, name);
    Some(describe_quota(
        number("x-ratelimit-remaining")?,
        number("x-ratelimit-limit")?,
        number("x-ratelimit-reset")?,
        now,
    ))
}

fn describe_quota(remaining: u64, limit: u64, reset: u64, now: u64) -> String {
    format!(
        "{remaining} of {limit} API requests left this hour ({})",
        super::resets_in(reset, now)
    )
}

/// Asks GitHub who a token belongs to.
///
/// `/user` answers for every kind of token, and its `x-ratelimit-*` headers are the truthful
/// quota - unlike `/rate_limit`, which can lag behind them for a token.
async fn check(token: &Token) -> Result<Check> {
    let unreachable = |e: reqwest::Error| VendorError::AuthCheckUnreachable(e.to_string());
    let response = super::http::client()?
        .get(format!("{}/user", super::API_ROOT))
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token.expose()),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .send()
        .await
        .map_err(unreachable)?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(unreachable)?;
    Ok(interpret(status, &headers, &body, super::now()))
}

/// Reports which token a run would use, and whether GitHub accepts it.
///
/// # Errors
///
/// Returns [`VendorError::NotLoggedIn`] when there is no token,
/// [`VendorError::StoredTokenRejected`] when GitHub refuses it, and
/// [`VendorError::AuthCheckUnreachable`] when GitHub cannot be asked.
pub async fn status() -> Result<()> {
    let found = tokio::task::spawn_blocking(Found::read)
        .await
        .map_err(|_| VendorError::AuthUnknownFailure)?;

    let Some((origin, token, shadowed)) = found.active() else {
        let client = super::http::client()?;
        if let Some(quota) = super::fetch_quota(&client).await {
            ui::info(format!(
                "Anonymous: {}",
                describe_quota(quota.remaining, quota.limit, quota.reset, super::now())
            ));
        }
        return Err(VendorError::NotLoggedIn);
    };

    match check(&token).await? {
        Check::Valid {
            login,
            quota,
            expires,
        } => {
            let who = login.map_or_else(String::new, |login| format!(" as {login}"));
            ui::success(format!(
                "Logged in to GitHub{who} with the token {}",
                origin.describe()
            ));
            if let Some(quota) = quota {
                ui::info(quota);
            }
            if let Some(expires) = expires {
                ui::info(format!("The token expires {expires}"));
            }
        }
        Check::RateLimited(detail) => ui::warning(format!(
            "The token {} is valid, but its rate limit is spent{detail}",
            origin.describe()
        )),
        Check::Rejected => {
            return Err(VendorError::StoredTokenRejected {
                origin: origin.describe(),
                fix: origin.fix(shadowed),
            });
        }
        Check::Failed { status, message } => {
            return Err(VendorError::RequestFailed { status, message });
        }
    }
    if shadowed {
        ui::info("A token is also saved in the credential store, but GITHUB_TOKEN comes first");
    }
    Ok(())
}

/// Verifies a token against the API and stores it.
///
/// # Errors
///
/// Returns [`VendorError::InvalidToken`], [`VendorError::TokenRateLimited`] or
/// [`VendorError::AuthUnknownFailure`] depending on how GitHub rejects the token.
pub async fn login_with_token(token: &str) -> Result<()> {
    let response = super::http::client()?
        .head("https://api.github.com")
        .header(reqwest::header::AUTHORIZATION, format!("bearer {token}"))
        .header(reqwest::header::CACHE_CONTROL, "no-store")
        .send()
        .await
        .map_err(|e| VendorError::Http(e.to_string()))?;

    match response.status().as_u16() {
        401 => return Err(VendorError::InvalidToken),
        403 => return Err(VendorError::TokenRateLimited),
        status if !(200..300).contains(&status) => return Err(VendorError::AuthUnknownFailure),
        _ => {}
    }

    store(Token::new(token)).await;
    ui::success("Token saved successfully");
    Ok(())
}

/// Runs the OAuth device flow, storing the resulting token.
///
/// The prompts reproduce the reference's wording and its "press Enter, then we open the
/// browser" sequencing.
///
/// # Errors
///
/// Returns [`VendorError::DeviceFlow`] if the code request or the poll for authorisation fails.
pub async fn login_with_device_flow() -> Result<()> {
    let crab = octocrab::Octocrab::builder()
        .base_uri("https://github.com")
        .map_err(VendorError::from)?
        .add_header(reqwest::header::ACCEPT, "application/json".to_owned())
        .build()
        .map_err(VendorError::from)?;

    let codes = crab
        .authenticate_as_device(&OAUTH_CLIENT_ID.into(), std::iter::empty::<&str>())
        .await
        .map_err(|e| VendorError::DeviceFlow(e.to_string()))?;

    println!("First, copy your one-time code: {}", codes.user_code);
    println!("Then press [Enter] to continue in your web browser");
    wait_for_enter();
    println!("Opening your web browser...");
    let _ = open::that_detached(&codes.verification_uri);

    let auth = codes
        .poll_until_available(&crab, &OAUTH_CLIENT_ID.into())
        .await
        .map_err(|e| VendorError::DeviceFlow(e.to_string()))?;

    store(Token::new(auth.access_token.expose_secret())).await;
    ui::success("Logged in successfully");
    Ok(())
}

/// [`save_token`] off the async runtime, for the same reason as [`resolve_token_async`].
async fn store(token: Token) {
    let _ = tokio::task::spawn_blocking(move || save_token(&token)).await;
}

fn wait_for_enter() {
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderMap, HeaderValue};

    use super::{
        Check, Found, Logout, Origin, Token, check, forget, interpret, is_plausible_token,
    };
    use crate::github::credentials;

    const NOW: u64 = 1_000_000;

    fn found(env: bool, saved: bool, npm: bool) -> Found {
        let token = |present: bool, value: &str| present.then(|| Token::new(value));
        Found {
            env: token(env, "env"),
            saved: token(saved, "saved"),
            npm: token(npm, "npm"),
        }
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn status_picks_the_token_a_run_would_use() {
        let pick = |f: Found| {
            f.active()
                .map(|(origin, token, shadowed)| (origin, token.expose().to_owned(), shadowed))
        };
        assert_eq!(
            pick(found(true, true, true)),
            Some((Origin::Env, "env".to_owned(), true))
        );
        assert_eq!(
            pick(found(true, false, false)),
            Some((Origin::Env, "env".to_owned(), false))
        );
        assert_eq!(
            pick(found(false, true, true)),
            Some((Origin::Keyring, "saved".to_owned(), false))
        );
        assert_eq!(
            pick(found(false, false, true)),
            Some((Origin::NpmKeyring, "npm".to_owned(), false))
        );
        assert_eq!(pick(found(false, false, false)), None);
    }

    #[test]
    fn an_accepted_token_reports_its_user_quota_and_expiry() {
        let reset = (NOW + 600).to_string();
        let map = headers(&[
            ("x-ratelimit-remaining", "4990"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-reset", &reset),
            (
                "github-authentication-token-expiration",
                "2026-12-01 00:00:00 UTC",
            ),
        ]);
        assert_eq!(
            interpret(200, &map, br#"{"login":"octocat"}"#, NOW),
            Check::Valid {
                login: Some("octocat".to_owned()),
                quota: Some(
                    "4990 of 5000 API requests left this hour (resets in 10 min)".to_owned()
                ),
                expires: Some("2026-12-01 00:00:00 UTC".to_owned()),
            }
        );
        // An OAuth token never expires, and a body without a login still counts as accepted.
        assert_eq!(
            interpret(200, &HeaderMap::new(), b"{}", NOW),
            Check::Valid {
                login: None,
                quota: None,
                expires: None,
            }
        );
    }

    #[test]
    fn a_refused_token_is_rejected_and_a_spent_one_is_not() {
        let body = br#"{"message":"Bad credentials"}"#;
        assert_eq!(
            interpret(401, &HeaderMap::new(), body, NOW),
            Check::Rejected
        );

        let reset = (NOW + 120).to_string();
        let spent = headers(&[
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-reset", &reset),
        ]);
        assert_eq!(
            interpret(403, &spent, b"{}", NOW),
            Check::RateLimited(" - 0 of 5000 left, resets in 2 min".to_owned())
        );

        let secondary = br#"{"message":"You have exceeded a secondary rate limit"}"#;
        assert_eq!(
            interpret(429, &HeaderMap::new(), secondary, NOW),
            Check::RateLimited(String::new())
        );

        assert_eq!(
            interpret(502, &HeaderMap::new(), b"<html>", NOW),
            Check::Failed {
                status: 502,
                message: String::new(),
            }
        );
    }

    #[test]
    fn an_unreachable_github_does_not_blame_the_token() {
        let message =
            crate::VendorError::AuthCheckUnreachable("connection refused".to_owned()).to_string();
        assert!(!message.contains("rejected"), "{message}");
        assert!(!message.contains("vendor login"), "{message}");
    }

    /// Costs one request against the anonymous limit of whoever runs it.
    #[tokio::test]
    #[ignore = "queries the GitHub API"]
    async fn github_rejects_a_made_up_token() {
        let token = Token::new("ghp_000000000000000000000000000000000000");
        assert_eq!(
            check(&token).await.expect("GitHub answers"),
            Check::Rejected
        );
    }

    #[test]
    fn plausible_tokens_exclude_base64_ciphertext() {
        assert!(is_plausible_token("ghp_0123456789abcdefghijklmnop"));
        assert!(is_plausible_token("github_pat_11ABCDEFG0abcdefghij"));
        assert!(!is_plausible_token("Zm9vYmFy+YmFyL2Zvbw=="));
        assert!(!is_plausible_token(""));
        assert!(!is_plausible_token("short"));
    }

    #[test]
    fn tokens_do_not_leak_through_debug() {
        assert_eq!(format!("{:?}", Token::new("ghp_secret")), "Token(***)");
    }

    #[test]
    fn forgetting_a_token_removes_it_once_and_then_says_so() {
        const USER: &str = "logout";
        let entry = credentials::entry("vendorfiles-cli-test", USER).expect("a store");
        if entry.set_password("ghp_logout_0123456789").is_err() {
            return; // No writable store here; `credentials`'s own tests report that.
        }
        assert_eq!(
            forget("vendorfiles-cli-test", USER).expect("delete"),
            Logout::Removed
        );
        assert_eq!(
            forget("vendorfiles-cli-test", USER).expect("delete again"),
            Logout::NotStored
        );
    }
}
