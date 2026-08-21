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
    /// Which tags count, for repositories that publish more than one train of releases.
    ///
    /// `bitwarden/sdk` tags `bws-v2.1.0` alongside `rust-v2.1.0` and `python-v2.1.0`, so without
    /// this the newest release is one with no assets at all.
    #[serde(rename = "releaseRegex", default)]
    pub release_regex: Option<String>,
    /// A path in the repository, for something that is not a release asset at all.
    ///
    /// Mutually exclusive with `asset`/`targets`: a repository file is the same on every platform.
    #[serde(default)]
    pub path: Option<String>,
    /// Track that file by commit rather than by tag.
    #[serde(rename = "hashVersionFile", default)]
    pub hash_version_file: Option<bool>,
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
    /// What to fetch per host, keyed `{os}-{arch}`. Absent for a repository file.
    #[serde(default)]
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

    /// A file that ships at the repository root, two directories above this crate.
    fn repository_file(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()))
    }

    /// The published JSON Schema, which editors validate `registry.yml` against.
    fn published_schema() -> serde_json::Value {
        serde_json::from_str(&repository_file("registry.schema.json")).expect("valid JSON")
    }

    /// The same schema compiled as Draft 2020-12.
    ///
    /// Building it is itself a test: a `$ref` that resolves to nothing, or a keyword the draft
    /// does not define, fails here rather than silently doing nothing in an editor.
    fn compiled_schema() -> jsonschema::Validator {
        jsonschema::draft202012::new(&published_schema())
            .expect("registry.schema.json compiles as Draft 2020-12")
    }

    /// A registry document as the validator wants it: YAML is JSON with a friendlier syntax, so
    /// the fixtures stay in the form a contributor would actually write.
    fn as_json(document: &str) -> serde_json::Value {
        serde_yaml_ng::from_str(document).expect("valid YAML")
    }

    /// Every way one document offends the schema, for an assertion message worth reading.
    fn why_invalid(validator: &jsonschema::Validator, document: &serde_json::Value) -> String {
        validator
            .iter_errors(document)
            .map(|error| format!("  {}: {error}", error.instance_path()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The fields serde will accept, taken from its own complaint about one it will not.
    ///
    /// Asking serde rather than repeating the list keeps this honest: add a field to the struct
    /// and this set grows on its own, so the schema cannot quietly fall behind.
    fn fields_serde_accepts<T: serde::de::DeserializeOwned>(document: &str) -> Vec<String> {
        let error = serde_yaml_ng::from_str::<T>(document)
            .err()
            .expect("the probe field must be rejected")
            .to_string();
        // serde words the list three ways depending on its length — "expected one of `a`, `b`",
        // "expected `a` or `b`", "expected `a`" — so take every backticked name after "expected"
        // rather than matching one phrasing.
        let (_, listed) = error
            .split_once("expected ")
            .unwrap_or_else(|| panic!("unexpected serde wording: {error}"));
        let mut fields: Vec<String> = listed
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect();
        fields.sort();
        assert!(!fields.is_empty(), "no fields parsed out of: {error}");
        fields
    }

    fn schema_properties(schema: &serde_json::Value, pointer: &str) -> Vec<String> {
        schema
            .pointer(pointer)
            .unwrap_or_else(|| panic!("no properties at {pointer}"))
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn the_published_schema_lists_exactly_the_fields_serde_accepts() {
        let schema = published_schema();

        for (what, probe, pointer) in [
            (
                "Document",
                "version: 1
nope: 1
",
                "/properties",
            ),
            (
                "Program",
                "repository: r
targets: {}
nope: 1
",
                "/$defs/program/properties",
            ),
            (
                "Explicit",
                "asset: a
nope: 1
",
                "/$defs/explicit/properties",
            ),
        ] {
            let expected = match what {
                "Document" => fields_serde_accepts::<Document>(probe),
                "Program" => fields_serde_accepts::<super::Program>(probe),
                _ => fields_serde_accepts::<super::Explicit>(probe),
            };
            let mut published = schema_properties(&schema, pointer);
            published.sort();
            assert_eq!(
                published, expected,
                "registry.schema.json is out of step with `{what}`"
            );
        }
    }

    #[test]
    fn the_schema_agrees_with_this_build_about_the_version() {
        let schema = published_schema();
        let declared = schema
            .pointer("/properties/version/const")
            .and_then(serde_json::Value::as_u64)
            .expect("a version constant");
        assert_eq!(
            u32::try_from(declared).unwrap(),
            SUPPORTED_VERSION,
            "the schema documents a format this build does not support"
        );
    }

    #[test]
    fn an_empty_registry_is_valid() {
        let document = parse("version: 1\n").expect("valid");
        assert!(document.programs.is_empty());
    }

    /// The file this repository actually ships, against the schema that ships beside it. Nothing
    /// else proves the two have ever met.
    #[test]
    fn the_shipped_registry_satisfies_the_published_schema() {
        let text = repository_file("registry.yml");
        parse(&text).expect("registry.yml parses");
        let registry = as_json(&text);
        let validator = compiled_schema();
        assert!(
            validator.is_valid(&registry),
            "registry.yml does not satisfy registry.schema.json:\n{}",
            why_invalid(&validator, &registry)
        );
    }

    /// A repository file takes no `targets`, so the `allOf` rule has to let it through — and the
    /// shipped registry has no such entry to prove it with.
    const PATH_FORM: &str = r"
version: 1
programs:
  starship-preset:
    repository: https://github.com/starship/starship
    path: docs/public/presets/toml/nerd-font-symbols.toml
    hashVersionFile: true
    as: starship.toml
";

    /// The compact form and the path form, from the fixtures above; the explicit form arrives
    /// with `registry.yml`, where `fzf` names an asset per host.
    #[test]
    fn the_compact_and_path_forms_satisfy_the_published_schema() {
        let validator = compiled_schema();
        for (what, document) in [("the compact form", COMPACT), ("the path form", PATH_FORM)] {
            let value = as_json(document);
            assert!(
                validator.is_valid(&value),
                "the schema refuses {what}:\n{}",
                why_invalid(&validator, &value)
            );
            parse(document).unwrap_or_else(|error| panic!("{what} must parse: {error}"));
        }
    }

    /// One malformed entry per rule the schema enforces, each named after the rule it breaks.
    ///
    /// Naming them one by one is the point: a failure says which constraint stopped holding
    /// rather than only that something somewhere loosened.
    const REFUSED: &[(&str, &str)] = &[
        (
            "a repository file that also names an asset",
            r#"
version: 1
programs:
  both:
    repository: https://github.com/example/both
    path: config.toml
    asset: "{release}/both{ext}"
"#,
        ),
        (
            "a repository file that also names a member",
            r"
version: 1
programs:
  both:
    repository: https://github.com/example/both
    path: config.toml
    member: inner/file
",
        ),
        (
            "a repository file that also lists targets",
            r"
version: 1
programs:
  both:
    repository: https://github.com/example/both
    path: config.toml
    targets:
      linux-x86_64: x86_64-unknown-linux-gnu
",
        ),
        (
            "a release entry with no targets at all",
            r"
version: 1
programs:
  nowhere:
    repository: https://github.com/example/nowhere
",
        ),
        (
            "an empty targets map",
            r"
version: 1
programs:
  nowhere:
    repository: https://github.com/example/nowhere
    targets: {}
",
        ),
        (
            "a host key that is not `{os}-{arch}`",
            r"
version: 1
programs:
  shouty:
    repository: https://github.com/example/shouty
    targets:
      Windows-X86_64: x86_64-pc-windows-msvc
",
        ),
        (
            "a repository that is not a GitHub URL",
            r"
version: 1
programs:
  elsewhere:
    repository: https://gitlab.com/example/elsewhere
    targets:
      linux-x86_64: x86_64-unknown-linux-gnu
",
        ),
        (
            "an explicit target with no asset",
            r"
version: 1
programs:
  assetless:
    repository: https://github.com/example/assetless
    targets:
      linux-x86_64:
        member: inner/bin
",
        ),
        (
            "a field this build does not know",
            r#"
version: 1
programs:
  sneaky:
    repository: https://github.com/example/sneaky
    vendorFolder: "C:/Windows/System32"
    targets:
      linux-x86_64: x86_64-unknown-linux-gnu
"#,
        ),
        (
            "a program name `add` could not write to a config",
            r"
version: 1
programs:
  -dashed:
    repository: https://github.com/example/dashed
    targets:
      linux-x86_64: x86_64-unknown-linux-gnu
",
        ),
        (
            "a format version this build does not support",
            r"
version: 2
programs: {}
",
        ),
    ];

    /// What the schema is for: telling a contributor their entry is wrong before CI does.
    #[test]
    fn the_published_schema_refuses_malformed_entries() {
        let validator = compiled_schema();
        for (what, document) in REFUSED {
            assert!(
                !validator.is_valid(&as_json(document)),
                "the schema accepts {what}"
            );
        }
    }
}
