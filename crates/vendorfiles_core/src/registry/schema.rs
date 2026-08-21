//! The registry file's wire format.
//!
//! Both structs refuse unknown fields. A registry that grows a key this build does not understand
//! is then a loud error rather than something silently ignored — which matters because the file is
//! remote data that decides what gets downloaded. The cost is that adding a field means bumping
//! [`SUPPORTED_VERSION`], and that is the right way round for a trust boundary.
//!
//! Note what is *absent*: there is no `vendorFolder`. A registry entry says what to fetch; where
//! it lands is the config's business.

use indexmap::IndexMap;
use serde::Deserialize;

/// The only schema version this build understands.
pub const SUPPORTED_VERSION: u32 = 1;

/// The whole file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Bumped whenever an entry gains a field, so an older binary can say so plainly.
    pub version: u32,
    /// Programs by canonical name, which is also the config key `add` writes.
    #[serde(default)]
    pub programs: IndexMap<String, Program>,
}

/// One installable program.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    /// Other names that resolve here.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The GitHub repository releases come from.
    pub repository: String,
    /// The asset pattern shared by every target, for projects that name assets by target triple.
    #[serde(default)]
    pub asset: Option<String>,
    /// The path inside that asset, shared by every target.
    ///
    /// Omitted when the asset *is* the executable — plenty of projects publish a bare binary
    /// rather than an archive.
    #[serde(default)]
    pub member: Option<String>,
    /// The name to save it under, when the basename is not what you want to type.
    ///
    /// `ox-macos` and `shfmt_v3.13.1_linux_amd64` are assets; `ox` and `shfmt` are commands.
    #[serde(rename = "as", default)]
    pub output: Option<String>,
    /// What to fetch per host, keyed `{os}-{arch}`.
    pub targets: IndexMap<String, Target>,
}

/// What one host gets.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Target {
    /// The target triple to substitute into the program's shared `asset` and `member`.
    Triple(String),
    /// A spelled-out asset, for projects whose names follow no pattern.
    Explicit(Explicit),
}

/// One host's asset, named outright.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explicit {
    /// The release asset, including the `{release}/` prefix.
    pub asset: String,
    /// The path to the executable inside it, or omitted when the asset is the executable.
    #[serde(default)]
    pub member: Option<String>,
    /// The name to save it under; defaults to the basename of whichever of the two is used.
    #[serde(rename = "as", default)]
    pub output: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{Document, SUPPORTED_VERSION, Target};

    const COMPACT: &str = r#"
version: 1
programs:
  fd:
    aliases: [fdfind]
    repository: https://github.com/sharkdp/fd
    asset: "{release}/fd-v{version}-{target}{ext}"
    member: "fd-v{version}-{target}/fd{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
"#;

    fn parse(text: &str) -> Result<Document, serde_yaml_ng::Error> {
        serde_yaml_ng::from_str(text)
    }

    #[test]
    fn the_compact_form_parses() {
        let document = parse(COMPACT).expect("valid");
        assert_eq!(document.version, SUPPORTED_VERSION);
        let program = &document.programs["fd"];
        assert_eq!(program.aliases, ["fdfind"]);
        assert!(matches!(
            program.targets["windows-x86_64"],
            Target::Triple(ref triple) if triple == "x86_64-pc-windows-msvc"
        ));
    }

    #[test]
    fn the_explicit_form_parses() {
        let document = parse(
            r#"
version: 1
programs:
  fzf:
    repository: https://github.com/junegunn/fzf
    targets:
      windows-x86_64:
        asset: "{release}/fzf-{version}-windows_amd64.zip"
        member: "fzf.exe"
"#,
        )
        .expect("valid");
        let Target::Explicit(explicit) = &document.programs["fzf"].targets["windows-x86_64"] else {
            panic!("expected the explicit form");
        };
        assert_eq!(explicit.member.as_deref(), Some("fzf.exe"));
    }

    #[test]
    fn a_field_this_build_does_not_know_is_refused() {
        // The whole point of the trust boundary: a registry cannot smuggle in a key that an old
        // binary would ignore. `vendorFolder` is the one that would matter.
        let refused = parse(
            r#"
version: 1
programs:
  sneaky:
    repository: https://github.com/example/sneaky
    vendorFolder: "C:/Windows/System32"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
"#,
        );
        assert!(refused.is_err(), "unknown fields must be refused");
    }

    #[test]
    fn a_missing_repository_is_refused() {
        let refused = parse(
            r"
version: 1
programs:
  nameless:
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
",
        );
        assert!(refused.is_err());
    }

    #[test]
    fn an_empty_registry_is_valid() {
        let document = parse("version: 1\n").expect("valid");
        assert!(document.programs.is_empty());
    }
}
