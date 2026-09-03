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
    let read = |user: &str| {
        credentials::entry(KEYRING_SERVICE, user)?
            .get_password()
            .ok()
            .filter(|value| is_plausible_token(value))
    };
    read(KEYRING_USER)
        .or_else(|| read(LEGACY_KEYRING_USER))
        .map(Token::new)
}

/// Resolves the token to use: `GITHUB_TOKEN`, then the keyring, then none.
#[must_use]
pub fn resolve_token() -> Option<Token> {
    std::env::var("GITHUB_TOKEN")
        .ok()
        .filter(|v| !v.is_empty())
        .map(Token::new)
        .or_else(keyring_token)
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
    use super::{Logout, Token, forget, is_plausible_token};
    use crate::github::credentials;

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
