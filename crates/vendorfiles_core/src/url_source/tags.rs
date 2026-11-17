//! The latest tag of a git repository, read from the refs a git host lists.
//!
//! Both answers this reads, the protocol v2 `ls-refs` listing and the older full advertisement,
//! are pkt-lines: each is prefixed with its length as four hex digits.

use std::sync::LazyLock;

use regex::Regex;

static NUMBERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+(?:[._][0-9]+)*").expect("valid regex"));

/// The tag names in a ref listing, in the order the host sent them.
///
/// Peeled `^{}` lines repeat a tag that is already listed, so they are dropped.
#[must_use]
pub fn parse_advertisement(body: &[u8]) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = body;
    while let Some(len) = rest
        .get(..4)
        .and_then(|hex| std::str::from_utf8(hex).ok())
        .and_then(|hex| usize::from_str_radix(hex, 16).ok())
    {
        // `0000` is a flush packet with no payload.
        if len < 4 {
            rest = &rest[4..];
            continue;
        }
        let Some(line) = rest.get(4..len) else { break };
        rest = &rest[len..];

        // `<sha> <ref>`, then a NUL and the capability list on the first ref of the full
        // advertisement, or a space and attributes like `peeled:<sha>` in a v2 listing.
        let line = line
            .split(|&b| b == b'\0' || b == b'\n')
            .next()
            .unwrap_or_default();
        let Some(name) = std::str::from_utf8(line)
            .ok()
            .and_then(|l| l.split(' ').nth(1))
            .and_then(|r| r.strip_prefix("refs/tags/"))
        else {
            continue;
        };
        if !name.ends_with("^{}") {
            tags.push(name.to_owned());
        }
    }
    tags
}

/// How a tag sorts, most significant first:
///
/// - whether its numbers are dotted, so `v1.10.0` beats a build number like `r1234`;
/// - its first run of numbers joined by `.` or `_` (curl tags `curl-8_10_0`), with trailing zeros
///   dropped so `1.2` equals `1.2.0`;
/// - whether it is a stable release. Numbers followed by `-` mark a prerelease (`v2.0.0-rc.1`).
///
/// `None` for a tag with no digits.
fn sort_key(tag: &str) -> Option<(bool, Vec<u64>, bool)> {
    let run = NUMBERS.find(tag)?;
    let mut numbers = run
        .as_str()
        .split(['.', '_'])
        .map(str::parse)
        .collect::<Result<Vec<u64>, _>>()
        .ok()?;
    let dotted = numbers.len() > 1;
    while numbers.last() == Some(&0) {
        numbers.pop();
    }
    let stable = !tag[run.end()..].starts_with('-');
    Some((dotted, numbers, stable))
}

/// The latest of `tags`.
///
/// Without a regex only stable tags count. With one, every tag it matches counts, prereleases
/// included. Tags sort by [`sort_key`], and the greater text breaks any remaining tie.
#[must_use]
pub fn latest_tag(tags: &[String], regex: Option<&fancy_regex::Regex>) -> Option<String> {
    tags.iter()
        .filter(|tag| regex.is_none_or(|re| re.is_match(tag).unwrap_or(false)))
        .filter_map(|tag| sort_key(tag).map(|key| (key, tag)))
        .filter(|((_, _, stable), _)| *stable || regex.is_some())
        .max()
        .map(|(_, tag)| tag.clone())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{latest_tag, parse_advertisement};

    fn pkt(line: &str) -> String {
        format!("{:04x}{line}", line.len() + 4)
    }

    /// A smart-HTTP ref advertisement listing `refs`, as a git host sends it.
    pub fn advertisement(refs: &[&str]) -> Vec<u8> {
        let mut s = pkt("# service=git-upload-pack\n") + "0000";
        for (i, r) in refs.iter().enumerate() {
            let caps = if i == 0 {
                "\0multi_ack symref=HEAD:refs/heads/main"
            } else {
                ""
            };
            s += &pkt(&format!("{} {r}{caps}\n", "a".repeat(40)));
        }
        (s + "0000").into_bytes()
    }

    /// A protocol v2 `ls-refs` answer listing `refs`.
    pub fn ls_refs(refs: &[&str]) -> Vec<u8> {
        let mut s = String::new();
        for r in refs {
            s += &pkt(&format!("{} {r}\n", "b".repeat(40)));
        }
        (s + "0000").into_bytes()
    }

    #[test]
    fn a_protocol_v2_listing_reads_the_same_way() {
        let mut body = ls_refs(&["refs/tags/v2.0.0"]);
        // A peeled line carries its target after the name.
        body.splice(
            ..0,
            pkt(&format!(
                "{} refs/tags/v1.0.0 peeled:{}\n",
                "c".repeat(40),
                "d".repeat(40)
            ))
            .into_bytes(),
        );
        assert_eq!(parse_advertisement(&body), ["v1.0.0", "v2.0.0"]);
    }

    #[test]
    fn tags_are_read_and_peeled_lines_dropped() {
        let body = advertisement(&[
            "HEAD",
            "refs/heads/main",
            "refs/tags/v1.0.0",
            "refs/tags/v1.0.0^{}",
            "refs/tags/v1.1.0",
        ]);
        assert_eq!(parse_advertisement(&body), ["v1.0.0", "v1.1.0"]);
    }

    #[test]
    fn the_highest_stable_tag_wins_by_number_not_by_text() {
        let tags = ["v1.9.0", "v1.10.0", "v2.0.0-beta.1", "nightly", "v1.10"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v1.10.0"));
    }

    #[test]
    fn a_dotted_version_beats_a_bare_build_number() {
        let tags = ["r1234", "v1.10.0", "build-20240101", "v1.9.0"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v1.10.0"));
    }

    #[test]
    fn underscores_separate_numbers_too() {
        // curl tags its releases `curl-8_10_0`.
        let tags = ["curl-8_9_1", "curl-8_10_0"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("curl-8_10_0"));
    }

    #[test]
    fn a_regex_admits_prereleases_and_filters() {
        let tags = ["v1.0.0", "v2.0.0-beta.1", "cli-v9.0.0"].map(String::from);
        let re = fancy_regex::Regex::new("^v").unwrap();
        assert_eq!(
            latest_tag(&tags, Some(&re)).as_deref(),
            Some("v2.0.0-beta.1")
        );
    }

    #[test]
    fn at_equal_numbers_stable_beats_prerelease() {
        let tags = ["v2.0.0-rc.1", "v2.0.0"].map(String::from);
        let re = fancy_regex::Regex::new(".").unwrap();
        assert_eq!(latest_tag(&tags, Some(&re)).as_deref(), Some("v2.0.0"));
    }
}
