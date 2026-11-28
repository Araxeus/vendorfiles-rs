//! Folders and globs: which files a dependency selects, and where each one lands.
//!
//! A path ending in `/` selects a folder. A path holding `*` or `?` is a glob pattern. Both
//! expand against a listing, so their outputs are known only after it. The lockfile records each
//! expansion as source path → output.

use globset::{GlobBuilder, GlobMatcher};

use crate::error::VendorError;
use crate::model::{FileEntry, FileTarget};
use crate::template::{is_release_path, joined_version, replace_version, version_token};

/// Characters that end the literal part of a glob pattern.
const GLOB_META: &[char] = &['*', '?', '[', '{', '\\'];

/// Whether a repository `files` key names a folder.
#[must_use]
pub fn is_folder(input: &str) -> bool {
    input.ends_with('/') && !is_release_path(input)
}

/// Whether a repository `files` key is a glob pattern.
///
/// Only `*` and `?` make a pattern, since `{` and `[` appear in placeholders and real file names.
/// Inside a pattern, `[...]`, `{a,b}` and `\` escapes work as usual.
#[must_use]
pub fn is_pattern(input: &str) -> bool {
    input.contains(['*', '?']) && !is_release_path(input)
}

/// Whether a repository `files` key selects more than one file.
#[must_use]
pub fn is_selection(input: &str) -> bool {
    is_folder(input) || is_pattern(input)
}

/// A selection that cannot be used, with the key it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    pub key: String,
    pub reason: String,
}

impl Invalid {
    fn new(key: &str, reason: impl Into<String>) -> Self {
        Self {
            key: key.to_owned(),
            reason: reason.into(),
        }
    }

    /// The error to report for dependency `name`.
    #[must_use]
    pub fn named(self, name: &str) -> VendorError {
        VendorError::InvalidSelection {
            name: name.to_owned(),
            key: self.key,
            reason: self.reason,
        }
    }
}

/// One folder or glob, ready to match repository paths.
#[derive(Debug, Clone)]
pub struct Selection {
    /// The key as written, which is also its key in the lockfile.
    pub key: String,
    /// The folder listed for it, with a trailing `/`, or `""` for the root.
    pub base: String,
    destination: String,
    matcher: Option<GlobMatcher>,
}

impl Selection {
    /// The selection `input` makes, or `None` when it names one file.
    ///
    /// `destination` is where the files land, or `None` for a bare entry. A bare folder keeps its
    /// own name and a bare pattern keeps paths below its base. `{version}` and `{tag}` in `input`
    /// are filled in before matching.
    ///
    /// # Errors
    ///
    /// Returns [`Invalid`] for a pattern that does not compile, or an unsafe folder path.
    #[must_use]
    pub fn of(
        input: &str,
        destination: Option<&str>,
        version: &str,
    ) -> Option<Result<Self, Invalid>> {
        let path = replace_version(input, version);
        if path.ends_with('/') {
            return Some(Self::folder(input, &path, destination));
        }
        if path.contains(['*', '?']) {
            return Some(Self::pattern(input, &path, destination));
        }
        None
    }

    fn folder(key: &str, path: &str, destination: Option<&str>) -> Result<Self, Invalid> {
        check_source(path.trim_end_matches('/')).map_err(|reason| Invalid::new(key, reason))?;
        Ok(Self {
            key: key.to_owned(),
            base: path.to_owned(),
            destination: destination.map_or_else(|| folder_name(path), str::to_owned),
            matcher: None,
        })
    }

    fn pattern(key: &str, path: &str, destination: Option<&str>) -> Result<Self, Invalid> {
        let matcher = GlobBuilder::new(path)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .map_err(|error| Invalid::new(key, format!("is not a valid glob ({error})")))?
            .compile_matcher();
        Ok(Self {
            key: key.to_owned(),
            base: literal_base(path),
            destination: destination.unwrap_or("./").to_owned(),
            matcher: Some(matcher),
        })
    }

    /// Whether this is a pattern rather than a folder.
    #[must_use]
    pub const fn is_pattern(&self) -> bool {
        self.matcher.is_some()
    }

