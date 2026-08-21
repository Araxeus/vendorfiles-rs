//! The registry of programs installable by name.
//!
//! `vendor add fd` should not require knowing that `fd` lives at `sharkdp/fd` and ships its binary
//! inside a directory named after the target triple. A hosted file records that once, and a pull
//! request against it is how a program becomes installable by name.
//!
//! Only `install` ever reads it. The file is fetched from `raw.githubusercontent.com` rather than
//! the GitHub API — no anonymous rate limit — and cached, so the usual case makes no request at
//! all. See [`cache`] for the freshness rules and [`schema`] for the trust boundary.

pub mod cache;
pub mod resolve;
pub mod schema;

use std::path::Path;

pub use resolve::Entry;
use schema::{Document, SUPPORTED_VERSION};

use crate::error::{Result, VendorError};

/// The registry, as loaded.
#[derive(Debug, Clone)]
pub struct Registry {
    document: Document,
}

impl Registry {
    /// Parses a registry, refusing one written for a newer `vendor`.
    ///
    /// # Errors
    ///
    /// Returns [`VendorError::RegistryUnreadable`] if the text is not a registry, and
    /// [`VendorError::RegistryTooNew`] if it declares a version this build does not understand.
    pub fn parse(text: &str) -> Result<Self> {
        let document: Document = serde_yaml_ng::from_str(text)
            .map_err(|source| VendorError::RegistryUnreadable(source.to_string()))?;
        if document.version > SUPPORTED_VERSION {
            return Err(VendorError::RegistryTooNew {
                found: document.version,
                supported: SUPPORTED_VERSION,
            });
        }
        Ok(Self { document })
    }

    /// The program `name` refers to, by canonical name first and then by alias.
    ///
    /// Canonical names win, so adding an alias can never quietly redirect an existing one.
    #[must_use]
    pub fn find(&self, name: &str) -> Option<(&str, &schema::Program)> {
        if let Some((canonical, program)) = self.document.programs.get_key_value(name) {
            return Some((canonical.as_str(), program));
        }
        self.document
            .programs
            .iter()
            .find(|(_, program)| program.aliases.iter().any(|alias| alias == name))
            .map(|(canonical, program)| (canonical.as_str(), program))
    }

    /// Resolves `name` for this host.
    ///
    /// # Errors
    ///
    /// Propagates [`resolve::for_host`] when the name is known but this platform is not covered.
    pub fn entry(&self, name: &str) -> Result<Option<Entry>> {
        match self.find(name) {
            Some((canonical, program)) => {
                resolve::for_host(canonical, program, &resolve::host()).map(Some)
            }
            None => Ok(None),
        }
    }

    /// Every canonical name, in file order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.document.programs.keys().map(String::as_str)
    }
}

/// Loads the registry and resolves `name`, or `None` if it names nothing.
///
/// # Errors
///
/// Returns an error when the registry cannot be obtained at all, or when it covers `name` but not
/// this platform. Callers treat the former as a miss — being offline should not stop
/// `vendor add owner/repo` from working — so the message is theirs to report.
pub async fn lookup(name: &str, refresh: bool) -> Result<Option<Entry>> {
    let registry = Registry::parse(&fetch(refresh).await?)?;
    registry.entry(name)
}

/// The registry text, from the override, the cache, or the network.
async fn fetch(refresh: bool) -> Result<String> {
    if let Some(source) = std::env::var_os(cache::OVERRIDE) {
        return from_override(&source).await;
    }

    let Some(directory) = cache::directory() else {
        // Nowhere to cache: fetch and use it for this run only.
        return download(None).await.map(Fetched::into_text);
    };
    let file = directory.join("registry.yml");
    let tag = directory.join("registry.etag");

    let action = cache::decide(
        cache::age(&file),
        tokio::fs::read_to_string(&tag).await.ok(),
        refresh,
    );
    if action == cache::Action::UseCache
        && let Ok(text) = tokio::fs::read_to_string(&file).await
    {
        return Ok(text);
    }

    let etag = match action {
        cache::Action::Revalidate(etag) => etag,
        _ => None,
    };
    match download(etag.as_deref()).await {
        Ok(Fetched {
            text: None,
            etag: _,
        }) => {
            // Unchanged. Mark the copy fresh so the next day's installs skip the request too.
            let text =
                tokio::fs::read_to_string(&file)
                    .await
                    .map_err(|source| VendorError::ReadFile {
                        path: file.clone(),
                        source,
                    })?;
            let _ = filetime_now(&file).await;
            Ok(text)
        }
        Ok(Fetched {
            text: Some(text),
            etag,
        }) => {
            store(&directory, &file, &tag, &text, etag.as_deref()).await;
            Ok(text)
        }
        // A failed fetch falls back to whatever is on disk, however old: a registry that cannot be
        // reached should slow nobody down.
        Err(error) => tokio::fs::read_to_string(&file).await.map_err(|_| error),
    }
}

