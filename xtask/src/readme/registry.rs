//! The README's list of programs `vendor add` knows by name, generated from `registry.yml`.
//!
//! The list sits between `<!-- registry -->` and `<!-- /registry -->`, folded into a `<details>`
//! so it costs an already long README one line until someone opens it.

use anyhow::{Context, Result, anyhow, bail};
use vendorfiles_core::registry::Registry;
use vendorfiles_core::registry::schema::Program;

const OPEN: &str = "<!-- registry -->";
const CLOSE: &str = "<!-- /registry -->";

/// The platforms a full entry covers, as `{os}-{arch}` host keys spell them.
const OSES: [(&str, &str); 3] = [
    ("windows", "Windows"),
    ("macos", "macOS"),
    ("linux", "Linux"),
];
const ARCHES: [(&str, &str); 2] = [("x86_64", "x64"), ("aarch64", "arm64")];

/// Rewrites the program list from `registry`, leaving the rest of the file alone.
pub fn regenerate(markdown: &str, registry: &str) -> Result<String> {
    let registry = Registry::parse(registry).context("reading registry.yml")?;
    let lines: Vec<&str> = markdown.split('\n').collect();

    let open = lines
        .iter()
        .position(|line| line.trim() == OPEN)
        .ok_or_else(|| anyhow!("README.md has no `{OPEN}` region for the program list"))?;
    let close = lines[open + 1..]
        .iter()
        .position(|line| line.trim() == CLOSE)
        .ok_or_else(|| {
            anyhow!(
                "the region at line {} is never closed by `{CLOSE}`",
                open + 1
            )
        })?
        + open
        + 1;
    if lines[close + 1..].iter().any(|line| line.trim() == OPEN) {
        bail!("README.md has more than one `{OPEN}` region");
    }

    let mut out: Vec<String> = lines[..=open]
        .iter()
        .map(|line| (*line).to_owned())
        .collect();
    out.extend(table(&registry));
    out.extend(lines[close..].iter().map(|line| (*line).to_owned()));
    Ok(out.join("\n"))
}

/// The collapsed table, one row per program, by name.
fn table(registry: &Registry) -> Vec<String> {
    let mut programs: Vec<(&str, &Program)> = registry.programs().collect();
    programs.sort_by_key(|(name, _)| *name);

    let mut lines = vec![
        "<details>".to_owned(),
        format!(
            "<summary>Programs you can add by name ({})</summary>",
            programs.len()
        ),
        String::new(),
        "| Program | Aliases | Description | Platforms |".to_owned(),
        "| --- | --- | --- | --- |".to_owned(),
    ];
    for (name, program) in programs {
        let aliases = program
            .aliases
            .iter()
            .map(|alias| format!("`{alias}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let description = program
            .description
            .as_deref()
            .unwrap_or_default()
            .replace('|', "\\|");
        lines.push(format!(
            "| [{name}]({}) | {aliases} | {description} | {} |",
            program.repository,
            platforms(program)
        ));
    }
    lines.push(String::new());
    lines.push("</details>".to_owned());
    lines
}

/// Where a program installs, as briefly as stays exact.
///
/// `All` for every OS and architecture, an OS alone when both of its architectures are covered,
/// and the architecture spelled out otherwise. A host outside the usual six is listed as written.
fn platforms(program: &Program) -> String {
    if program.path.is_some() {
        return "All".to_owned();
    }
    let hosts: Vec<&str> = program.targets.keys().map(String::as_str).collect();
    let covered = |os: &str, arch: &str| hosts.contains(&format!("{os}-{arch}").as_str());

    let usual = |host: &&str| {
        OSES.iter().any(|(os, _)| {
            ARCHES
                .iter()
                .any(|(arch, _)| **host == format!("{os}-{arch}"))
        })
    };
    let unusual: Vec<&str> = hosts.iter().copied().filter(|host| !usual(host)).collect();

    let mut parts: Vec<String> = Vec::new();
    for (os, os_label) in OSES {
        let arches: Vec<&str> = ARCHES
            .iter()
            .filter(|(arch, _)| covered(os, arch))
            .map(|(_, label)| *label)
            .collect();
        match arches.len() {
            0 => {}
            len if len == ARCHES.len() => parts.push(os_label.to_owned()),
            _ => parts.push(format!("{os_label} {}", arches.join("/"))),
        }
    }

    if unusual.is_empty() && parts == OSES.map(|(_, label)| label.to_owned()) {
        return "All".to_owned();
    }
    parts.extend(unusual.into_iter().map(str::to_owned));
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::regenerate;

    const REGISTRY: &str = r#"
version: 2
programs:
  zoxide:
    description: "Smarter cd"
    repository: https://github.com/ajeetdsouza/zoxide
    targets:
      windows-x86_64: { asset: "{release}/z.zip" }
      windows-aarch64: { asset: "{release}/z.zip" }
      macos-x86_64: { asset: "{release}/z.tar.gz" }
      macos-aarch64: { asset: "{release}/z.tar.gz" }
      linux-x86_64: { asset: "{release}/z.tar.gz" }
      linux-aarch64: { asset: "{release}/z.tar.gz" }
  delta:
    aliases: [git-delta]
    description: "Pager for git | diff output"
    repository: https://github.com/dandavison/delta
    targets:
      windows-x86_64: { asset: "{release}/d.zip" }
      macos-aarch64: { asset: "{release}/d.tar.gz" }
      linux-x86_64: { asset: "{release}/d.tar.gz" }
      linux-aarch64: { asset: "{release}/d.tar.gz" }
      linux-riscv64: { asset: "{release}/d.tar.gz" }
"#;

    const README: &str = "Intro.\n\n<!-- registry -->\nstale\n<!-- /registry -->\n\nOutro.\n";

    #[test]
    fn fills_the_region_by_name_and_leaves_the_rest() {
        let out = regenerate(README, REGISTRY).unwrap();
        assert!(
            out.starts_with("Intro.\n\n<!-- registry -->\n<details>\n"),
            "{out}"
        );
        assert!(
            out.ends_with("</details>\n<!-- /registry -->\n\nOutro.\n"),
            "{out}"
        );
        assert!(!out.contains("stale"), "{out}");
        assert!(out.contains("(2)</summary>"), "{out}");
        assert!(out.find("[delta]") < out.find("[zoxide]"), "{out}");
    }

    #[test]
    fn rows_carry_aliases_escaped_descriptions_and_platforms() {
        let out = regenerate(README, REGISTRY).unwrap();
        assert!(
            out.contains(
                "| [delta](https://github.com/dandavison/delta) | `git-delta` | \
                 Pager for git \\| diff output | Windows x64, macOS arm64, Linux, linux-riscv64 |"
            ),
            "{out}"
        );
        assert!(out.contains("| Smarter cd | All |"), "{out}");
    }

    #[test]
    fn is_idempotent() {
        let once = regenerate(README, REGISTRY).unwrap();
        assert_eq!(regenerate(&once, REGISTRY).unwrap(), once);
    }

    #[test]
    fn a_readme_without_the_region_is_an_error() {
        let error = format!("{:#}", regenerate("Nothing here.\n", REGISTRY).unwrap_err());
        assert!(error.contains("no `<!-- registry -->` region"), "{error}");
    }

    #[test]
    fn an_unclosed_region_is_an_error() {
        let error = format!(
            "{:#}",
            regenerate("<!-- registry -->\n", REGISTRY).unwrap_err()
        );
        assert!(error.contains("never closed"), "{error}");
    }
}
