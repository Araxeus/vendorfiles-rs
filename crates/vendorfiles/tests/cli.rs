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
        .env_remove("DEFAULT_VENDOR_CONFIG")
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

/// The reference help, plus every place our help deliberately differs from it.
///
/// The fixtures stay exactly as captured from `vendorfiles@1.4.2`, so both facts stay checkable:
/// what the reference printed, and how we depart from it. Three decisions account for the deltas,
/// all recorded in `docs/DESIGN.md` §6 - `-p` means `--plain` everywhere, so `--pr` gave up its
/// short form; `install` takes any number of sources, with the version moved out of its second
/// operand into `source@version`; and `list` and `config` are commands the reference never had,
/// so the root help gains two lines with nothing to compare against.
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
                "  install|add [options] <url/name> [version]  Install a dependency",
                &format!(
                    "{:<46}{}",
                    "  install|add [options] <url/name...>", "Install dependencies"
                ),
            )
            .replace(
                "  help [command]                              display help for command",
                "  completions <shell>                         Print a shell completion script\n  help [command]                              display help for command",
            )
            // `list` and `config` have no counterpart in the reference at all.
            .replace(
                "  login|auth [token]",
                &format!(
                    "{}\n{}\n  login|auth [token]",
                    format_args!(
                        "{:<46}{}",
                        "  list|ls", "List dependencies in the config file"
                    ),
                    format_args!(
                        "{:<46}{}",
                        "  config|cfg [command]", "Show or edit the config file"
                    )
                ),
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
        // about. `install` also takes any number of sources now, each carrying its own version
        // as `source@version`, so the usage line, the summary, the argument list and one example
        // all depart from the reference.
        "install" => reference
            .replace(
                "Usage: vendor install|add [options] <url/name> [version]",
                "Usage: vendor install|add [options] <url/name...>",
            )
            .replace(
                "Install a dependency. origin can be a GitHub repo URL or owner/repo format or
name of repo to search for.",
                "Install dependencies. Each source can be a GitHub repo URL or owner/repo format
or name of repo to search for, and may pin a version as source@version.",
            )
            .replace(
                "  url/name                GitHub repo URL or owner/repo format or name of repo
                          to search for
  version                 Version to install",
                "  url/name                GitHub repo URL or owner/repo format or name of repo
                          to search for, optionally as source@version",
            )
            .replace(
                "  -h, --help              display help for command",
                &format!(
                    "{:<24}{}
{:<24}{}
  -h, --help              display help for command",
                    "  --refresh",
                    "Re-check the program registry",
                    "  --dry-run",
                    "Print the entry, change nothing"
                ),
            )
            .replace(
                "Files have to be provided with -f or --files <files...>",
                "A name in the program registry needs no files; otherwise provide them with -f or
--files <files...>",
            )
            .replace(
                "    vendor add Araxeus/vendorfiles v1.0.0 -f README.md LICENSE",
                "    vendor add Araxeus/vendorfiles@v1.0.0 -f README.md LICENSE
    vendor add rg fd",
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
// Shell completions
// ---------------------------------------------------------------------------------------

#[test]
fn every_supported_shell_gets_a_script() {
    // No config anywhere: a completion script has nothing to do with a project.
    let dir = tempfile::tempdir().expect("temp dir");
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        let out = vendor(dir.path(), &["completions", shell]);
        assert_eq!(code(&out), 0, "exit code for {shell}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "stderr for {shell}");
        let script = stdout(&out);
        assert!(script.len() > 200, "{shell} script looks empty: {script}");
        assert!(
            script.contains("vendor"),
            "{shell} script does not mention the binary"
        );
    }
}

#[test]
fn the_script_covers_the_flags_it_was_generated_from() {
    // Generated from the parser, so our own additions appear without being listed twice.
    let dir = tempfile::tempdir().expect("temp dir");
    let script = stdout(&vendor(dir.path(), &["completions", "bash"]));
    for expected in [
        "--plain",
        "--refresh",
        "--config",
        "uninstall",
        "completions",
    ] {
        assert!(
            script.contains(expected),
            "{expected} missing from the script"
        );
    }
}

#[test]
fn the_script_covers_the_help_and_version_surface() {
    // `Cli` disables clap's help and version handling because `help::intercept` answers them, so
    // a script generated from the parser alone would promise less than the binary accepts.
    let dir = tempfile::tempdir().expect("temp dir");
    let script = stdout(&vendor(dir.path(), &["completions", "bash"]));
    for expected in [
        // Root flags, offered before any command.
        r#"opts="-c -p -h -v --config --plain --help --version"#,
        // `help` is a command, and the arm that recognises it offers the topics.
        "vendor,help)",
        r#"opts="-c -p --config --plain sync update outdated install uninstall list config login completions"#,
        // Each real subcommand takes `-h` too; `help` itself must not, `vendor help -h` fails.
        r#"opts="-f -h -c -p --force --help --config --plain"#,
    ] {
        assert!(
            script.contains(expected),
            "missing from the script: {expected}\n{script}"
        );
    }
}

#[test]
fn an_unknown_shell_names_the_ones_that_work() {
    let dir = tempfile::tempdir().expect("temp dir");
    let out = vendor(dir.path(), &["completions", "tcsh"]);
    assert_eq!(code(&out), 1);
    let message = stderr(&out);
    assert!(message.contains("tcsh"), "{message}");
    for shell in ["bash", "elvish", "fish", "powershell", "zsh"] {
        assert!(message.contains(shell), "{shell} not offered: {message}");
    }
}

#[test]
fn the_shell_name_is_case_insensitive() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert_eq!(code(&vendor(dir.path(), &["completions", "Bash"])), 0);
    assert_eq!(code(&vendor(dir.path(), &["completions", "PowerShell"])), 0);
}

#[test]
fn completions_help_is_routed_like_every_other_command() {
    let dir = project("{}");
    let served = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/help/completions.txt"),
    )
    .expect("the served help text")
    .replace("\r\n", "\n");
    // No reference fixture to diff against - the reference has no such command - so this checks
    // the binary serves the text we ship, through both routes.
    for args in [vec!["completions", "--help"], vec!["help", "completions"]] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), served, "stdout for `vendor {args:?}`");
        assert_eq!(code(&out), 0);
    }
}

