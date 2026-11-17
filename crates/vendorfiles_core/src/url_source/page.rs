//! Reading a version out of a page: its whole body, one element, or a regex match.

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

#[cfg(test)]
mod tests {
    use super::{extract_version, find_version};

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
}
