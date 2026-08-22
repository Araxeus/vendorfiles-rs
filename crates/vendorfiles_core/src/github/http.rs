//! The HTTP client used for streaming downloads and the token check.
//!
//! `reqwest` is built with `rustls-no-provider`, so the process has to name a crypto provider
//! before a client can be created. Doing that here - rather than pulling in reqwest's `rustls`
//! feature - keeps `ring` as the only crypto backend in the tree; that feature would add
//! aws-lc-rs alongside the `ring` build octocrab already uses.

use std::sync::Once;

use crate::error::{Result, VendorError};

/// Sent on every request; the GitHub API rejects requests without one.
pub const USER_AGENT: &str = concat!("vendorfiles/", env!("CARGO_PKG_VERSION"));

/// Installs `ring` as the process-wide rustls provider, once.
fn install_tls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // An error here only means a provider was already installed, which is just as good.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Builds a client with the tool's user agent and a working TLS stack.
///
/// # Errors
///
/// Returns [`VendorError::Http`] if the client cannot be constructed.
pub fn client() -> Result<reqwest::Client> {
    install_tls_provider();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| VendorError::Http(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::client;

    #[test]
    fn a_client_can_be_built_which_proves_the_tls_provider_is_installed() {
        // With `rustls-no-provider` and no provider installed, this fails.
        assert!(client().is_ok());
        // The provider install must be idempotent.
        assert!(client().is_ok());
    }
}
