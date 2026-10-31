//! Finding programs by name or by what they do.
//!
//! `vendor search disk` should turn up `gdu` and `dysk` though neither is called that, so a query
//! matches descriptions too. Names still rank first: someone typing `bat` wants `bat`, not every
//! program whose description mentions bats.

use super::Registry;
use super::schema::Program;

/// How well one query word matched a program, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// The word is a name or alias.
    Exact,
    /// A name or alias starts with it.
    Prefix,
    /// A name or alias contains it.
    Contains,
    /// A word of the description starts with it, or the repository contains it.
    Described,
}

/// Programs matching every word of `query`, best match first and then by name.
///
/// An empty query lists every program by name. Matching ignores case.
#[must_use]
pub fn search<'a>(registry: &'a Registry, query: &str) -> Vec<(&'a str, &'a Program)> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();

    let mut found: Vec<(usize, &str, &Program)> = registry
        .programs()
        .filter_map(|(name, program)| {
            let score = words.iter().try_fold(0, |score, word| {
                rank(name, program, word).map(|rank| score + rank as usize)
            })?;
            Some((score, name, program))
        })
        .collect();

    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    found
        .into_iter()
        .map(|(_, name, program)| (name, program))
        .collect()
}

/// How well `word`, already lowercase, matches one program, or `None` if it does not.
fn rank(name: &str, program: &Program, word: &str) -> Option<Rank> {
    let names: Vec<String> = names(name, program).map(str::to_lowercase).collect();

    if names.iter().any(|candidate| candidate == word) {
        return Some(Rank::Exact);
    }
    if names.iter().any(|candidate| candidate.starts_with(word)) {
        return Some(Rank::Prefix);
    }
    if names.iter().any(|candidate| candidate.contains(word)) {
        return Some(Rank::Contains);
    }

    // Word starts only, so `it` does not match every description containing "edit".
    let described = program.description.as_deref().is_some_and(|description| {
        description
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .any(|token| token.starts_with(word))
    });
    let repository = program
        .repository
        .to_lowercase()
        .split_once("github.com/")
        .is_some_and(|(_, path)| path.contains(word));
    (described || repository).then_some(Rank::Described)
}

/// The canonical name followed by every alias.
fn names<'a>(name: &'a str, program: &'a Program) -> impl Iterator<Item = &'a str> {
    std::iter::once(name).chain(program.aliases.iter().map(String::as_str))
}

/// Canonical names close to `typed`, closest first, for a "did you mean" hint.
///
/// Closeness is edit distance to the name or any alias, with more slack for longer words: one
/// typo in a short name is already a different program. A name that `typed` begins, three
/// letters or more, counts too, so `lazy` suggests `lazygit`.
#[must_use]
pub fn suggest<'a>(registry: &'a Registry, typed: &str) -> Vec<&'a str> {
    const MOST: usize = 3;

    let typed = typed.to_lowercase();
    let length = typed.chars().count();
    let slack = match length {
        0..=4 => 1,
        5..=8 => 2,
        _ => 3,
    };

    let mut close: Vec<(usize, &str)> = registry
        .programs()
        .filter_map(|(name, program)| {
            let distance = names(name, program)
                .map(|candidate| {
                    let candidate = candidate.to_lowercase();
                    if length >= 3 && candidate.starts_with(&typed) {
                        slack
                    } else {
                        distance(&typed, &candidate)
                    }
                })
                .min()?;
            (distance <= slack).then_some((distance, name))
        })
        .collect();

    close.sort_unstable();
    close.into_iter().take(MOST).map(|(_, name)| name).collect()
}

/// The Levenshtein distance between two strings, counted in characters.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();

    for (i, left) in a.chars().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, right) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(left != *right);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use super::{distance, search, suggest};
    use crate::registry::Registry;

    const REGISTRY: &str = r#"
version: 2
programs:
  gdu:
    description: "Fast disk usage analyzer with a terminal UI"
    repository: https://github.com/dundee/gdu
    targets:
      linux-x86_64:
        asset: "{release}/gdu_linux_amd64.tgz"
  dysk:
    description: "Lists mounted disks and their free space"
    repository: https://github.com/Canop/dysk
    targets:
      linux-x86_64:
        asset: "{release}/dysk.zip"
  ripgrep:
    aliases: [rg]
    description: "Fast recursive search through file contents"
    repository: https://github.com/BurntSushi/ripgrep
    targets:
      linux-x86_64:
        asset: "{release}/rg.tar.gz"
  edit:
    description: "Microsoft's small terminal text editor"
    repository: https://github.com/microsoft/edit
    targets:
      linux-x86_64:
        asset: "{release}/edit.tar.gz"
  lazygit:
    description: "Terminal UI for git"
    repository: https://github.com/jesseduffield/lazygit
    targets:
      linux-x86_64:
        asset: "{release}/lazygit.tar.gz"
"#;

    fn names(found: &[(&str, &crate::registry::schema::Program)]) -> Vec<String> {
        found.iter().map(|(name, _)| (*name).to_owned()).collect()
    }

    #[test]
    fn an_empty_query_lists_everything_by_name() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(
            names(&search(&registry, "")),
            ["dysk", "edit", "gdu", "lazygit", "ripgrep"]
        );
    }

    #[test]
    fn a_description_word_finds_programs_named_otherwise() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(names(&search(&registry, "disk")), ["dysk", "gdu"]);
    }

    #[test]
    fn names_rank_above_descriptions() {
        // `git` is lazygit's description and part of its name, which wins.
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(names(&search(&registry, "rg")), ["ripgrep"]);
        assert_eq!(names(&search(&registry, "GIT")), ["lazygit"]);
        assert_eq!(names(&search(&registry, "lazy")), ["lazygit"]);
    }

    #[test]
    fn every_word_has_to_match() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(names(&search(&registry, "disk free")), ["dysk"]);
        assert!(search(&registry, "disk editor").is_empty());
    }

    #[test]
    fn a_description_matches_on_word_starts_only() {
        // "alyzer" is inside gdu's "analyzer", but no word starts with it.
        let registry = Registry::parse(REGISTRY).unwrap();
        assert!(search(&registry, "alyzer").is_empty());
        assert_eq!(names(&search(&registry, "analy")), ["gdu"]);
    }

    #[test]
    fn the_repository_owner_matches_too() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(names(&search(&registry, "burntsushi")), ["ripgrep"]);
    }

    #[test]
    fn a_typo_suggests_the_program_meant() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(suggest(&registry, "ripgreb"), ["ripgrep"]);
        assert_eq!(suggest(&registry, "Lazygti"), ["lazygit"]);
        assert_eq!(suggest(&registry, "rh"), ["ripgrep"]);
        assert_eq!(suggest(&registry, "lazy"), ["lazygit"]);
    }

    #[test]
    fn nothing_close_suggests_nothing() {
        let registry = Registry::parse(REGISTRY).unwrap();
        assert_eq!(suggest(&registry, "react"), Vec::<&str>::new());
        assert_eq!(suggest(&registry, "kubernetes"), Vec::<&str>::new());
    }

    #[test]
    fn distance_counts_characters() {
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("ripgrep", "ripgrep"), 0);
        assert_eq!(distance("dü", "du"), 1);
    }
}
