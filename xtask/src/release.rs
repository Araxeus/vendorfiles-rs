//! `cargo xtask release` - the Rust counterpart of the reference project's `scripts/release.ts`.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use semver::Version;

use crate::sh;

/// Runs the release flow: clean check, version prompt, manifest and README update, format,
/// commit, tag.
pub fn run() -> Result<()> {
    let root = sh::workspace_root()?;

    let status = git(&root, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        bail!("Git working directory is not clean. Please commit changes before bumping version.");
    }

    let manifest_path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?;
    let mut document: toml_edit::DocumentMut = manifest.parse().context("parsing Cargo.toml")?;
    let current = current_version(&document)?;

    let candidates = [
        ("Patch", bump(&current, Level::Patch), "x.y.Z"),
        ("Minor", bump(&current, Level::Minor), "x.Y.z"),
        ("Major", bump(&current, Level::Major), "X.y.z"),
    ];
    let labels: Vec<String> = candidates
        .iter()
        .map(|(name, version, shape)| format!("{name} - ({version})  {shape}"))
        .collect();

    let choice = Select::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("current: {current} bump to:"))
        .items(&labels)
        .default(0)
        .interact_opt()
        .context("reading selection")?;
    let Some(choice) = choice else {
        bail!("Release cancelled.");
    };
    let new_version = candidates[choice].1.clone();

    set_version(&mut document, &new_version)?;
    std::fs::write(&manifest_path, document.to_string())
        .with_context(|| format!("writing {}", manifest_path.display()))?;
    println!("Updated Cargo.toml to version {new_version}");

    let readme_path = root.join("README.md");
    let readme = std::fs::read_to_string(&readme_path)
        .with_context(|| format!("reading {}", readme_path.display()))?;
    let rendered = crate::readme::regenerate(&set_readme_version(&readme, &new_version)?)
        .context("regenerating the README's derived format tabs")?;
    std::fs::write(&readme_path, rendered)
        .with_context(|| format!("writing {}", readme_path.display()))?;
    println!("Updated README.md to version v{new_version}");

    // Refresh Cargo.lock so the commit is self-consistent.
    sh::run(
        &root,
        "Refreshing Cargo.lock",
        "cargo",
        &["check", "--workspace", "--quiet"],
    )?;
    sh::run(&root, "Formatting sources", "cargo", &["fmt", "--all"])?;

    sh::run(&root, "Staging changes", "git", &["add", "."])?;
    sh::run(
        &root,
        &format!("Committing v{new_version}"),
        "git",
        &["commit", "-m", &format!("v{new_version}")],
    )?;
    sh::run(
        &root,
        &format!("Tagging v{new_version}"),
        "git",
        &["tag", &format!("v{new_version}")],
    )?;

    println!(
        "Committed and tagged version v{new_version} successfully.\n\
         run 'git push origin main && git push --tags origin main' to push changes.\n\
         run 'cargo publish --workspace' to publish crates.
         "
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum Level {
    Patch,
    Minor,
    Major,
}

const fn bump(version: &Version, level: Level) -> Version {
    match level {
        Level::Patch => Version::new(version.major, version.minor, version.patch + 1),
        Level::Minor => Version::new(version.major, version.minor + 1, 0),
        Level::Major => Version::new(version.major + 1, 0, 0),
    }
}

fn current_version(document: &toml_edit::DocumentMut) -> Result<Version> {
    let raw = document
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version"))
        .and_then(|v| v.as_str())
        .context("workspace.package.version is missing from Cargo.toml")?;
    Version::parse(raw).with_context(|| format!("parsing version {raw}"))
}

/// Updates `workspace.package.version` and the internal path-dependency pin that mirrors it.
///
/// Both edits keep the old value's surrounding whitespace and comments, so the diff is one
/// number per line and nothing else.
fn set_version(document: &mut toml_edit::DocumentMut, version: &Version) -> Result<()> {
    replace_keeping_decor(
        &mut document["workspace"]["package"]["version"],
        &version.to_string(),
    );

    let pin = document
        .get_mut("workspace")
        .and_then(|w| w.get_mut("dependencies"))
        .and_then(|d| d.get_mut("vendorfiles_core"))
        .and_then(|v| v.as_table_like_mut())
        .context("workspace.dependencies.vendorfiles_core is missing from Cargo.toml")?;
    match pin.get_mut("version") {
        Some(slot) => replace_keeping_decor(slot, &version.to_string()),
        None => {
            pin.insert("version", toml_edit::value(version.to_string()));
        }
    }
    Ok(())
}

/// Overwrites a TOML value in place, carrying its formatting across.
fn replace_keeping_decor(slot: &mut toml_edit::Item, value: &str) {
    match slot.as_value_mut() {
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = toml_edit::Value::from(value);
            *existing.decor_mut() = decor;
        }
        None => *slot = toml_edit::value(value),
    }
}

/// The `repository` of the self-vendoring example in the README's "Keeping vendor updated"
/// section, which is how that example is found.
const README_EXAMPLE: &str = "https://github.com/Araxeus/vendorfiles-rs";

