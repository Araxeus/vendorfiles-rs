//! Parsing one `install` operand.
//!
//! `install` takes any number of sources, each of which may carry its own version as
//! `source@version`. Splitting that is purely syntactic - which repository a source names is
//! `run::resolve_source`'s job - so it lives here on its own and is tested without a workspace.

/// Splits `source@version` into its two halves.
///
/// The `@` has to fall after the last `/` to count as a separator, so a URL carrying userinfo
/// (`https://user@github.com/o/r`) keeps it, and it may not be the first byte, so nothing is
/// split off an empty source. A separator with nothing after it names no version.
#[must_use]
pub fn split(arg: &str) -> (&str, Option<&str>) {
    let tail = arg.rfind('/').map_or(0, |index| index + 1);
    let Some(at) = arg[tail..].rfind('@').map(|index| index + tail) else {
        return (arg, None);
    };
    if at == 0 {
        return (arg, None);
    }
    let version = &arg[at + 1..];
    (&arg[..at], (!version.is_empty()).then_some(version))
}

/// Whether `arg` is a bare version rather than something that could name a repository.
///
/// Only used to catch the reference CLI's `install <url/name> [version]` form, where the version
/// was a second operand: every operand is a source now, so `vendor add owner/repo v1.0.0` would
/// otherwise search GitHub for a repository called `v1.0.0`.
#[must_use]
pub fn looks_like_bare_version(arg: &str) -> bool {
    is_commit_sha(arg) || is_dotted_number(arg)
}

/// A whole commit hash. Nothing shorter: `deadbee` is a plausible repository name.
fn is_commit_sha(arg: &str) -> bool {
    arg.len() == 40 && arg.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `1.0`, `v1.0.0`, `1.0.0-beta.1` - at least one dot, since `v1` could be a name.
fn is_dotted_number(arg: &str) -> bool {
    let core = arg.strip_prefix('v').unwrap_or(arg);
    // A prerelease or build suffix belongs to the tag, not to the number in front of it.
    let (number, suffix) = core.find(['-', '+']).map_or((core, None), |index| {
        (&core[..index], Some(&core[index + 1..]))
    });
    if let Some(suffix) = suffix
        && (suffix.is_empty()
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
    {
        return false;
    }
    number.contains('.')
        && number
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::{looks_like_bare_version, split};

    #[test]
    fn a_source_with_no_at_sign_carries_no_version() {
        assert_eq!(split("rg"), ("rg", None));
        assert_eq!(split("owner/repo"), ("owner/repo", None));
        assert_eq!(
            split("https://github.com/o/r"),
            ("https://github.com/o/r", None)
        );
    }

    #[test]
    fn the_half_after_the_at_sign_is_the_version() {
        assert_eq!(split("rg@1.2.3"), ("rg", Some("1.2.3")));
        assert_eq!(split("owner/repo@v1.0.0"), ("owner/repo", Some("v1.0.0")));
        assert_eq!(
            split("https://github.com/o/r@v1.0.0"),
            ("https://github.com/o/r", Some("v1.0.0"))
        );
    }

    #[test]
    fn an_at_sign_before_the_last_slash_belongs_to_the_url() {
        // Userinfo in a URL, not a version: the `@` has to come after the last `/` to be a
        // separator, or a host would be read as one.
        assert_eq!(
            split("https://user@github.com/o/r"),
            ("https://user@github.com/o/r", None)
        );
    }

    #[test]
    fn a_version_that_is_not_there_is_not_a_version() {
        assert_eq!(split("rg@"), ("rg", None));
        // Nothing before the `@` means nothing was separated.
        assert_eq!(split("@scope"), ("@scope", None));
        assert_eq!(split("@"), ("@", None));
    }

    #[test]
    fn the_last_at_sign_wins() {
        assert_eq!(split("rg@1@2"), ("rg@1", Some("2")));
    }

    #[test]
    fn a_dotted_number_is_a_bare_version() {
        assert!(looks_like_bare_version("v1.0.0"));
        assert!(looks_like_bare_version("1.0.0"));
        assert!(looks_like_bare_version("v0.17"));
        assert!(looks_like_bare_version("v1.0.0-beta.1"));
        assert!(looks_like_bare_version("1.0.0+build2"));
    }

    #[test]
    fn a_full_commit_sha_is_a_bare_version() {
        assert!(looks_like_bare_version(
            "f6ec482ea395cead4fd849c05df6edd8da284a52"
        ));
    }

    #[test]
    fn a_name_that_could_be_a_repository_is_not_a_bare_version() {
        assert!(!looks_like_bare_version("fd"));
        assert!(!looks_like_bare_version("owner/repo"));
        assert!(!looks_like_bare_version("https://github.com/o/r"));
        assert!(!looks_like_bare_version("v1"));
        assert!(!looks_like_bare_version("1"));
        assert!(!looks_like_bare_version(""));
        // A short hex string is a plausible name; only a whole sha is unmistakable.
        assert!(!looks_like_bare_version("deadbee"));
        assert!(!looks_like_bare_version("v1.0.0-"));
    }
}