// ---------------------------------------------------------------------------------------
// Argument errors - Commander's wording, and always exit 1
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
        .env_remove("DEFAULT_VENDOR_CONFIG")
        .output()
        .expect("running the vendor binary");
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

/// Runs the binary in `dir` with `DEFAULT_VENDOR_CONFIG` pointing at `fallback`.
fn vendor_with_default_config(dir: &Path, fallback: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vendor"))
        .args(args)
        .current_dir(dir)
        .env_remove("VENDOR_CONFIG")
        .env_remove("INIT_CWD")
        .env_remove("PWD")
        .env("GITHUB_TOKEN", "")
        .env("DEFAULT_VENDOR_CONFIG", fallback)
        .output()
        .expect("running the vendor binary")
}

#[test]
fn the_default_vendor_config_env_var_is_used_when_no_config_is_found() {
    let fallback = project(r#"{"vendorDependencies":{}}"#);
    let elsewhere = tempfile::tempdir().unwrap();
    let out = vendor_with_default_config(elsewhere.path(), fallback.path(), &["sync"]);
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn the_default_vendor_config_env_var_accepts_a_config_file() {
    let fallback = project(r#"{"vendorDependencies":{}}"#);
    let elsewhere = tempfile::tempdir().unwrap();
    let file = fallback.path().join("vendor.json");
    let out = vendor_with_default_config(elsewhere.path(), &file, &["sync"]);
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn the_default_vendor_config_env_var_is_ignored_when_a_config_is_found() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    // Reaching for this one instead would fail loudly.
    let fallback = project(r#"{"vendorConfig": "nope"}"#);
    let out = vendor_with_default_config(dir.path(), fallback.path(), &["sync"]);
    assert_eq!(stderr(&out), "");
    assert_eq!(code(&out), 0);
}

#[test]
fn an_explicit_config_option_never_falls_back_to_the_default() {
    let fallback = project(r#"{"vendorDependencies":{}}"#);
    let empty = tempfile::tempdir().unwrap();
    let location = empty.path().display().to_string();
    let out = vendor_with_default_config(empty.path(), fallback.path(), &["sync", "-c", &location]);
    assert!(
        stderr(&out).contains("No configuration file found in the current directory."),
        "{}",
        stderr(&out)
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn a_default_vendor_config_pointing_nowhere_is_reported() {
    let elsewhere = tempfile::tempdir().unwrap();
    let missing = elsewhere.path().join("no-such-folder");
    let out = vendor_with_default_config(elsewhere.path(), &missing, &["sync"]);
    assert!(
        stderr(&out).contains(&format!("Could not read {}", missing.display())),
        "{}",
        stderr(&out)
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn a_default_vendor_config_folder_without_a_config_reports_the_usual_error() {
    let elsewhere = tempfile::tempdir().unwrap();
    let empty = tempfile::tempdir().unwrap();
    let out = vendor_with_default_config(elsewhere.path(), empty.path(), &["sync"]);
    assert!(
        stderr(&out).contains("No configuration file found in the current directory."),
        "{}",
        stderr(&out)
    );
    assert_eq!(code(&out), 1);
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
// `config` and `list`, which the reference has no counterpart for
// ---------------------------------------------------------------------------------------

/// A config with two fully described entries and one that has nothing but files - the shapes
/// the table has to render, including the columns that can be missing.
const LISTABLE: &str = r#"{
  "vendorDependencies": {
    "React": {
      "version": "v18.2.0",
      "repository": "https://github.com/facebook/react",
      "files": ["README.md"]
    },
    "youtube-music": {
      "version": "v3.3.1",
      "repository": "https://github.com/th-ch/youtube-music",
      "files": ["LICENSE"]
    },
    "bare": { "files": ["x"] }
  }
}"#;

#[test]
fn config_prints_the_resolved_path_and_nothing_else() {
    let dir = project(LISTABLE);
    let expected = format!("{}\n", dir.path().join("vendor.json").display());
    for args in [vec!["config"], vec!["cfg"]] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), expected, "stdout for `vendor {args:?}`");
        assert_eq!(stderr(&out), "", "stderr for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn config_names_the_file_the_config_option_points_at() {
    let dir = project(LISTABLE);
    let elsewhere = tempfile::tempdir().unwrap();
    let folder = dir.path().to_string_lossy().into_owned();
    let expected = format!("{}\n", dir.path().join("vendor.json").display());

    for args in [
        vec!["-c", &folder, "config"],
        vec!["config", "--config", &folder],
    ] {
        let out = vendor(elsewhere.path(), &args);
        assert_eq!(stdout(&out), expected, "stdout for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

/// The path is what you ask for when the file no longer loads, so it must not need parsing.
#[test]
fn config_answers_for_a_config_that_does_not_parse() {
    let dir = project("{ not json at all");

    let out = vendor(dir.path(), &["config"]);
    assert_eq!(
        stdout(&out),
        format!("{}\n", dir.path().join("vendor.json").display())
    );
    assert_eq!(code(&out), 0);

    // `list` reads the file, so the same project still reports the parse failure.
    let out = vendor(dir.path(), &["list"]);
    assert!(stderr(&out).contains("Failed to parse"), "{}", stderr(&out));
    assert_eq!(code(&out), 1);
}

#[test]
fn a_missing_config_is_reported_by_config_too() {
    let dir = tempfile::tempdir().unwrap();
    let out = vendor(dir.path(), &["config"]);
    assert_eq!(stdout(&out), "");
    assert!(stderr(&out).contains("No configuration file found"));
    assert_eq!(code(&out), 1);
}

#[test]
fn list_prints_a_column_per_field_in_config_order() {
    let dir = project(LISTABLE);
    let expected = "\u{1b}[36mNAME           VERSION  REPOSITORY\u{1b}[0m
React          v18.2.0  https://github.com/facebook/react
youtube-music  v3.3.1   https://github.com/th-ch/youtube-music
bare           -        -
";

    // Every spelling is the same command.
    for args in [
        vec!["list"],
        vec!["ls"],
        vec!["config", "list"],
        vec!["cfg", "ls"],
    ] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), expected, "stdout for `vendor {args:?}`");
        assert_eq!(stderr(&out), "", "stderr for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn listing_an_empty_config_says_so_rather_than_printing_a_bare_header() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor(dir.path(), &["list"]);
    assert!(
        stdout(&out).contains("no dependencies in"),
        "{}",
        stdout(&out)
    );
    assert!(!stdout(&out).contains("NAME"));
    assert_eq!(code(&out), 0);
}

/// `-h` anywhere under `config` reaches the one page that documents all three forms, and so does
/// `vendor help config`.
#[test]
fn config_help_is_reachable_from_every_form() {
    let dir = project(LISTABLE);
    let expected = stdout(&vendor(dir.path(), &["config", "-h"]));

    assert!(
        expected.starts_with("Usage: vendor config|cfg [options] [command]"),
        "{expected}"
    );
    assert!(expected.contains("  edit [editor]  Open the config file in an editor"));
    assert!(expected.contains("  list|ls        List the dependencies in the config file"));

    for args in [
        vec!["config", "--help"],
        vec!["config", "edit", "-h"],
        vec!["config", "list", "--help"],
        vec!["cfg", "-h"],
        vec!["cfg", "ls", "--help"],
        vec!["help", "config"],
        vec!["help", "cfg"],
    ] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stdout(&out), expected, "stdout for `vendor {args:?}`");
        assert_eq!(code(&out), 0, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn list_help_is_reachable_from_every_form() {
    let dir = project(LISTABLE);
    let expected = stdout(&vendor(dir.path(), &["list", "-h"]));
    assert!(
        expected.starts_with("Usage: vendor list|ls [options]"),
        "{expected}"
    );
    for args in [vec!["ls", "-h"], vec!["help", "list"], vec!["help", "ls"]] {
        assert_eq!(
            stdout(&vendor(dir.path(), &args)),
            expected,
            "stdout for `vendor {args:?}`"
        );
    }
}

#[test]
fn a_config_subcommand_that_does_not_exist_is_worded_like_any_unknown_command() {
    let dir = project(LISTABLE);
    for (args, message) in [
        (
            vec!["config", "frobnicate"],
            "error: unknown command 'frobnicate'\n",
        ),
        (
            vec!["config", "edit", "one", "two"],
            "error: too many arguments for 'config'. Expected 2 arguments but got 3.\n",
        ),
        (
            vec!["list", "extra"],
            "error: too many arguments for 'list'. Expected 0 arguments but got 1.\n",
        ),
    ] {
        let out = vendor(dir.path(), &args);
        assert_eq!(stderr(&out), message, "stderr for `vendor {args:?}`");
        assert_eq!(code(&out), 1, "exit code for `vendor {args:?}`");
    }
}

/// An editor named on the command line is what was asked for, so it is run against the resolved
/// path and its failures are reported rather than routed around.
#[test]
fn config_edit_runs_the_editor_it_is_given() {
    let dir = project(LISTABLE);
    let stamp = dir.path().join("opened.txt");
    let editor = fake_editor(dir.path(), "editor", &stamp);

    let out = vendor(dir.path(), &["config", "edit", &editor]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        opened_path(&stamp).ends_with("vendor.json"),
        "editor was passed {:?}",
        opened_path(&stamp)
    );
}

#[test]
fn an_editor_that_was_asked_for_by_name_never_falls_through() {
    let dir = project(LISTABLE);
    let stamp = dir.path().join("opened.txt");
    let fallback = fake_editor(dir.path(), "fallback", &stamp);

    // Cannot be started at all.
    let out = vendor_with_editor(
        dir.path(),
        &fallback,
        &["config", "edit", "no-editor-a1b2c3"],
    );
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("could not run the editor given (no-editor-a1b2c3)"),
        "{}",
        stderr(&out)
    );
    assert!(!stamp.exists(), "$EDITOR ran anyway");

    // Started, and reported failure.
    let refuses = failing_editor(dir.path());
    let out = vendor_with_editor(dir.path(), &fallback, &["config", "edit", &refuses]);
    assert_eq!(code(&out), 1);
    assert!(
        stderr(&out).contains("could not run the editor given"),
        "{}",
        stderr(&out)
    );
    assert!(!stamp.exists(), "$EDITOR ran anyway");
}

/// `$EDITOR` describes the session rather than this command, so a value that will not start is a
/// warning on the way to the last candidate - but one that runs and refuses has had the file.
#[test]
fn edit_with_no_editor_named_uses_the_environment() {
    let dir = project(LISTABLE);
    let stamp = dir.path().join("opened.txt");
    let editor = fake_editor(dir.path(), "editor", &stamp);

    let out = vendor_with_editor(dir.path(), &editor, &["config", "edit"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(opened_path(&stamp).ends_with("vendor.json"));

    let refuses = failing_editor(dir.path());
    let out = vendor_with_editor(dir.path(), &refuses, &["config", "edit"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("$EDITOR"), "{}", stderr(&out));
}

/// Runs the binary with `EDITOR` set, which [`vendor`] deliberately does not do.
fn vendor_with_editor(dir: &Path, editor: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vendor"))
        .args(args)
        .current_dir(dir)
        .env_remove("VENDOR_CONFIG")
        .env_remove("DEFAULT_VENDOR_CONFIG")
        .env_remove("INIT_CWD")
        .env_remove("PWD")
        .env("GITHUB_TOKEN", "")
        .env("EDITOR", editor)
        .output()
        .expect("running the vendor binary")
}

/// The path a fake editor recorded, without the quoting `cmd` adds.
fn opened_path(stamp: &Path) -> String {
    std::fs::read_to_string(stamp)
        .expect("the editor recorded its argument")
        .trim()
        .trim_matches('"')
        .to_owned()
}

/// Writes a script that records the path it was handed, and returns the command that runs it.
///
/// A real editor would block on a terminal, so the test supplies one that does not: it is the
/// launch and the argument that are under test, not what an editor does with them.
fn fake_editor(dir: &Path, name: &str, stamp: &Path) -> String {
    script(
        dir,
        name,
        &if cfg!(windows) {
            format!("@echo %1> \"{}\"\r\n", stamp.display())
        } else {
            format!("#!/bin/sh\nprintf '%s' \"$1\" > \"{}\"\n", stamp.display())
        },
    )
}

/// An editor that starts and then reports failure - `vim` closed with `:cq`.
fn failing_editor(dir: &Path) -> String {
    script(
        dir,
        "refuses",
        if cfg!(windows) {
            "@exit /b 3\r\n"
        } else {
            "#!/bin/sh\nexit 3\n"
        },
    )
}

fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(format!(
        "{name}{}",
        if cfg!(windows) { ".bat" } else { ".sh" }
    ));
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path.to_string_lossy().into_owned()
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
    // The output is already plain here - a test harness has no terminal - so what this pins is
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

// ---------------------------------------------------------------------------------------
// Inheriting from a neighbouring entry
// ---------------------------------------------------------------------------------------

/// A config already vendoring `repository` under the name `first`.
fn neighbour_project(entry: &str) -> tempfile::TempDir {
    project(&format!(
        r#"{{"vendorDependencies":{{"first":{{{entry}}}}}}}"#
    ))
}

/// The neighbour the surprising case needs: a repository, a version and files.
fn described_neighbour(repository: &str) -> tempfile::TempDir {
    neighbour_project(&format!(
        r#""repository":"{repository}","version":"v1.0.0","files":["LICENSE"]"#
    ))
}

/// A repository nothing will be fetched from - every test here stops at `--dry-run`, which is
/// what keeps the warning decision checkable without a request.
const NEIGHBOURED: &str = "https://github.com/vendorfiles-rs-tests/not-a-real-repository";

#[test]
fn adding_a_second_name_for_one_repository_says_it_is_inheriting() {
    // The trap: without `--files`, the new entry silently takes the neighbour's files *and* its
    // version, and when that version already matches nothing is written at all.
    let dir = described_neighbour(NEIGHBOURED);

    let out = vendor(dir.path(), &["add", NEIGHBOURED, "--dry-run"]);
    let warning = stderr(&out);

    assert_eq!(code(&out), 0, "stderr: {warning}");
    assert!(warning.contains("'first' already vendors"), "{warning}");
    assert!(
        warning.contains("inherits its files and version"),
        "{warning}"
    );
    assert!(warning.contains("--files"), "{warning}");
}

#[test]
fn describing_it_with_files_still_borrows_the_version() {
    // `--files` describes the files, but the neighbour is still the base of the new entry, so its
    // version comes along - and that is the half that can skip the config write entirely.
    let dir = described_neighbour(NEIGHBOURED);

    let out = vendor(
        dir.path(),
        &["add", NEIGHBOURED, "--dry-run", "-f", "README.md"],
    );
    let warning = stderr(&out);

    assert_eq!(code(&out), 0, "stderr: {warning}");
    assert!(warning.contains("inherits its version."), "{warning}");
    assert!(
        !warning.contains("its files"),
        "the files were described, not borrowed: {warning}"
    );
    // And it really is borrowed: the entry a dry run prints says so.
    assert!(
        stdout(&out).contains(r#""version": "v1.0.0""#),
        "{}",
        stdout(&out)
    );
}

#[test]
fn re_adding_the_same_name_says_nothing() {
    // Updating an entry under its own name is ordinary, not a surprise: `first` is not a
    // neighbour of itself.
    let dir = described_neighbour(NEIGHBOURED);

    let out = vendor(
        dir.path(),
        &["add", NEIGHBOURED, "--dry-run", "-n", "first"],
    );
    assert!(
        !stderr(&out).contains("already vendors"),
        "its own entry is not a neighbour: {}",
        stderr(&out)
    );
}

#[test]
fn a_neighbour_with_nothing_to_give_is_not_mentioned_before_the_error() {
    // No files anywhere, so the command fails asking for them. Warning first about files it never
    // borrowed would be the worst of both.
    let dir = neighbour_project(&format!(r#""repository":"{NEIGHBOURED}""#));

    let out = vendor(dir.path(), &["add", NEIGHBOURED, "--dry-run"]);
    assert_ne!(code(&out), 0);
    assert!(
        !stderr(&out).contains("already vendors"),
        "nothing was inherited: {}",
        stderr(&out)
    );
}

/// A registry with one program, written next to the project.
fn registry(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("registry.yml");
    std::fs::write(
        &path,
        r#"
version: 1
programs:
  fd:
    aliases: [fdfind]
    repository: https://github.com/sharkdp/fd
    asset: "{release}/fd-v{version}-{target}{ext}"
    member: "fd-v{version}-{target}/fd{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      windows-aarch64: aarch64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
      linux-aarch64: aarch64-unknown-linux-gnu
  ripgrep:
    aliases: [rg]
    repository: https://github.com/BurntSushi/ripgrep
    asset: "{release}/ripgrep-{version}-{target}{ext}"
    member: "ripgrep-{version}-{target}/rg{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      windows-aarch64: aarch64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
      linux-aarch64: aarch64-unknown-linux-gnu
"#,
    )
    .expect("writing the registry");
    path
}

/// Runs the binary with a registry of our own and no network reachable through it.
fn vendor_with_registry(dir: &Path, args: &[&str]) -> Output {
    let registry = registry(dir);
    Command::new(env!("CARGO_BIN_EXE_vendor"))
        .args(args)
        .current_dir(dir)
        .env_remove("VENDOR_CONFIG")
        .env_remove("DEFAULT_VENDOR_CONFIG")
        .env_remove("INIT_CWD")
        .env_remove("PWD")
        .env("GITHUB_TOKEN", "")
        .env("VENDOR_REGISTRY", &registry)
        .output()
        .expect("running the vendor binary")
}

// ---------------------------------------------------------------------------------------
// `--dry-run`, which is also how the registry path is covered without a network
// ---------------------------------------------------------------------------------------

#[test]
fn a_dry_run_reports_the_registry_entry_and_writes_nothing() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let before = std::fs::read_to_string(dir.path().join("vendor.json")).unwrap();

    let out = vendor_with_registry(dir.path(), &["add", "fd", "--dry-run"]);
    let printed = stdout(&out);

    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    // The platform's own asset, with `{version}` left for the install to resolve.
    assert!(
        printed.contains("\"repository\": \"https://github.com/sharkdp/fd\""),
        "{printed}"
    );
    assert!(printed.contains("{release}/fd-v{version}-"), "{printed}");
    assert!(printed.contains("{version}"), "{printed}");
    assert!(printed.contains("would be added as"), "{printed}");
    assert!(
        printed.contains("nothing was downloaded or written"),
        "{printed}"
    );

    // Nothing touched: no config change, no vendor folder, no lockfile.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("vendor.json")).unwrap(),
        before
    );
    assert!(!dir.path().join("vendor").exists(), "a folder was created");
}

#[test]
fn a_dry_run_resolves_an_alias_to_its_canonical_name() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(dir.path(), &["add", "fdfind", "--dry-run"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    // Keyed `fd`, the canonical name, not the alias that was typed.
    assert!(stdout(&out).contains("\"fd\": {"), "{}", stdout(&out));
}

#[test]
fn a_dry_run_of_an_unknown_name_never_reaches_the_network() {
    // Not in the registry and not a URL, so the only way on is a GitHub search - which must not
    // be attempted before `--files` is even satisfied.
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(
        dir.path(),
        &["add", "definitely-not-a-program", "--dry-run"],
    );
    assert_ne!(code(&out), 0);
    assert!(!dir.path().join("vendor").exists());
}

// ---------------------------------------------------------------------------------------
// Several sources at once, and the version that moved into `source@version`
// ---------------------------------------------------------------------------------------

#[test]
fn a_dry_run_reports_every_source_it_was_given() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(dir.path(), &["add", "rg", "fd", "--dry-run"]);
    let printed = stdout(&out);

    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(printed.contains("\"ripgrep\": {"), "{printed}");
    assert!(printed.contains("\"fd\": {"), "{printed}");
    // Reported in the order they were named, one entry at a time.
    assert!(
        printed.find("ripgrep would be added as") < printed.find("fd would be added as"),
        "{printed}"
    );
}

#[test]
fn a_source_that_fails_stops_the_ones_after_it() {
    // `definitely-not-a-program` needs a search, which cannot happen offline, so the run must
    // fail there rather than carrying on to `fd`.
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(
        dir.path(),
        &["add", "definitely-not-a-program", "fd", "--dry-run"],
    );
    assert_ne!(code(&out), 0);
    assert!(
        !stdout(&out).contains("fd would be added as"),
        "the second source ran anyway: {}",
        stdout(&out)
    );
}

#[test]
fn a_version_after_an_at_sign_pins_the_entry() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(dir.path(), &["add", "fd@v10.0.0", "--dry-run"]);
    let printed = stdout(&out);

    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(printed.contains(r#""version": "v10.0.0""#), "{printed}");
    // The name comes from the source half, not from the whole operand.
    assert!(printed.contains("\"fd\": {"), "{printed}");
}

#[test]
fn options_that_describe_one_entry_are_rejected_for_several_sources() {
    let dir = project(r#"{"vendorDependencies":{}}"#);
    for (args, expected) in [
        (
            vec!["add", "rg", "fd", "-n", "Both"],
            "-n or --name describes one dependency, so it cannot be used with more than one source",
        ),
        (
            vec!["add", "rg", "fd", "--name", "Both"],
            "-n or --name describes one dependency, so it cannot be used with more than one source",
        ),
        (
            vec!["add", "rg", "fd", "-f", "LICENSE"],
            "-f or --files describes one dependency, so it cannot be used with more than one \
             source",
        ),
    ] {
        let out = vendor_with_registry(dir.path(), &args);
        assert_eq!(
            stderr(&out),
            format!("\u{1b}[31mERROR: {expected}\u{1b}[0m\n"),
            "stderr for `vendor {args:?}`"
        );
        assert_eq!(code(&out), 1, "exit code for `vendor {args:?}`");
    }
}

#[test]
fn one_source_still_takes_those_options() {
    // The rejection above is about ambiguity, not about the options themselves.
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(dir.path(), &["add", "fd", "-n", "Mine", "--dry-run"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("\"Mine\": {"), "{}", stdout(&out));
}

#[test]
fn the_reference_versions_second_operand_is_named_as_a_mistake() {
    // `vendor add owner/repo v1.0.0` used to pin a version. Every operand is a source now, so
    // without this it would quietly search GitHub for a repository called `v1.0.0`.
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(dir.path(), &["add", "Araxeus/vendorfiles", "v1.0.0"]);

    assert_eq!(
        stderr(&out),
        "\u{1b}[31mERROR: 'v1.0.0' looks like a version, not a source. Did you mean \
         'Araxeus/vendorfiles@v1.0.0'?\u{1b}[0m\n"
    );
    assert_eq!(code(&out), 1);
}

#[test]
fn a_lone_version_shaped_source_is_left_to_the_search() {
    // Nothing in front of it to attach it to, so there is no `source@version` to suggest - and
    // a repository really could be named this.
    let dir = project(r#"{"vendorDependencies":{}}"#);
    let out = vendor_with_registry(dir.path(), &["add", "v1.0.0", "--dry-run"]);
    assert_ne!(code(&out), 0);
    assert!(
        !stderr(&out).contains("looks like a version"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_missing_file_makes_sync_consider_the_dependency_stale() {
    let dir = up_to_date_project();
    std::fs::remove_file(dir.path().join("vendor/Coloris/COLORIS_LICENSE")).unwrap();
    let out = vendor(dir.path(), &["sync"]);
    // Stale means it tries to download, which fails offline or without credentials - either
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
