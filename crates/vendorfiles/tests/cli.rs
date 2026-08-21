//! End-to-end tests for the `vendor` binary.
//!
//! Everything here runs offline. The help fixtures were captured from the reference
//! `vendorfiles@1.4.2` CLI, so a diff against them is a real parity regression, not a
//! self-consistency check.

use std::path::Path;
use std::process::{Command, Output};

/// Runs the built binary in `dir` and returns its output.
fn vendor(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vendor"))
        .args(args)
        .current_dir(dir)
        // Keep the environment from redirecting config discovery or granting credentials.
        .env_remove("VENDOR_CONFIG")
        .env_remove("INIT_CWD")
        .env_remove("PWD")
        .env("GITHUB_TOKEN", "")
        .output()
        .expect("running the vendor binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n")
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

/// A temporary project directory containing `vendor.json`.
fn project(config: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("vendor.json"), config).expect("writing config");
    dir
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/help")
        .join(format!("{name}.txt"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
        .replace("\r\n", "\n")
}

/// The reference help, plus the two places our help deliberately differs from it.
///
/// The fixtures stay exactly as captured from `vendorfiles@1.4.2`, so both facts stay checkable:
/// what the reference printed, and how we depart from it. Both departures are the same decision —
/// `-p` means `--plain` everywhere, so `--pr` gave up its short form.
fn expected_help(name: &str) -> String {
    let reference = fixture(name);
    match name {
        // The config value is required here, so it is `<…>` rather than the reference's `[…]`.
        "root" => reference
            .replace(
                "  -c, --config [file/folder path]",
                "  -c, --config <file/folder path>",
            )
            .replace(
                "  -v, --version",
                &format!(
                    "{}\n  -v, --version",
                    format_args!(
                        "{:<46}{}",
                        "  -p, --plain", "Print plain lines instead of a live display"
                    )
                ),
            ),
        "update" => reference.replace("  -p|--pr     ", &format!("{:<14}", "  --pr")),
        // `--refresh` is ours, and the summary mentions the registry the reference has no idea
        // about.
        "install" => reference
            .replace(
                "  -h, --help              display help for command",
                &format!(
                    "{:<24}{}
  -h, --help              display help for command",
                    "  --refresh", "Re-check the program registry"
                ),
            )
            .replace(
                "Files have to be provided with -f or --files <files...>",
                "A name in the program registry needs no files; otherwise provide them with -f or
--files <files...>",
            ),
        _ => reference,
    }
}

// ---------------------------------------------------------------------------------------
// Help and version
// ---------------------------------------------------------------------------------------

#[test]
fn help_matches_the_reference_byte_for_byte() {
    let dir = project("{}");
    for (name, args) in [
        ("root", vec!["--help"]),
        ("root", vec!["-h"]),
        ("root", vec!["help"]),
        ("sync", vec!["sync", "--help"]),
        ("sync", vec!["help", "sync"]),
        ("sync", vec!["s", "-h"]),
        ("update", vec!["update", "--help"]),
        ("update", vec!["bump", "-h"]),
        ("outdated", vec!["outdated", "--help"]),
        ("install", vec!["install", "--help"]),
        ("install", vec!["i", "-h"]),
        ("uninstall", vec!["uninstall", "--help"]),
        ("uninstall", vec!["rm", "-h"]),
        ("login", vec!["login", "--help"]),
        ("login", vec!["auth", "-h"]),
    ] {
        let out = vendor(dir.path(), &args);
        assert_eq!(
            stdout(&out),
            expected_help(name),
            "stdout for `vendor {args:?}`"
        );
        assert_eq!(stderr(&out), "", "stderr for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn version_is_printed_alone() {
    let dir = project("{}");
    for args in [vec!["-v"], vec!["--version"]] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), format!("{}\n", env!("CARGO_PKG_VERSION")));
        assert_eq!(code(&out), 0);
    }
}

#[test]
fn help_is_answered_before_missing_arguments_are_reported() {
    let dir = project("{}");
    // `install` needs <url/name>, but `-h` still wins.
    let out = vendor(dir.path(), &["install", "-h"]);
    assert_eq!(stdout(&out), expected_help("install"));
    assert_eq!(code(&out), 0);
}

// ---------------------------------------------------------------------------------------
// Argument errors — Commander's wording, and always exit 1
// ---------------------------------------------------------------------------------------

#[test]
fn argument_errors_match_the_reference() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    for (args, expected) in [
        (vec!["frobnicate"], "error: unknown command 'frobnicate'\n"),
        (vec!["sync", "--nope"], "error: unknown option '--nope'\n"),
        (
            vec!["uninstall", "--bad"],
            "error: unknown option '--bad'\n",
        ),
        (
            vec!["install"],
            "error: missing required argument 'url/name'\n",
        ),
        (
            vec!["install", "-n"],
            "error: missing required argument 'url/name'\n",
        ),
        (
            vec!["install", "--files"],
            "error: option '-f, --files <files...>' argument missing\n",
        ),
        (
            vec!["sync", "-c"],
            "error: option '-c, --config <file/folder path>' argument missing\n",
        ),
        (
            vec!["uninstall", "-c"],
            "error: option '-c, --config <file/folder path>' argument missing\n",
        ),
        (
            vec!["sync", "extra"],
            "error: too many arguments for 'sync'. Expected 0 arguments but got 1.\n",
        ),
    ] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stderr(&out), expected, "stderr for `vendor {args:?}`");
        assert_eq!(stdout(&out), "", "stdout for `vendor {args:?}`");
        assert_eq!(code(&out), 1, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn no_command_prints_usage_to_stderr_and_fails() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    for args in [vec![], vec!["--config"], vec!["help", "nope"]] {
        let out = vendor(dir.path(), &args);
        assert_eq!(
            stderr(&out),
            expected_help("root"),
            "stderr for `vendor {args:?}`"
        );
        assert_eq!(stdout(&out), "", "stdout for `vendor {args:?}`");
        assert_eq!(code(&out), 1, "exit code for `vendor {args:?}`");
    }
}

