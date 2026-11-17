//! The latest tag of a git repository, read from the refs a git host lists.
//!
//! Both answers this reads, the protocol v2 `ls-refs` listing and the older full advertisement,
//! are pkt-lines: each is prefixed with its length as four hex digits.

use std::collections::HashMap;
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

/// Whether the text after a tag's numbers marks a prerelease: a `-` (`v2.0.0-rc.1`), or a marker
/// straight after the numbers (`go1.25rc1`, `1.0.beta2`). Any other letter names a release, as in
/// OpenSSL's `1_1_1w`.
fn is_prerelease_suffix(rest: &str) -> bool {
    let rest = rest.trim_start_matches(['.', '_']).to_ascii_lowercase();
    rest.starts_with('-')
        || ["rc", "alpha", "beta", "pre", "dev"]
            .iter()
            .any(|marker| rest.starts_with(marker))
}

/// What a tag sorts by.
struct SortKey<'a> {
    /// The text before its numbers, like `v` or `curl-`.
    prefix: &'a str,
    /// Whether its numbers are dotted, unlike a build number like `r1234`.
    dotted: bool,
    /// Its first run of numbers joined by `.` or `_` (curl tags `curl-8_10_0`), with trailing
    /// zeros dropped so `1.2` equals `1.2.0`.
    numbers: Vec<u64>,
    /// Whether it is a stable release. See [`is_prerelease_suffix`].
    stable: bool,
}

/// The [`SortKey`] of `tag`, or `None` for a tag with no digits.
fn sort_key(tag: &str) -> Option<SortKey<'_>> {
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
    Some(SortKey {
        prefix: &tag[..run.start()],
        dotted,
        numbers,
        stable: !is_prerelease_suffix(&tag[run.end()..]),
    })
}

/// The widest gap between bare majors that still reads as one run of releases.
const BARE_MAJOR_REACH: u64 = 10;

/// The latest of `tags`.
///
/// Without a regex only stable tags count, and tags with `@` are left out: in a monorepo those
/// are its packages' releases, like `plugin-legacy@6.1.0`. With a regex, every tag it matches
/// counts, prereleases included.
///
/// A bare number counts as a release only beside dotted tags with its own prefix, ignoring case,
/// and only in the run of majors that carries on from their newest one, each within
/// [`BARE_MAJOR_REACH`] of the last: systemd tags `v263` after `v262.1`, and gwsw/less went from
/// `v581.2` to `v712`. Otherwise it is a build number or a date, like `r1234` or `20240101`, and
/// any dotted tag beats it. Then numbers decide, a stable tag beats a prerelease, and the greater
/// text breaks any remaining tie.
#[must_use]
pub fn latest_tag(tags: &[String], regex: Option<&fancy_regex::Regex>) -> Option<String> {
    let candidates: Vec<(SortKey<'_>, &String)> = tags
        .iter()
        .filter(|tag| {
            regex.map_or_else(
                || !tag.contains('@'),
                |re| re.is_match(tag).unwrap_or(false),
            )
        })
        .filter_map(|tag| sort_key(tag).map(|key| (key, tag)))
        .filter(|(key, _)| key.stable || regex.is_some())
        .collect();

    let major = |key: &SortKey<'_>| key.numbers.first().copied().unwrap_or(0);

    // Per prefix: the newest dotted major, then the bare majors above it.
    let mut newest_dotted: HashMap<String, u64> = HashMap::new();
    let mut bare_majors: HashMap<String, Vec<u64>> = HashMap::new();
    for (key, _) in &candidates {
        let prefix = key.prefix.to_ascii_lowercase();
        if key.dotted {
            let newest = newest_dotted.entry(prefix).or_insert(0);
            *newest = (*newest).max(major(key));
        } else {
            bare_majors.entry(prefix).or_default().push(major(key));
        }
    }

    // How far the run of releases reaches: from the newest dotted major, through each bare major
    // no more than `BARE_MAJOR_REACH` past the last.
    let reach: HashMap<String, u64> = newest_dotted
        .into_iter()
        .map(|(prefix, newest)| {
            let mut majors = bare_majors.remove(&prefix).unwrap_or_default();
            majors.sort_unstable();
            let reach = majors
                .into_iter()
                .filter(|&m| m > newest)
                .try_fold(newest, |last, m| {
                    (m - last <= BARE_MAJOR_REACH).then_some(m).ok_or(last)
                })
                .unwrap_or_else(|last| last);
            (prefix, reach)
        })
        .collect();
    let is_release = |key: &SortKey<'_>| {
        key.dotted
            || reach
                .get(&key.prefix.to_ascii_lowercase())
                .is_some_and(|&reach| major(key) <= reach)
    };

    candidates
        .into_iter()
        .map(|(key, tag)| (is_release(&key), key.numbers, key.stable, tag))
        .max()
        .map(|(.., tag)| tag.clone())
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
    fn a_date_beside_unprefixed_versions_is_still_a_build_number() {
        let tags = ["1.9.0", "1.10.0", "20240101"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("1.10.0"));
        let tags = ["v1.10.0", "v20240101"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v1.10.0"));
    }

    #[test]
    fn package_tags_count_only_when_a_regex_asks_for_them() {
        let tags = ["v6.0.11", "plugin-legacy@6.1.0"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v6.0.11"));
        let re = fancy_regex::Regex::new("^plugin-legacy@").unwrap();
        assert_eq!(
            latest_tag(&tags, Some(&re)).as_deref(),
            Some("plugin-legacy@6.1.0")
        );
    }

    #[test]
    fn prefixes_compare_without_case() {
        let tags = ["V1.0", "v2"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v2"));
    }

    #[test]
    fn a_run_of_bare_majors_after_a_dotted_one_counts() {
        // gwsw/less tagged one dotted release, `v581.2`, then kept tagging bare majors.
        let mut tags: Vec<String> = (1..=712).map(|n| format!("v{n}")).collect();
        tags.push("v581.2".to_owned());
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v712"));
    }

    #[test]
    fn a_huge_major_does_not_overflow_the_reach() {
        let tags = ["v18446744073709551615.1", "v3"].map(String::from);
        assert_eq!(
            latest_tag(&tags, None).as_deref(),
            Some("v18446744073709551615.1")
        );
    }

    #[test]
    fn a_release_candidate_without_a_dash_is_a_prerelease() {
        // Go tags `go1.25rc1`, then `go1.25.0`.
        let tags = ["go1.24.5", "go1.25rc1", "go1.25rc2", "go1.25.0"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("go1.25.0"));
        let re = fancy_regex::Regex::new("^go").unwrap();
        assert_eq!(latest_tag(&tags, Some(&re)).as_deref(), Some("go1.25.0"));
        // A letter that is not a prerelease marker still names a release: OpenSSL `1_1_1w`.
        let tags = ["OpenSSL_1_1_1v", "OpenSSL_1_1_1w"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("OpenSSL_1_1_1w"));
    }

    #[test]
    fn a_new_major_without_a_dot_still_wins() {
        // systemd tags majors bare and point releases dotted.
        let tags = ["v261.5", "v262", "v262.1", "v263"].map(String::from);
        assert_eq!(latest_tag(&tags, None).as_deref(), Some("v263"));
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
