//! `vendor list` and `vendor config edit`.

use anyhow::{Result, anyhow, bail};
use vendorfiles_core::{Workspace, ui};

/// `vendor list` / `vendor config list` - the config's dependencies as a table.
///
/// Config order, not sorted: the file is the source of truth, and reading the two side by side
/// is the point. An unset column prints `-` rather than an empty cell, so a missing `repository`
/// - the thing that stops `update` working - is visible rather than blank.
pub(super) fn list_dependencies(workspace: &Workspace) {
    const HEADERS: [&str; 3] = ["NAME", "VERSION", "REPOSITORY"];

    if workspace.dependencies.is_empty() {
        ui::info(format!(
            "no dependencies in {}",
            workspace.file.display_path()
        ));
        return;
    }

    let rows: Vec<[&str; 3]> = workspace
        .dependencies
        .iter()
        .map(|(name, entry)| {
            [
                name.as_str(),
                field(entry.version.as_deref()),
                field(entry.repository.as_deref()),
            ]
        })
        .collect();

    // Character counts, not bytes: a non-ASCII name would otherwise pad short.
    let widths: [usize; 3] = std::array::from_fn(|column| {
        rows.iter()
            .map(|row| row[column].chars().count())
            .chain(std::iter::once(HEADERS[column].chars().count()))
            .max()
            .unwrap_or(0)
    });

    // The last column is never padded, so nothing trails a line.
    let line = |cells: [&str; 3]| {
        format!(
            "{:<name$}  {:<version$}  {}",
            cells[0],
            cells[1],
            cells[2],
            name = widths[0],
            version = widths[1]
        )
    };

    vendorfiles_core::progress::print_out(&ui::cyan(&line(HEADERS)));
    for row in rows {
        vendorfiles_core::progress::print_out(&line(row));
    }
}

/// A dependency field as the table shows it: the value, or `-` when it is unset or empty.
fn field(value: Option<&str>) -> &str {
    value.map_or("-", |text| if text.is_empty() { "-" } else { text })
}

/// `vendor config edit [editor]` - open the config file for editing.
///
/// Three candidates, tried in order: the editor named on the command line, `$EDITOR`, and
/// finally whatever the operating system associates with the file.
///
/// Only `$EDITOR` falls through. An editor named on the command line is what the user asked for,
/// so a failure there is reported rather than papered over by opening something else - being told
/// `nano` is not installed beats having a different editor appear. `$EDITOR` describes the
/// session rather than this command, and can easily be stale in a way its owner would rather
/// route around, so a value that will not start warns and hands on to the last candidate.
///
/// An editor that starts and then exits non-zero is never a fall-through: the file was opened, so
/// there is nothing left to try.
pub(super) fn edit_config(path: &std::path::Path, editor: Option<&str>) -> Result<()> {
    if let Some(command) = editor.map(str::trim).filter(|value| !value.is_empty()) {
        return spawn_editor(command, path)
            .map_err(|error| anyhow!("could not run the editor given ({command}): {error}"));
    }

    if let Some(command) = env_editor() {
        match spawn_editor(&command, path) {
            Ok(()) => return Ok(()),
            // Started and refused: the file was opened, so the chain stops here.
            Err(EditorError::Exited(message)) => bail!("$EDITOR ({command}): {message}"),
            Err(EditorError::NotStarted(source)) => {
                ui::warning(format!("could not run $EDITOR ({command}): {source}"));
            }
        }
    }

    open::that_detached(path)
        .map_err(|source| anyhow!("could not open {}: {source}", path.display()))
}