// ---------------------------------------------------------------------------------------
// Config discovery and validation
// ---------------------------------------------------------------------------------------

#[test]
fn a_missing_config_is_reported_in_colour_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let out = vendor(dir.path(), &["sync"]);
    assert_eq!(
        stderr(&out),
        "\u{1b}[31mERROR: No configuration file found in the current directory.\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn login_does_not_need_a_config_file() {
    // A deliberate divergence: the reference loads the config for every command.
    let dir = tempfile::tempdir().unwrap();
    let out = vendor(dir.path(), &["login", "--help"]);
    assert_eq!(stdout(&out), fixture("login"));
    assert_eq!(code(&out), 0);
}

#[test]
fn the_config_option_accepts_a_folder_or_a_file() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let elsewhere = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let file = dir
        .path()
        .join("vendor.json")
        .to_string_lossy()
        .into_owned();

    for location in [folder, file] {
        // Global, so either side of the subcommand, long or short.
        for args in [
            vec!["-c", &location, "sync"],
            vec!["sync", "-c", &location],
            vec!["sync", "--config", &location],
            vec!["outdated", "-c", &location],
        ] {
            let out = vendor(elsewhere.path(), &args);
            assert_eq!(stderr(&out), "", "stderr for `vendor {args:?}`");
            assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
        }
    }
}

#[test]
fn the_vendor_config_env_var_is_honoured() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let elsewhere = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vendor"))
        .arg("sync")
        .current_dir(elsewhere.path())
        .env("VENDOR_CONFIG", dir.path())
        .output()
        .expect("running the vendor binary");
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn invalid_config_keys_are_reported_with_the_reference_wording() {
    let cases = [
        (
            r#"{"vendorDependencies": {"Foo": {"files": ["a"]}}}"#,
            "config key 'vendorDependencies.Foo.repository' is not a valid github url",
        ),
        (
            r#"{"vendorDependencies": {"Foo": {"repository": "https://github.com/a/b"}}}"#,
            "config key 'vendorDependencies.Foo.files' is not a valid array",
        ),
        (
            r#"{"vendorDependencies": {"Foo": {"repository": "https://github.com/a/b", "files": ["a"], "releaseRegex": "("}}}"#,
            "config key 'vendorDependencies.Foo.releaseRegex' must be a valid regex string",
        ),
        (r#"{"vendorConfig": "nope"}"#, "Invalid vendorConfig key in"),
        (
            r#"{"vendorDependencies": 7}"#,
            "Invalid vendorDependencies key in",
        ),
    ];

    for (config, expected) in cases {
        let dir = project(config);
        let out = vendor(dir.path(), &["sync"]);
        let message = stderr(&out);
        assert!(
            message.contains(expected),
            "expected {expected:?} in {message:?}"
        );
        assert_eq!(code(&out), 1);
    }
}

#[test]
fn an_empty_dependency_map_is_a_silent_success() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    for args in [vec!["sync"], vec!["update"], vec!["outdated"]] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), "", "stdout for `vendor {args:?}`");
        assert_eq!(stderr(&out), "", "stderr for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

// ---------------------------------------------------------------------------------------
// Offline behaviour driven by the lockfile
// ---------------------------------------------------------------------------------------

/// A project whose lockfile and files already agree with the config, so no request is made.
fn up_to_date_project() -> tempfile::TempDir {
    let dir = project(
        r#"{
  "vendorDependencies": {
    "Coloris": {
      "version": "v0.18.0",
      "repository": "https://github.com/mdbassit/Coloris",
      "files": ["dist/coloris.min.js", {"LICENSE": "COLORIS_LICENSE"}]
    }
  }
}
"#,
    );
    let folder = dir.path().join("vendor").join("Coloris");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("coloris.min.js"), "// js\n").unwrap();
    std::fs::write(folder.join("COLORIS_LICENSE"), "MIT\n").unwrap();
    std::fs::write(
        folder.join("vendor-lock.json"),
        r#"{
  "Coloris": {
    "repository": "https://github.com/mdbassit/Coloris",
    "version": "v0.18.0",
    "files": {
      "dist/coloris.min.js": "coloris.min.js",
      "LICENSE": "COLORIS_LICENSE"
    }
  }
}
"#,
    )
    .unwrap();
    dir
}