    /// Whether `path`, relative to the repository root, belongs to this selection.
    ///
    /// A leading `./`, as some tarballs write their members, is ignored.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        let path = path.strip_prefix("./").unwrap_or(path);
        self.matcher.as_ref().map_or_else(
            || path.starts_with(&self.base),
            |matcher| matcher.is_match(path),
        )
    }

    /// Where `source` lands, relative to the dependency folder unless the destination is absolute.
    ///
    /// # Errors
    ///
    /// Returns [`Invalid`] for an unknown placeholder, a `{parent}` the source does not have, or
    /// a source path that could escape the destination.
    pub fn output(&self, source: &str, version: &str) -> Result<String, Invalid> {
        let source = source.strip_prefix("./").unwrap_or(source);
        check_source(source).map_err(|reason| Invalid::new(&self.key, reason))?;
        let relative = source.strip_prefix(&self.base).unwrap_or(source);
        render(&self.destination, source, relative, version)
            .map_err(|reason| Invalid::new(&self.key, reason))
    }
}

/// One declared member of an archive whose members include folders or patterns.
#[derive(Debug, Clone)]
pub enum Member {
    /// A member named outright, with its output, both with the version filled in.
    Literal { member: String, output: String },
    /// A folder or pattern of members.
    Selected(Selection),
}

/// An archive input whose members include a folder or pattern.
#[derive(Debug, Clone)]
pub struct ArchiveSelection {
    /// The `files` key of the archive: a `{release}/` asset or a URL.
    pub key: String,
    /// Each declared member as written, with what it selects, in declaration order.
    pub members: Vec<(String, Member)>,
}

/// Every folder and pattern a dependency declares: in the repository, and inside archives.
#[derive(Debug, Clone, Default)]
pub struct Selections {
    pub repo: Vec<Selection>,
    pub archives: Vec<ArchiveSelection>,
}

impl Selections {
    /// Both kinds, at `version`.
    ///
    /// # Errors
    ///
    /// Returns [`Invalid`] for a selection that cannot be used.
    pub fn of(files: &[FileEntry], version: &str, repository: bool) -> Result<Self, Invalid> {
        Ok(Self {
            repo: if repository {
                selections(files, version)?
            } else {
                Vec::new()
            },
            archives: archive_selections(files, version)?,
        })
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.repo.is_empty() && self.archives.is_empty()
    }
}

/// What each selection matched: one list per repository selection, then one list per declared
/// member of each archive, empty for a literal member.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matched {
    pub repo: Vec<Vec<String>>,
    pub archives: Vec<Vec<Vec<String>>>,
}

/// Whether an archive member names a folder or a pattern rather than one member.
#[must_use]
pub fn is_member_selection(member: &str) -> bool {
    member.ends_with('/') || member.contains(['*', '?'])
}

/// Every archive among a dependency's files whose members include a folder or pattern.
///
/// # Errors
///
/// Returns [`Invalid`] for a pattern that does not compile, or an unsafe folder path.
pub fn archive_selections(
    files: &[FileEntry],
    version: &str,
) -> Result<Vec<ArchiveSelection>, Invalid> {
    let mut out = Vec::new();
    for spec in crate::model::flatten_files(files) {
        let Some(pairs) = declared_members(&spec.output) else {
            continue;
        };
        if !pairs.iter().any(|(member, _)| is_member_selection(member)) {
            continue;
        }
        let mut members = Vec::with_capacity(pairs.len());
        for (member, output) in pairs {
            let resolved = match Selection::of(&member, output.as_deref(), version) {
                Some(selection) => Member::Selected(selection?),
                None => Member::Literal {
                    member: replace_version(&member, version),
                    output: replace_version(output.as_deref().unwrap_or(&member), version),
                },
            };
            members.push((member, resolved));
        }
        out.push(ArchiveSelection {
            key: spec.input,
            members,
        });
    }
    Ok(out)
}

/// An archive target's members with their written outputs, or `None` for a plain file.
fn declared_members(target: &FileTarget) -> Option<Vec<(String, Option<String>)>> {
    match target {
        FileTarget::Rename(_) | FileTarget::Selected(_) => None,
        FileTarget::ExtractList(list) => Some(list.iter().map(|m| (m.clone(), None)).collect()),
        FileTarget::ExtractMap(map) => Some(
            map.iter()
                .map(|(m, o)| (m.clone(), Some(o.clone())))
                .collect(),
        ),
    }
}