/// Reads the override, which may be a URL or a path.
async fn from_override(source: &std::ffi::OsStr) -> Result<String> {
    let source = source.to_string_lossy().into_owned();
    if source.starts_with("http://") || source.starts_with("https://") {
        return get(&source, None).await.map(Fetched::into_text);
    }
    let path = Path::new(&source);
    tokio::fs::read_to_string(path)
        .await
        .map_err(|source| VendorError::ReadFile {
            path: path.to_path_buf(),
            source,
        })
}

/// A registry response: `None` text means the cached copy is still current.
struct Fetched {
    text: Option<String>,
    etag: Option<String>,
}

impl Fetched {
    /// The body, empty when the server said nothing changed and there is no cache to fall back on.
    fn into_text(self) -> String {
        self.text.unwrap_or_default()
    }
}

/// Fetches the default registry.
async fn download(etag: Option<&str>) -> Result<Fetched> {
    get(cache::DEFAULT_URL, etag).await
}

/// Conditionally fetches `url`.
async fn get(url: &str, etag: Option<&str>) -> Result<Fetched> {
    let mut request = crate::github::http::client()?.get(url);
    if let Some(etag) = etag {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request
        .send()
        .await
        .map_err(|source| VendorError::RegistryUnreachable(source.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched {
            text: None,
            etag: etag.map(str::to_owned),
        });
    }
    if !response.status().is_success() {
        return Err(VendorError::RegistryUnreachable(format!(
            "{url} returned {}",
            response.status()
        )));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let text = response
        .text()
        .await
        .map_err(|source| VendorError::RegistryUnreachable(source.to_string()))?;
    Ok(Fetched {
        text: Some(text),
        etag,
    })
}

/// Writes the registry and its `ETag`, ignoring failures — a cache that cannot be written only
/// costs the next run a request.
async fn store(directory: &Path, file: &Path, tag: &Path, text: &str, etag: Option<&str>) {
    if tokio::fs::create_dir_all(directory).await.is_err() {
        return;
    }
    let _ = tokio::fs::write(file, text).await;
    match etag {
        Some(etag) => {
            let _ = tokio::fs::write(tag, etag).await;
        }
        None => {
            let _ = tokio::fs::remove_file(tag).await;
        }
    }
}

/// Marks a file as touched now, by rewriting it in place.
async fn filetime_now(file: &Path) -> Result<()> {
    let text = tokio::fs::read_to_string(file)
        .await
        .map_err(|source| VendorError::ReadFile {
            path: file.to_path_buf(),
            source,
        })?;
    tokio::fs::write(file, text)
        .await
        .map_err(|source| VendorError::WriteFile {
            path: file.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::Registry;
    use crate::model::FileEntry;

    const REGISTRY: &str = r#"
version: 1
programs:
  fd:
    aliases: [fdfind, fd-find]
    repository: https://github.com/sharkdp/fd
    asset: "{release}/fd-v{version}-{target}{ext}"
    member: "fd-v{version}-{target}/fd{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
  rg:
    repository: https://github.com/BurntSushi/ripgrep
    asset: "{release}/ripgrep-{version}-{target}{ext}"
    member: "ripgrep-{version}-{target}/rg{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
"#;

    #[test]
    fn a_name_resolves_to_itself() {
        let registry = Registry::parse(REGISTRY).unwrap();
        let (canonical, _) = registry.find("fd").expect("fd is known");
        assert_eq!(canonical, "fd");
    }

    #[test]
    fn an_alias_resolves_to_the_canonical_name() {
        // The whole point of the feature: `vendor add fdfind` keys the entry `fd`.
        let registry = Registry::parse(REGISTRY).unwrap();
        for alias in ["fdfind", "fd-find"] {
            let (canonical, _) = registry.find(alias).expect("alias is known");
            assert_eq!(canonical, "fd", "for {alias}");
        }
    }

    #[test]
    fn an_unknown_name_is_a_miss_rather_than_an_error() {
        // `install` falls through to its GitHub search, so this must not be fatal.
        let registry = Registry::parse(REGISTRY).unwrap();
        assert!(registry.find("nothing-like-this").is_none());
        assert!(registry.entry("nothing-like-this").unwrap().is_none());
    }

    #[test]
    fn a_registry_from_a_newer_vendor_is_refused_with_a_readable_reason() {
        let error = Registry::parse("version: 99\nprograms: {}\n").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("99"), "{message}");
        assert!(message.contains('1'), "{message}");
    }

    #[test]
    fn text_that_is_not_a_registry_is_refused() {
        assert!(Registry::parse("this is not: [a, registry").is_err());
        assert!(
            Registry::parse("programs: {}\n").is_err(),
            "version required"
        );
    }

    /// The registry this repository ships, which is what `vendor add` fetches.
    fn shipped() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../registry.yml")
            .canonicalize()
            .expect("registry.yml sits at the repository root");
        std::fs::read_to_string(path).expect("readable")
    }

    #[test]
    fn the_shipped_registry_resolves_for_every_host_it_claims() {
        // The gate that makes pull requests against `registry.yml` safe to accept: a typo in a
        // macOS entry fails here even when the test runs on Linux.
        let registry = Registry::parse(&shipped()).expect("the shipped registry parses");
        let mut checked = 0;
        for name in registry.names() {
            let (canonical, program) = registry.find(name).expect("just listed");
            assert!(
                program.repository.starts_with("https://github.com/"),
                "{canonical}: repository must be a GitHub URL, found {}",
                program.repository
            );
            assert!(
                !program.targets.is_empty(),
                "{canonical}: needs at least one target"
            );
            for host in program.targets.keys() {
                let entry = super::resolve::for_host(canonical, program, host)
                    .unwrap_or_else(|error| panic!("{canonical} for {host}: {error}"));
                let [FileEntry::Mapped(files)] = entry.files.as_slice() else {
                    panic!("{canonical} for {host}: expected one mapped entry");
                };
                let (asset, _) = files.iter().next().expect("one asset");
                assert!(
                    asset.starts_with("{release}/"),
                    "{canonical} for {host}: assets need the {{release}}/ prefix, found {asset}"
                );
                assert!(
                    !asset.contains("{target}") && !asset.contains("{ext}"),
                    "{canonical} for {host}: host placeholders should be expanded, found {asset}"
                );
                let expected = if host.starts_with("windows") {
                    ".zip"
                } else {
                    ".tar.gz"
                };
                assert!(
                    asset.ends_with(expected),
                    "{canonical} for {host}: expected a {expected} asset, found {asset}"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "the registry should not be empty");
    }

    #[test]
    fn the_shipped_registry_has_no_duplicate_names_or_aliases() {
        // Two entries claiming one alias would make `vendor add` depend on file order.
        let registry = Registry::parse(&shipped()).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for name in registry.names() {
            let (canonical, program) = registry.find(name).unwrap();
            assert!(
                seen.insert(canonical.to_owned()),
                "duplicate name {canonical}"
            );
            for alias in &program.aliases {
                assert!(
                    seen.insert(alias.clone()),
                    "{canonical}: alias '{alias}' is already taken"
                );
            }
        }
    }

    #[test]
    fn names_are_listed_in_file_order() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(registry.names().collect::<Vec<_>>(), ["fd", "rg"]);
    }
}