#[test]
fn sync_reports_up_to_date_without_touching_the_network() {
    let dir = up_to_date_project();
    let out = vendor(dir.path(), &["sync"]);
    assert_eq!(
        stdout(&out),
        "\u{1b}[36mINFO: Coloris is up to date\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 0);
}

#[test]
fn plain_is_accepted_on_either_side_of_the_subcommand() {
    // The output is already plain here — a test harness has no terminal — so what this pins is
    // that the flag parses in both positions and changes nothing else.
    let expected = "\u{1b}[36mINFO: Coloris is up to date\u{1b}[0m\n";
    for args in [
        vec!["sync", "--plain"],
        vec!["sync", "-p"],
        vec!["--plain", "sync"],
        vec!["-p", "sync"],
    ] {
        let dir = up_to_date_project();
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), expected, "stdout for `vendor {args:?}`");
        assert_eq!(stderr(&out), "", "stderr for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn a_missing_file_makes_sync_consider_the_dependency_stale() {
    let dir = up_to_date_project();
    std::fs::remove_file(dir.path().join("vendor/Coloris/COLORIS_LICENSE")).unwrap();
    let out = vendor(dir.path(), &["sync"]);
    // Stale means it tries to download, which fails offline or without credentials — either
    // way it must *not* claim to be up to date.
    assert!(
        !stdout(&out).contains("is up to date"),
        "unexpected stdout: {}",
        stdout(&out)
    );
}

#[test]
fn sync_reports_dependencies_in_config_order() {
    // The download stage overlaps across dependencies; the commit stage must not.
    let dir = tempfile::tempdir().expect("temp dir");
    let names = ["zeta", "alpha", "middle", "beta"];
    let entries: Vec<String> = names
        .iter()
        .map(|name| {
            format!(
                r#"    "{name}": {{
      "version": "v1",
      "repository": "https://github.com/example/{name}",
      "files": ["one"]
    }}"#
            )
        })
        .collect();
    std::fs::write(
        dir.path().join("vendor.json"),
        format!(
            "{{\n  \"vendorDependencies\": {{\n{}\n  }}\n}}\n",
            entries.join(",\n")
        ),
    )
    .expect("writing config");

    for name in names {
        let folder = dir.path().join("vendor").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("one"), name).unwrap();
        std::fs::write(
            folder.join("vendor-lock.json"),
            format!(
                "{{\n  \"{name}\": {{\n    \"repository\": \"https://github.com/example/{name}\",\n    \"version\": \"v1\",\n    \"files\": {{\n      \"one\": \"one\"\n    }}\n  }}\n}}\n"
            ),
        )
        .unwrap();
    }

    let out = vendor(dir.path(), &["sync"]);
    let expected = names.iter().fold(String::new(), |mut acc, name| {
        use std::fmt::Write as _;
        let _ = writeln!(acc, "\u{1b}[36mINFO: {name} is up to date\u{1b}[0m");
        acc
    });
    assert_eq!(stdout(&out), expected);
    assert_eq!(code(&out), 0);
}

