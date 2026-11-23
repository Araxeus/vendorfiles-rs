//! Reading a version out of a page: its whole body, one element, a JSON value, or a regex match.

use scraper::{Html, Selector};

/// The version in `text`.
///
/// Without a regex the trimmed text is the version. With one, it is capture group 1, or the
/// whole match when the regex has no groups. Either way a version is non-empty and holds no
/// whitespace, so an HTML page or a sentence is never mistaken for one.
#[must_use]
pub fn extract_version(text: &str, regex: Option<&fancy_regex::Regex>) -> Option<String> {
    let found = match regex {
        None => text,
        Some(re) => {
            let captures = re.captures(text).ok()??;
            let group = usize::from(re.captures_len() > 1);
            captures.get(group)?.as_str()
        }
    }
    .trim();
    (!found.is_empty() && !found.contains(char::is_whitespace)).then(|| found.to_owned())
}

/// The version in a page body, per `versionSelector` and `versionRegex`.
///
/// With a selector, the first matching element's text is searched. A regex that finds nothing
/// there also tries the element's HTML, which reaches versions kept in attributes, like
/// `<a href="/dl/v1.2.3">`.
#[must_use]
pub fn find_version(
    body: &str,
    selector: Option<&Selector>,
    regex: Option<&fancy_regex::Regex>,
) -> Option<String> {
    let Some(selector) = selector else {
        return extract_version(body, regex);
    };
    let page = Html::parse_document(body);
    let element = page.select(selector).next()?;
    let text = element.text().collect::<String>();
    extract_version(&text, regex)
        .or_else(|| regex.and_then(|_| extract_version(&element.html(), regex)))
}

/// The version at `pointer` in a JSON body, per `versionJsonPointer` and `versionRegex`.
///
/// A string or number there is read like a page's text. An array is a list of versions, and the
/// latest one wins, ranked like the tags of a `.git` `versionUrl`. There `versionRegex` says
/// which versions count.
///
/// A `*` segment stands for every item of an array or value of an object, so `/*/version`
/// gathers each entry's version into one list.
#[must_use]
pub fn find_json_version(
    body: &str,
    pointer: &str,
    regex: Option<&fancy_regex::Regex>,
) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let found = select(&json, pointer);
    let list = match found.as_slice() {
        [serde_json::Value::Array(items)] => items.iter().collect(),
        [single] if !pointer.split('/').any(|segment| segment == "*") => {
            return extract_version(&scalar(single)?, regex);
        }
        _ => found,
    };
    let versions: Vec<String> = list
        .into_iter()
        .filter_map(scalar)
        .filter(|version| super::is_plausible_version(version))
        .collect();
    super::tags::latest_tag(&versions, regex)
}

/// The values `pointer` reaches in `json`, where a `*` segment reaches every child.
fn select<'a>(json: &'a serde_json::Value, pointer: &str) -> Vec<&'a serde_json::Value> {
    let mut found = vec![json];
    for segment in pointer.split('/').skip(1) {
        found = found
            .into_iter()
            .flat_map(|value| -> Vec<&serde_json::Value> {
                match (segment, value) {
                    ("*", serde_json::Value::Array(items)) => items.iter().collect(),
                    ("*", serde_json::Value::Object(map)) => map.values().collect(),
                    _ => value.pointer(&format!("/{segment}")).into_iter().collect(),
                }
            })
            .collect();
    }
    found
}