/// Every folder and pattern among a dependency's repository files, in declaration order.
///
/// # Errors
///
/// Returns [`Invalid`] for a selection with an archive-style target or a pattern that does not
/// compile.
pub fn selections(files: &[FileEntry], version: &str) -> Result<Vec<Selection>, Invalid> {
    let mut out = Vec::new();
    for entry in files {
        match entry {
            FileEntry::Simple(input) if is_selection(input) => {
                out.extend(Selection::of(input, None, version).transpose()?);
            }
            FileEntry::Simple(_) => {}
            FileEntry::Mapped(map) => {
                for (input, target) in map.iter().filter(|(input, _)| is_selection(input)) {
                    let FileTarget::Rename(destination) = target else {
                        return Err(Invalid::new(
                            input,
                            "selects several files, so it takes one destination",
                        ));
                    };
                    out.extend(Selection::of(input, Some(destination), version).transpose()?);
                }
            }
        }
    }
    Ok(out)
}

/// The name a bare folder key saves under: its last component, as a folder.
fn folder_name(input: &str) -> String {
    let trimmed = input.trim_end_matches('/');
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    format!("{last}/")
}

/// The longest literal folder prefix of `pattern`, with a trailing `/`.
fn literal_base(pattern: &str) -> String {
    let literal = pattern
        .find(GLOB_META)
        .map_or(pattern, |index| &pattern[..index]);
    literal
        .rfind('/')
        .map_or_else(String::new, |index| literal[..=index].to_owned())
}

/// Rejects a repository path that could write outside the folder it is placed in.
///
/// Git itself forbids most of these, so this guards against a hostile or odd listing.
fn check_source(path: &str) -> Result<(), String> {
    let bad = path.split('/').find(|component| {
        component.is_empty()
            || *component == "."
            || *component == ".."
            || component.contains('\\')
            || (cfg!(windows) && component.contains(':'))
    });
    bad.map_or(Ok(()), |component| {
        Err(format!(
            "selects '{path}', whose part '{component}' is not a safe file name"
        ))
    })
}

/// Renders `destination` for one source file.
///
/// A destination ending in `/`, or empty, keeps the path below the selection's base. Anything
/// else is a filename template. Substitution is a single pass, so placeholder-like text that
/// comes from a file name stays literal.
fn render(
    destination: &str,
    source: &str,
    relative: &str,
    version: &str,
) -> Result<String, String> {
    let destination = match destination {
        "" | "." => "./",
        other => other,
    };
    let expanded = expand(destination, source, relative, version)?;
    if destination.ends_with('/') {
        let prefix = expanded.strip_prefix("./").unwrap_or(&expanded);
        return Ok(format!("{prefix}{relative}"));
    }
    if expanded.is_empty() || expanded.ends_with('/') {
        return Err(format!(
            "renders '{destination}' to '{expanded}', which names no file"
        ));
    }
    Ok(expanded)
}

