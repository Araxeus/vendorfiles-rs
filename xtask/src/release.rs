//! `cargo xtask release` — the Rust counterpart of the reference project's `scripts/release.ts`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use dialoguer::{Select, theme::ColorfulTheme};
use indicatif::{ProgressBar, ProgressStyle};
use semver::Version;

/// Runs the release flow: clean check, version prompt, manifest update, format, commit, tag.
pub fn run() -> Result<()> {
    let root = workspace_root()?;

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

    // Refresh Cargo.lock so the commit is self-consistent.
    run_command(
        &root,
        "Refreshing Cargo.lock",
        "cargo",
        &["check", "--workspace", "--quiet"],
    )?;
    run_command(&root, "Formatting sources", "cargo", &["fmt", "--all"])?;

    run_command(&root, "Staging changes", "git", &["add", "."])?;
    run_command(
        &root,
        &format!("Committing v{new_version}"),
        "git",
        &["commit", "-m", &format!("v{new_version}")],
    )?;
    run_command(
        &root,
        &format!("Tagging v{new_version}"),
        "git",
        &["tag", &format!("v{new_version}")],
    )?;

    println!(
        "Committed and tagged version v{new_version} successfully.\n\
         run 'git push origin main && git push --tags origin main' to push changes."
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
        .and_then(|d| d.get_mut("vendorfiles"))
        .and_then(|v| v.as_table_like_mut())
        .context("workspace.dependencies.vendorfiles is missing from Cargo.toml")?;
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

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .context("locating the workspace root")
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

/// Runs a command under a spinner, so a slow step (`cargo check`) never looks like a hang.
///
/// The child's output is captured rather than inherited — otherwise it would fight the spinner
/// for the same lines — and is replayed only when the command fails.
fn run_command(root: &Path, label: &str, program: &str, args: &[&str]) -> Result<()> {
    let spinner = ProgressBar::new_spinner().with_message(label.to_owned());
    spinner.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg} {elapsed}")
            .expect("the spinner template is valid"),
    );
    spinner.enable_steady_tick(Duration::from_millis(100));

    let started = Instant::now();
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("running {program} {}", args.join(" ")));
    spinner.finish_and_clear();

    let output = output?;
    if !output.status.success() {
        bail!(
            "{program} {} failed:\n{}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("✔ {label} ({:.1?})", started.elapsed());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Level, bump, current_version, set_version};
    use semver::Version;

    const MANIFEST: &str = "\
[workspace]
members = [\"a\"]

[workspace.package]
version = \"1.4.2\"      # bumped by xtask
license = \"MIT\"

[workspace.dependencies]
vendorfiles = { path = \"crates/vendorfiles\", version = \"1.4.2\" }
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
            rendered
                .contains("vendorfiles = { path = \"crates/vendorfiles\", version = \"1.5.0\" }"),
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
}
