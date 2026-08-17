//! `cargo xtask release` — the Rust counterpart of the reference project's `scripts/release.ts`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use dialoguer::{theme::ColorfulTheme, Select};
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
    run_command(&root, "cargo", &["check", "--workspace", "--quiet"])?;
    run_command(&root, "cargo", &["fmt", "--all"])?;

    run_command(&root, "git", &["add", "."])?;
    run_command(&root, "git", &["commit", "-m", &format!("v{new_version}")])?;
    run_command(&root, "git", &["tag", &format!("v{new_version}")])?;

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
fn set_version(document: &mut toml_edit::DocumentMut, version: &Version) -> Result<()> {
    document["workspace"]["package"]["version"] = toml_edit::value(version.to_string());

    let pin = document
        .get_mut("workspace")
        .and_then(|w| w.get_mut("dependencies"))
        .and_then(|d| d.get_mut("vendorfiles"))
        .and_then(|v| v.as_table_like_mut())
        .context("workspace.dependencies.vendorfiles is missing from Cargo.toml")?;
    pin.insert("version", toml_edit::value(version.to_string()));
    Ok(())
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

fn run_command(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} failed", args.join(" "));
    }
    Ok(())
}