#[test]
fn uninstall_removes_files_the_config_entry_and_the_lockfile() {
    let dir = up_to_date_project();
    let out = vendor(dir.path(), &["uninstall", "Coloris"]);
    assert_eq!(
        stdout(&out),
        "\u{1b}[32mSUCCESS: Uninstalled Coloris\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 0);
    assert!(!dir.path().join("vendor/Coloris").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("vendor.json")).unwrap(),
        "{\n  \"vendorDependencies\": {}\n}\n"
    );
}

#[test]
fn uninstall_rewrites_a_shared_lockfile_without_a_trailing_newline() {
    let dir = project(
        r#"{
  "vendorConfig": {"vendorFolder": "./vendor"},
  "vendorDependencies": {
    "A": {"version": "v1", "repository": "https://github.com/a/b", "files": ["one"], "vendorFolder": "{vendorFolder}"},
    "B": {"version": "v1", "repository": "https://github.com/c/d", "files": ["two"], "vendorFolder": "{vendorFolder}"}
  }
}
"#,
    );
    let folder = dir.path().join("vendor");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("one"), "1").unwrap();
    std::fs::write(folder.join("two"), "2").unwrap();
    std::fs::write(
        folder.join("vendor-lock.json"),
        "{\n  \"A\": {\n    \"repository\": \"https://github.com/a/b\",\n    \"version\": \"v1\",\n    \"files\": {\n      \"one\": \"one\"\n    }\n  },\n  \"B\": {\n    \"repository\": \"https://github.com/c/d\",\n    \"version\": \"v1\",\n    \"files\": {\n      \"two\": \"two\"\n    }\n  }\n}\n",
    )
    .unwrap();

    let out = vendor(dir.path(), &["uninstall", "A"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(!folder.join("one").exists());
    assert!(folder.join("two").exists());
    assert_eq!(
        std::fs::read_to_string(folder.join("vendor-lock.json")).unwrap(),
        "{\n  \"B\": {\n    \"repository\": \"https://github.com/c/d\",\n    \"version\": \"v1\",\n    \"files\": {\n      \"two\": \"two\"\n    }\n  }\n}"
    );
}

#[test]
fn uninstalling_an_unknown_dependency_names_the_config_file() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor(dir.path(), &["uninstall", "Nope"]);
    assert!(
        stderr(&out).contains("ERROR: Dependency Nope not found in"),
        "unexpected stderr: {}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("vendor.json"));
    assert_eq!(code(&out), 1);
}

#[test]
fn uninstall_with_no_names_is_rejected() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor(dir.path(), &["uninstall"]);
    assert_eq!(
        stderr(&out),
        "\u{1b}[31mERROR: No package names provided\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn update_of_an_unknown_or_locked_dependency_is_rejected() {
    let dir = project(
        r#"{
  "vendorDependencies": {
    "Pinned": {
      "version": "v1",
      "repository": "https://github.com/a/b",
      "files": ["one"],
      "locked": true
    }
  }
}
"#,
    );
    let out = vendor(dir.path(), &["update", "Missing"]);
    assert_eq!(
        stderr(&out),
        "\u{1b}[31mERROR: No dependency found with name Missing\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 1);

    let out = vendor(dir.path(), &["bump", "Pinned"]);
    assert_eq!(
        stderr(&out),
        "\u{1b}[31mERROR: Dependency Pinned is locked and cannot be upgraded\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn install_without_files_is_rejected_before_any_request() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor(dir.path(), &["install", "https://github.com/a/b"]);
    assert_eq!(
        stderr(&out),
        "\u{1b}[31mERROR: you must provide files to install with -f or --files <files...>\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn config_formats_are_all_discovered() {
    for (name, body) in [
        ("vendor.toml", "[vendorDependencies]\n"),
        ("vendor.yml", "vendorDependencies: {}\n"),
        ("vendor.yaml", "vendorDependencies: {}\n"),
        ("vendor.json", "{\"vendorDependencies\": {}}\n"),
        (
            "package.json",
            "{\"name\": \"x\", \"vendorDependencies\": {}}\n",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(name), body).unwrap();
        let out = vendor(dir.path(), &["sync"]);
        assert_eq!(stderr(&out), "", "stderr for {name}");
        assert_eq!(code(&out), 0, "exit code for {name}");
    }
}

#[test]
fn the_first_matching_config_name_wins() {
    let dir = tempfile::tempdir().unwrap();
    // vendor.toml outranks vendor.json; the invalid JSON must therefore never be parsed.
    std::fs::write(dir.path().join("vendor.toml"), "[vendorDependencies]\n").unwrap();
    std::fs::write(dir.path().join("vendor.json"), "not json at all").unwrap();
    let out = vendor(dir.path(), &["sync"]);
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}
