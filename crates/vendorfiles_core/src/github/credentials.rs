//! The platform's native credential store.
//!
//! Rather than the `keyring` facade - which compiles every backend's support code on every
//! target, and on Linux links libdbus - the store crates are pulled in per platform through
//! `[target.'cfg(…)'.dependencies]`: the Credential Manager on Windows, the login Keychain on
//! macOS, and on Linux the Secret Service with a keyutils fallback. On a target with no store
//! compiled in, [`entry`] yields `None` and token resolution falls back to `GITHUB_TOKEN`.
//!
//! Entries are built from a store handle this module owns rather than through
//! `keyring_core::set_default_store`, so there is no process-global to initialise in the right
//! order - the store is opened lazily, once, on first use.

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

/// Linux has two candidates, tried in order of how long they keep a secret.
///
/// The Secret Service persists to disk, so a token stored there survives a reboot - but it needs
/// a daemon (`gnome-keyring`, `KWallet`, `KeePassXC`) that a headless box, a minimal container or WSL
/// may not have. keyutils always works but lives in kernel memory, so it comes second and
/// [`transience_warning`] tells the user what they got.
#[cfg(target_os = "linux")]
fn native() -> keyring_core::Result<Arc<CredentialStore>> {
    match zbus_secret_service_keyring_store::Store::new() {
        Ok(store) => Ok(store),
        Err(_) => Ok(linux_keyutils_keyring_store::Store::new()?),
    }
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
        // `UntilDelete` - the desktop keychains - needs no caveat. And since
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

    /// Whether an error says the environment has no usable store, rather than that the code
    /// asked for the wrong thing.
    ///
    /// A CI runner can lack an unlocked keychain or a session keyring; that is not a defect
    /// here, and the tool already degrades to `GITHUB_TOKEN` when it happens.
    fn is_environmental(error: &keyring_core::Error) -> bool {
        matches!(
            error,
            keyring_core::Error::NoStorageAccess(_)
                | keyring_core::Error::PlatformFailure(_)
                | keyring_core::Error::NotSupportedByStore(_)
        )
    }

    #[test]
    fn building_an_entry_does_not_create_a_credential() {
        // `build` is a specifier: it must not read or write the underlying credential.
        let handle = entry("vendorfiles-cli-test", "does-not-exist").expect("a store");
        match handle.get_password() {
            Err(keyring_core::Error::NoEntry) => {}
            Err(other) if is_environmental(&other) => eprintln!("store unavailable: {other}"),
            Err(other) => panic!("unexpected keyring error: {other}"),
            Ok(_) => panic!("`build` must not materialise a credential"),
        }
    }

    #[test]
    fn secrets_round_trip_through_the_platform_store() {
        // Writing is the half that only shows up when a user runs `vendor login`, so exercise
        // it here - under a name of its own, never the real credential.
        let handle = entry("vendorfiles-cli-test", "round-trip").expect("a store");
        match handle.set_password("ghp_roundtrip_0123456789") {
            Ok(()) => {}
            Err(error) if is_environmental(&error) => {
                eprintln!("store not writable here: {error}");
                return;
            }
            Err(error) => panic!("unexpected keyring error: {error}"),
        }

        assert_eq!(
            handle.get_password().expect("read back what was written"),
            "ghp_roundtrip_0123456789"
        );
        handle.delete_credential().expect("delete");
        assert!(
            matches!(handle.get_password(), Err(keyring_core::Error::NoEntry)),
            "the credential should be gone after deletion"
        );
    }

    #[test]
    fn the_warning_says_exactly_what_the_store_reports() {
        let Some(store) = open() else { return };
        let durable = matches!(
            store.persistence(),
            keyring_core::api::CredentialPersistence::UntilDelete
        );
        assert_eq!(
            durable,
            transience_warning().is_none(),
            "a durable store must warn about nothing, and a transient one must warn ({})",
            store.vendor()
        );
    }

    #[test]
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn desktop_keychains_are_durable() {
        assert_eq!(transience_warning(), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_persistent_store_wins_when_the_system_offers_one() {
        let Some(store) = open() else { return };
        if zbus_secret_service_keyring_store::Store::new().is_ok() {
            assert!(
                store.vendor().contains("Secret Service"),
                "keyutils was chosen despite a working Secret Service: {}",
                store.vendor()
            );
            assert_eq!(transience_warning(), None);
        } else {
            // No daemon here, so the session-scoped store is the honest answer - and it has to
            // say so.
            assert!(
                transience_warning().is_some_and(|w| w.contains("reboot")),
                "the keyutils fallback must warn about reboots"
            );
        }
    }
}