/// Replaces every placeholder in `template` in one pass.
fn expand(template: &str, source: &str, relative: &str, version: &str) -> Result<String, String> {
    let filename = source.rsplit('/').next().unwrap_or(source);
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return Ok(out);
        };
        let name = &after[..close];
        match name {
            "path" => out.push_str(relative),
            "filename" => out.push_str(filename),
            "stem" => out.push_str(split_extension(filename).0),
            "ext" => out.push_str(split_extension(filename).1),
            "parent" => out.push_str(parent(source).ok_or_else(|| {
                format!("selects '{source}', which has no parent folder for {{parent}}")
            })?),
            "version" => out.push_str(version_token(version)),
            joined if joined.starts_with("version:") => {
                out.push_str(&joined_version(version, &joined["version:".len()..]));
            }
            "tag" => out.push_str(version),
            other => return Err(format!("uses an unknown placeholder {{{other}}}")),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A file name's stem and extension, split at the last dot.
///
/// A leading dot belongs to the stem, so `.bashrc` has no extension.
fn split_extension(filename: &str) -> (&str, &str) {
    match filename.rfind('.') {
        Some(index) if index > 0 => (&filename[..index], &filename[index + 1..]),
        _ => (filename, ""),
    }
}

/// The name of the folder holding `source`.
fn parent(source: &str) -> Option<&str> {
    let (folder, _) = source.rsplit_once('/')?;
    folder.rsplit('/').next()
}

#[cfg(test)]
mod tests {
    use super::{Selection, is_folder, is_pattern, literal_base, selections, split_extension};
    use crate::model::FileEntry;

    fn files(json: &str) -> Vec<FileEntry> {
        serde_json::from_str(json).expect("valid files")
    }

    fn glob(pattern: &str, destination: &str) -> Selection {
        Selection::pattern(pattern, pattern, Some(destination)).expect("a valid pattern")
    }

    #[test]
    fn only_a_trailing_slash_makes_a_folder() {
        assert!(is_folder("skills/fonts/"));
        assert!(!is_folder("skills/fonts"));
        assert!(!is_folder("{release}/assets/"));
    }

    #[test]
    fn the_base_is_the_literal_folder_before_the_first_wildcard() {
        assert_eq!(literal_base("dist/**/*.js"), "dist/");
        assert_eq!(literal_base("a/b/c*.md"), "a/b/");
        assert_eq!(literal_base("**/*.md"), "");
        assert_eq!(literal_base("*.md"), "");
        assert_eq!(literal_base("docs/README.md"), "docs/");
    }

    #[test]
    fn a_single_star_stays_inside_one_folder() {
        let selection = glob("dist/*", "./");
        assert!(selection.matches("dist/a.js"));
        assert!(!selection.matches("dist/sub/a.js"));
    }

    #[test]
    fn a_double_star_reaches_every_depth() {
        let selection = glob("dist/**/*.js", "./");
        assert!(selection.matches("dist/a.js"));
        assert!(selection.matches("dist/sub/deep/a.js"));
        assert!(!selection.matches("dist/a.ts"));
        assert!(!selection.matches("other/a.js"));
    }

    #[test]
    fn matching_is_case_sensitive_and_includes_dotfiles() {
        let selection = glob("conf/*", "./");
        assert!(selection.matches("conf/.editorconfig"));
        assert!(!glob("*.MD", "./").matches("README.md"));
    }

    #[test]
    fn a_trailing_slash_destination_keeps_the_path_below_the_base() {
        let selection = glob("dist/**/*.js", "scripts/");
        assert_eq!(
            selection.output("dist/utils/a.js", "v1").unwrap(),
            "scripts/utils/a.js"
        );
        assert_eq!(
            glob("dist/**/*.js", "./")
                .output("dist/a.js", "v1")
                .unwrap(),
            "a.js"
        );
        assert_eq!(
            glob("dist/**/*.js", "").output("dist/a.js", "v1").unwrap(),
            "a.js"
        );
    }

    #[test]
    fn templates_fill_every_placeholder() {
        let source = "skills/pdf/a.tar.gz";
        let render = |destination: &str| glob("skills/**", destination).output(source, "v1.2.3");
        assert_eq!(render("{path}").unwrap(), "pdf/a.tar.gz");
        assert_eq!(render("out/{filename}").unwrap(), "out/a.tar.gz");
        assert_eq!(render("{stem}-{ext}").unwrap(), "a.tar-gz");
        assert_eq!(render("{parent}.md").unwrap(), "pdf.md");
        assert_eq!(
            render("{version}/{tag}/{filename}").unwrap(),
            "1.2.3/v1.2.3/a.tar.gz"
        );
    }

    #[test]
    #[expect(clippy::literal_string_with_formatting_args, reason = "placeholders")]
    fn a_version_joiner_rejoins_the_parts() {
        let selection = glob("skills/**", "{version:_}-{version:}");
        assert_eq!(
            selection.output("skills/a.md", "v1.2.3").unwrap(),
            "1_2_3-123"
        );
    }

    #[test]
    fn substituted_text_is_never_expanded_again() {
        let selection = glob("**", "{filename}");
        assert_eq!(
            selection.output("{parent}.txt", "v1").unwrap(),
            "{parent}.txt"
        );
    }

    #[test]
    fn a_dotfile_and_an_extensionless_file_have_no_extension() {
        assert_eq!(split_extension(".bashrc"), (".bashrc", ""));
        assert_eq!(split_extension("Makefile"), ("Makefile", ""));
        assert_eq!(split_extension("a.tar.gz"), ("a.tar", "gz"));
    }

    #[test]
    fn unknown_placeholders_and_a_missing_parent_are_errors() {
        assert!(glob("*.md", "{nope}.md").output("README.md", "v1").is_err());
        assert!(
            glob("*.md", "{parent}.md")
                .output("README.md", "v1")
                .is_err()
        );
        assert!(glob("*.md", "out/").output("README.md", "v1").is_ok());
    }

    #[test]
    fn a_template_that_names_no_file_is_an_error() {
        assert!(glob("*.md", "{nope").output("README.md", "v1").is_ok());
        assert!(
            glob("a/*.md", "{ext}")
                .output("a/Makefile.md", "v1")
                .is_ok()
        );
        assert!(glob("a/*", "{ext}").output("a/Makefile", "v1").is_err());
    }

    #[test]
    fn unsafe_source_paths_are_refused() {
        let selection = glob("**", "./");
        assert!(selection.output("a/../b", "v1").is_err());
        assert!(selection.output("a\\b", "v1").is_err());
        assert!(selection.output("a//b", "v1").is_err());
    }

    #[test]
    fn folders_and_patterns_are_picked_out_of_files() {
        let found = selections(
            &files(
                r#"["LICENSE", "a/fonts/", {"b/icons/": "icons/", "README.md": "R.md", "skills/**/SKILL.md": "{parent}.md"}]"#,
            ),
            "v1",
        )
        .unwrap();
        let summary: Vec<_> = found
            .iter()
            .map(|s| (s.key.as_str(), s.base.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                ("a/fonts/", "a/fonts/"),
                ("b/icons/", "b/icons/"),
                ("skills/**/SKILL.md", "skills/")
            ]
        );
        assert_eq!(
            found[0].output("a/fonts/x/y.ttf", "v1").unwrap(),
            "fonts/x/y.ttf"
        );
        assert_eq!(
            found[1].output("b/icons/i.svg", "v1").unwrap(),
            "icons/i.svg"
        );
        assert!(found[0].matches("a/fonts/x.ttf"));
        assert!(!found[0].matches("a/fontsx.ttf"));
    }

    #[test]
    fn a_bare_pattern_keeps_paths_below_the_base() {
        let found = selections(&files(r#"["dist/**/*.js"]"#), "v1").unwrap();
        assert_eq!(found[0].output("dist/u/a.js", "v1").unwrap(), "u/a.js");
    }

    #[test]
    fn only_a_star_or_question_mark_makes_a_pattern() {
        assert!(is_pattern("dist/*.js"));
        assert!(is_pattern("a?.txt"));
        assert!(!is_pattern("app/[id].tsx"));
        assert!(!is_pattern("docs/{README,LICENSE}.md"));
        assert!(!is_pattern("{release}/tool-*.zip"));
    }

    #[test]
    fn an_escaped_star_matches_itself() {
        let selection = glob("a\\*b", "./");
        assert!(selection.matches("a*b"));
        assert!(!selection.matches("axb"));
    }

    #[test]
    fn the_version_is_filled_in_before_matching() {
        let selection = Selection::of("tool-{version}/bin/*", None, "v1.2.0")
            .unwrap()
            .unwrap();
        assert!(selection.matches("tool-1.2.0/bin/x"));
        assert_eq!(selection.base, "tool-1.2.0/bin/");
    }

    #[test]
    fn a_selection_with_an_archive_target_is_refused() {
        assert!(selections(&files(r#"[{"a/": ["x"]}]"#), "v1").is_err());
        assert!(selections(&files(r#"[{"a/*": {"x": "y"}}]"#), "v1").is_err());
    }

    #[test]
    fn an_invalid_pattern_is_refused() {
        assert!(selections(&files(r#"["a/[b*"]"#), "v1").is_err());
    }
}