/// `$EDITOR`, ignoring an unset or empty value.
fn env_editor() -> Option<String> {
    std::env::var("EDITOR")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Why an editor did not edit anything - the two halves [`edit_config`] treats differently.
#[derive(Debug)]
enum EditorError {
    /// The program could not be started at all: not on `PATH`, not executable, no such file.
    NotStarted(std::io::Error),
    /// It ran and reported failure, so it had its chance at the file.
    Exited(String),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted(source) => write!(f, "{source}"),
            Self::Exited(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EditorError {}

/// Runs `command` against `path` and waits for it to finish.
///
/// Stdio is inherited, because a terminal editor needs the terminal it was started from.
fn spawn_editor(command: &str, path: &std::path::Path) -> std::result::Result<(), EditorError> {
    let (program, arguments) = split_editor_command(command);
    let status = std::process::Command::new(&program)
        .args(arguments)
        .arg(path)
        .status()
        .map_err(EditorError::NotStarted)?;
    if !status.success() {
        return Err(EditorError::Exited(format!("exited with {status}")));
    }
    Ok(())
}

/// Splits an editor setting into the program to run and the arguments to pass it.
///
/// Three shapes have to come out right, and whitespace alone cannot separate them:
///
/// * `code --wait` - a program and its flags, which is why the string is split at all.
/// * `C:\Program Files\Microsoft VS Code\code.exe` - one path that happens to contain spaces.
///   Splitting it would try to run `C:\Program`, so a value that names an existing file is taken
///   whole, before any splitting is considered.
/// * `"C:\Program Files\Microsoft VS Code\code.exe" --wait` - both at once. Quotes say where the
///   program ends, exactly as they would in the shell the value was probably copied from.
fn split_editor_command(command: &str) -> (String, Vec<String>) {
    // A bare path with spaces cannot be told from a program with arguments, so the filesystem
    // breaks the tie: if the whole value names something, it is the program and nothing else.
    if std::path::Path::new(command).is_file() {
        return (command.to_owned(), Vec::new());
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    for character in command.chars() {
        match quote {
            Some(open) if character == open => quote = None,
            Some(_) => current.push(character),
            None if matches!(character, '"' | '\'') => {
                quote = Some(character);
                // An empty pair of quotes is still a word - `"" --flag` names a program of "".
                started = true;
            }
            None if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                started = true;
                current.push(character);
            }
        }
    }
    if started {
        words.push(current);
    }

    let mut words = words.into_iter();
    // The caller only ever passes a non-empty, trimmed value, so there is always a first word.
    let program = words.next().unwrap_or_else(|| command.to_owned());
    (program, words.collect())
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_editor_setting_splits_into_a_program_and_its_flags() {
        let split = |command: &str| {
            let (program, arguments) = super::split_editor_command(command);
            (program, arguments.join("|"))
        };

        assert_eq!(split("vim"), ("vim".to_owned(), String::new()));
        assert_eq!(
            split("code --wait"),
            ("code".to_owned(), "--wait".to_owned())
        );
        assert_eq!(
            split("code --wait --new-window"),
            ("code".to_owned(), "--wait|--new-window".to_owned())
        );
        // Quotes say where the program ends, so a path with spaces keeps its flags.
        assert_eq!(
            split(r#""C:\Program Files\Microsoft VS Code\code.exe" --wait"#),
            (
                r"C:\Program Files\Microsoft VS Code\code.exe".to_owned(),
                "--wait".to_owned()
            )
        );
        assert_eq!(
            split("'/usr/local/my editors/vim' -n"),
            ("/usr/local/my editors/vim".to_owned(), "-n".to_owned())
        );
        // Runs of whitespace collapse rather than producing empty arguments.
        assert_eq!(
            split("code   --wait"),
            ("code".to_owned(), "--wait".to_owned())
        );
    }

    /// The unquoted path with spaces, which no amount of splitting can tell from a program and
    /// its arguments - so the filesystem decides.
    #[test]
    fn an_editor_setting_that_names_a_real_file_is_taken_whole() {
        let dir = tempfile::tempdir().expect("temp dir");
        let editor = dir.path().join("my editor.exe");
        std::fs::write(&editor, "").expect("writing the fake editor");

        let command = editor.to_string_lossy().into_owned();
        assert!(command.contains(' '), "the test needs a path with a space");
        assert_eq!(
            super::split_editor_command(&command),
            (command.clone(), Vec::new())
        );

        // A file that is not there is split as usual, so `code --wait` still works.
        let missing = dir
            .path()
            .join("nothing there")
            .to_string_lossy()
            .into_owned();
        assert_ne!(
            super::split_editor_command(&missing).1,
            [] as [std::string::String; 0]
        );
    }
}