/// Retags the self-vendoring example in the README to `v{version}`.
///
/// Only the JSON is touched. The YAML and TOML tabs beside it are derived from it, so
/// [`crate::readme::regenerate`] brings them along and there is one rendering rule rather than
/// three. The block is found by its `repository` rather than by the version being replaced, so a
/// README that has drifted is repaired instead of skipped.
fn set_readme_version(markdown: &str, version: &Version) -> Result<String> {
    let mut lines: Vec<String> = markdown.split('\n').map(str::to_owned).collect();
    let mut patched = 0_usize;

    let mut at = 0;
    while at < lines.len() {
        let Some(fence) = lines[at]
            .trim_start()
            .strip_prefix("```")
            .map(str::trim)
            .filter(|info| matches!(*info, "json" | "jsonc"))
        else {
            at += 1;
            continue;
        };
        let Some(close) = lines[at + 1..]
            .iter()
            .position(|line| line.trim() == "```")
            .map(|offset| at + 1 + offset)
        else {
            bail!(
                "the `{fence}` block at README.md line {} is never closed",
                at + 1
            );
        };

        let body = &lines[at + 1..close];
        let slot = body
            .iter()
            .any(|line| line.contains(README_EXAMPLE))
            .then(|| {
                body.iter()
                    .position(|line| line.trim_start().starts_with("\"version\":"))
            })
            .flatten();
        if let Some(offset) = slot {
            let line = &lines[at + 1 + offset];
            let indent = &line[..line.len() - line.trim_start().len()];
            let comma = if line.trim_end().ends_with(',') {
                ","
            } else {
                ""
            };
            lines[at + 1 + offset] = format!("{indent}\"version\": \"v{version}\"{comma}");
            patched += 1;
        }
        at = close + 1;
    }

    if patched == 0 {
        bail!("no JSON example in README.md declares `{README_EXAMPLE}` with a `version`");
    }
    Ok(lines.join("\n"))
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::{Level, bump, current_version, set_readme_version, set_version};
    use semver::Version;

    const MANIFEST: &str = "\
[workspace]
members = [\"a\"]

[workspace.package]
version = \"1.4.2\"      # bumped by xtask
license = \"MIT\"

[workspace.dependencies]
vendorfiles_core = { path = \"crates/vendorfiles_core\", version = \"1.4.2\" }
anyhow = \"1.0\"
";

    #[test]
    fn levels_bump_the_right_component() {
        let current = Version::parse("1.4.2").unwrap();
        assert_eq!(bump(&current, Level::Patch).to_string(), "1.4.3");
        assert_eq!(bump(&current, Level::Minor).to_string(), "1.5.0");
        assert_eq!(bump(&current, Level::Major).to_string(), "2.0.0");
    }

    #[test]
    fn the_current_version_comes_from_the_workspace_package() {
        let document = MANIFEST.parse().unwrap();
        assert_eq!(current_version(&document).unwrap().to_string(), "1.4.2");
    }

    #[test]
    fn setting_the_version_updates_the_internal_pin_and_keeps_the_layout() {
        let mut document: toml_edit::DocumentMut = MANIFEST.parse().unwrap();
        set_version(&mut document, &Version::parse("1.5.0").unwrap()).unwrap();
        let rendered = document.to_string();
        assert!(
            rendered.contains("version = \"1.5.0\"      # bumped by xtask"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "vendorfiles_core = { path = \"crates/vendorfiles_core\", version = \"1.5.0\" }"
            ),
            "{rendered}"
        );
        assert!(rendered.contains("anyhow = \"1.0\""), "{rendered}");
    }

    #[test]
    fn a_manifest_without_the_pin_is_an_error() {
        let mut document: toml_edit::DocumentMut = "[workspace.package]\nversion = \"1.0.0\"\n"
            .parse()
            .unwrap();
        assert!(set_version(&mut document, &Version::parse("1.0.1").unwrap()).is_err());
    }

    const README: &str = "\
## Keeping vendor updated

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    \"vendorDependencies\": {
        \"vendorfiles-rs\": {
            \"version\": \"v1.4.2\",
            \"repository\": \"https://github.com/Araxeus/vendorfiles-rs\"
        }
    }
}
```

</details>
<!-- /formats -->
";

    #[test]
    fn the_readme_example_is_retagged_in_place() {
        let out = set_readme_version(README, &Version::parse("1.5.0").unwrap()).unwrap();
        assert!(
            out.contains("            \"version\": \"v1.5.0\","),
            "{out}"
        );
        assert!(out.contains("## Keeping vendor updated"), "{out}");
    }

    #[test]
    fn an_example_that_is_not_the_tool_is_left_alone() {
        let source = README.replace("Araxeus/vendorfiles-rs", "mdbassit/Coloris");
        let error = format!(
            "{:#}",
            set_readme_version(&source, &Version::parse("1.5.0").unwrap()).unwrap_err()
        );
        assert!(error.contains("no JSON example"), "{error}");
    }

    #[test]
    fn an_unclosed_block_points_at_its_opening_fence() {
        let source = README.replace("\n```\n\n</details>", "\n\n</details>");
        let error = format!(
            "{:#}",
            set_readme_version(&source, &Version::parse("1.5.0").unwrap()).unwrap_err()
        );
        assert!(
            error.contains("`json` block at README.md line 7"),
            "{error}"
        );
    }

    #[test]
    fn the_committed_readme_tracks_the_workspace_version() {
        let root = crate::sh::workspace_root().unwrap();
        let manifest: toml_edit::DocumentMut = std::fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let version = current_version(&manifest).unwrap();
        let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
        assert_eq!(
            set_readme_version(&readme, &version).unwrap(),
            readme,
            "the README's vendorfiles-rs example is not on v{version}; run `cargo xtask release`"
        );
    }
}
