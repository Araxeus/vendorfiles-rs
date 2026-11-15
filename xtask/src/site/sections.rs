//! The README read as a list of heading sections.

use std::collections::HashSet;

use anyhow::{Result, bail};

/// One heading and the lines under it, up to the next heading of any level.
#[derive(Debug)]
pub struct Section {
    pub level: usize,
    pub title: String,
    pub slug: String,
    pub body: Vec<String>,
}

/// Splits Markdown at its headings, returning the lines before the first one and the sections.
///
/// Lines inside fenced code are never headings, so a `# comment` in a shell block stays put.
/// Two headings with the same slug are an error, because a link to either would be ambiguous.
pub fn split(markdown: &str) -> Result<(Vec<String>, Vec<Section>)> {
    let mut preamble = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut seen = HashSet::new();
    let mut in_fence = false;

    for line in markdown.split('\n') {
        let heading = if in_fence { None } else { heading(line) };
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }

        if let Some((level, title)) = heading {
            let slug = slug(&title);
            if !seen.insert(slug.clone()) {
                bail!("two headings share the anchor `{slug}`; rename one of them");
            }
            sections.push(Section {
                level,
                title,
                slug,
                body: Vec::new(),
            });
            continue;
        }

        match sections.last_mut() {
            Some(section) => section.body.push(line.to_owned()),
            None => preamble.push(line.to_owned()),
        }
    }
    Ok((preamble, sections))
}

/// The level and title of an ATX heading, without any trailing `<!-- … -->` comment.
fn heading(line: &str) -> Option<(usize, String)> {
    let level = line.bytes().take_while(|&byte| byte == b'#').count();
    let title = line[level..].strip_prefix(' ')?;
    if !(1..=6).contains(&level) {
        return None;
    }
    let title = title.find("<!--").map_or(title, |at| &title[..at]);
    Some((level, title.trim().to_owned()))
}

/// The anchor GitHub gives a heading: lowercased, punctuation dropped, spaces turned to `-`.
pub fn slug(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '-' | '_'))
        .map(|ch| if ch == ' ' { '-' } else { ch })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{slug, split};

    #[test]
    fn fenced_hash_lines_are_not_headings() {
        let (_, sections) = split("## Install\n```bash\n# Linux\ncurl x\n```\n## Next\n").unwrap();
        let titles: Vec<_> = sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Install", "Next"]);
        assert!(sections[0].body.contains(&"# Linux".to_owned()));
    }

    #[test]
    fn slugs_follow_github() {
        assert_eq!(
            slug("Differences from the TypeScript version"),
            "differences-from-the-typescript-version"
        );
        assert_eq!(slug("Commit-Based Versioning"), "commit-based-versioning");
        assert_eq!(slug("JSON Schema"), "json-schema");
    }

    #[test]
    fn toc_comments_leave_the_title() {
        let (_, sections) = split("## Table of Contents <!-- omit from toc -->\n").unwrap();
        assert_eq!(sections[0].title, "Table of Contents");
    }

    #[test]
    fn duplicate_headings_are_an_error() {
        let error = format!("{:#}", split("## A\n## A\n").unwrap_err());
        assert!(error.contains("`a`"), "{error}");
    }
}