/// A JSON string or number as text.
fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_version, find_json_version, find_version};

    #[test]
    fn a_plain_body_is_the_version() {
        assert_eq!(
            extract_version("  v1.2.3\n", None).as_deref(),
            Some("v1.2.3")
        );
    }

    #[test]
    fn a_page_without_a_regex_is_not_a_version() {
        assert_eq!(
            extract_version("<html>\n<body>404</body>\n</html>", None),
            None
        );
        assert_eq!(extract_version("1.2.3 (stable)", None), None);
        assert_eq!(extract_version("   ", None), None);
    }

    #[test]
    fn a_regex_takes_group_one_or_the_whole_match() {
        let grouped = fancy_regex::Regex::new(r"v(\d+\.\d+\.\d+)").unwrap();
        assert_eq!(
            extract_version("Release v3.4.5 notes", Some(&grouped)).as_deref(),
            Some("3.4.5")
        );
        let bare = fancy_regex::Regex::new(r"\d+\.\d+").unwrap();
        assert_eq!(
            extract_version("build 7.1 ok", Some(&bare)).as_deref(),
            Some("7.1")
        );
        assert_eq!(extract_version("nothing here", Some(&bare)), None);
    }

    fn sel(css: &str) -> scraper::Selector {
        scraper::Selector::parse(css).unwrap()
    }

    #[test]
    fn a_selector_picks_the_first_matching_element_text() {
        let page = r#"<ul class="rel"><li><h2>v1.2.3</h2></li><li><h2>v1.2.2</h2></li></ul>"#;
        assert_eq!(
            find_version(page, Some(&sel("ul.rel > li h2")), None).as_deref(),
            Some("v1.2.3")
        );
        assert_eq!(
            find_version(page, Some(&sel("li:nth-child(2) h2")), None).as_deref(),
            Some("v1.2.2")
        );
        assert_eq!(find_version(page, Some(&sel("h3")), None), None);
    }

    #[test]
    fn a_regex_reads_the_selected_text_then_its_html() {
        let page =
            r#"<h2>v1.2.3 <small>stable</small></h2><a class="dl" href="/dl/v4.5.6">Download</a>"#;
        let re = fancy_regex::Regex::new(r"v(\d+\.\d+\.\d+)").unwrap();
        assert_eq!(find_version(page, Some(&sel("h2")), None), None);
        assert_eq!(
            find_version(page, Some(&sel("h2")), Some(&re)).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            find_version(page, Some(&sel("a.dl")), Some(&re)).as_deref(),
            Some("4.5.6")
        );
    }

    #[test]
    fn a_pointer_reads_one_json_value() {
        let search = r#"{"totalHits":1,"data":[{"id":"pkg","version":"23.1.5","versions":[{"version":"8.0.0"}]}]}"#;
        assert_eq!(
            find_json_version(search, "/data/0/version", None).as_deref(),
            Some("23.1.5")
        );
        assert_eq!(
            find_json_version(r#"{"v":"Release v1.2.3"}"#, "/v", None),
            None
        );
        let re = fancy_regex::Regex::new(r"v(\d+\.\d+\.\d+)").unwrap();
        assert_eq!(
            find_json_version(r#"{"v":"Release v1.2.3"}"#, "/v", Some(&re)).as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            find_json_version(r#"{"build":42}"#, "/build", None).as_deref(),
            Some("42")
        );
    }

    #[test]
    fn a_pointer_to_a_list_picks_the_latest_stable_version() {
        let index =
            r#"{"versions":["8.0.0","18.1.3","18.1.3.2","23.1.5","24.0.0-preview.1","9.0.0"]}"#;
        assert_eq!(
            find_json_version(index, "/versions", None).as_deref(),
            Some("23.1.5")
        );
        assert_eq!(
            find_json_version(r#"["v1.9.0","v1.10.0"]"#, "", None).as_deref(),
            Some("v1.10.0")
        );
        let re = fancy_regex::Regex::new(r"^18\.").unwrap();
        assert_eq!(
            find_json_version(index, "/versions", Some(&re)).as_deref(),
            Some("18.1.3.2")
        );
    }

    #[test]
    fn a_wildcard_gathers_a_version_from_every_entry() {
        let zig = r#"{"master":{"version":"0.18.0-dev.1+abc"},"0.17.1":{"version":"0.17.1"},"0.9.1":{"version":"0.9.1"}}"#;
        assert_eq!(
            find_json_version(zig, "/*/version", None).as_deref(),
            Some("0.17.1")
        );
        let go = r#"[{"version":"go1.26.11"},{"version":"go1.27.4"},{"version":"go1.28rc1"}]"#;
        assert_eq!(
            find_json_version(go, "/*/version", None).as_deref(),
            Some("go1.27.4")
        );
        let nested = r#"{"releases":[{"v":"1.0.0"},{"v":"1.1.0"},{"nope":true}]}"#;
        assert_eq!(
            find_json_version(nested, "/releases/*/v", None).as_deref(),
            Some("1.1.0")
        );
        assert_eq!(
            find_json_version(r#"{"only":{"version":"2.0.0"}}"#, "/*/version", None).as_deref(),
            Some("2.0.0")
        );
    }

    #[test]
    fn a_pointer_that_finds_nothing_usable_is_a_miss() {
        assert_eq!(find_json_version("<html></html>", "/v", None), None);
        assert_eq!(find_json_version(r#"{"v":"1.0"}"#, "/w", None), None);
        assert_eq!(find_json_version(r#"{"v":{"x":"1.0"}}"#, "/v", None), None);
        assert_eq!(
            find_json_version(r#"{"v":{"x":"1.0"}}"#, "/v/*/y", None),
            None
        );
        assert_eq!(
            find_json_version(r#"{"v":["../x","{}"]}"#, "/v", None),
            None
        );
    }
}
