//! The platform's native credential store.
//!
//! Rather than the `keyring` facade — which compiles every backend's support code on every
//! target, and on Linux links libdbus for a Secret Service backend this tool does not use —
//! exactly one store crate is pulled in per platform through `[target.'cfg(…)'.dependencies]`.
//! On a target with no store compiled in, [`entry`] yields `None` and token resolution simply
//! falls back to `GITHUB_TOKEN`.
//!
//! Entries are built from a store handle this module owns rather than through
//! `keyring_core::set_default_store`, so there is no process-global to initialise in the right
//! order — the store is opened lazily, once, on first use.

use std::sync::{Arc, OnceLock};

use keyring_core::api::CredentialPersistence;
use keyring_core::{CredentialStore, Entry};

/// The store for this platform, or `None` if there is none or it could not be opened.
fn open() -> Option<&'static Arc<CredentialStore>> {
    static STORE: OnceLock<Option<Arc<CredentialStore>>> = OnceLock::new();
    STORE.get_or_init(|| native().ok()).as_ref()
}

#[cfg(target_os = "windows")]
fn native() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn native() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "linux")]
fn native() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(linux_keyutils_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn native() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(keyring_core::Error::NotSupportedByStore(
        "no native credential store is available for this platform".to_owned(),
    ))
}

/// A handle to the `service`/`user` credential, or `None` when there is no usable store.
#[must_use]
pub fn entry(service: &str, user: &str) -> Option<Entry> {
    open()?.build(service, user, None).ok()
}

/// Why a token stored here might not still be there next time, if that is a risk.
///
/// The Linux keyutils store keeps secrets in kernel memory, so a token saved by `vendor login`
/// does not survive a reboot. Saying so beats letting the next run look mysteriously anonymous.
#[must_use]
pub fn transience_warning() -> Option<&'static str> {
    match open()?.persistence() {
        CredentialPersistence::UntilReboot => Some(
            "this system's credential store keeps secrets in kernel memory, \
             so the token will be gone after a reboot",
        ),
        CredentialPersistence::UntilLogout => Some(
            "this system's credential store is tied to your login session, \
             so the token will be gone once you log out",
        ),
        CredentialPersistence::ProcessOnly | CredentialPersistence::EntryOnly => Some(
            "this system's credential store does not persist, \
             so the token will be gone when this command exits",
        ),
        // `UntilDelete` — the desktop keychains — needs no caveat. And since
        // `CredentialPersistence` is `#[non_exhaustive]`, an unknown variant is not a reason
        // to invent one either.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{entry, open, transience_warning};

    #[test]
    fn a_store_is_compiled_in_for_this_platform() {
        // Every target this project ships for has one; a missing store would silently
        // downgrade every user to `GITHUB_TOKEN`-only.
        assert!(
            open().is_some(),
            "no native credential store for {}",
            std::env::consts::OS
        );
    }

    #[test]
    fn entries_can_be_built_without_touching_the_store() {
        // `build` is a specifier: it must not read or write the underlying credential.
        let handle = entry("vendorfiles-cli-test", "does-not-exist").expect("a store");
        assert!(matches!(
            handle.get_password(),
            Err(keyring_core::Error::NoEntry)
        ));
    }

    #[test]
    fn secrets_round_trip_through_the_platform_store() {
        // Writing is the half that only shows up when a user runs `vendor login`, so exercise
        // it here — under a name of its own, never the real credential.
        let handle = entry("vendorfiles-cli-test", "round-trip").expect("a store");
        handle
            .set_password("ghp_roundtrip_0123456789")
            .expect("write");
        assert_eq!(
            handle.get_password().expect("read"),
            "ghp_roundtrip_0123456789"
        );
        handle.delete_credential().expect("delete");
        assert!(matches!(
            handle.get_password(),
            Err(keyring_core::Error::NoEntry)
        ));
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn desktop_keychains_are_durable() {
        assert_eq!(transience_warning(), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_keyutils_store_is_reported_as_lost_on_reboot() {
        assert!(
            transience_warning().is_some_and(|w| w.contains("reboot")),
            "keyutils should warn about reboots"
        );
    }
}
