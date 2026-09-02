# vendorfiles-rs - Design

A Rust rewrite of [vendorfiles](https://github.com/Araxeus/vendorfiles) (TypeScript/Bun) with
byte-level parity on CLI surface, config files and lockfiles.

## 1. Source architecture (TypeScript)

| File | Responsibility |
| --- | --- |
| `cli.ts` | Commander command tree, argument massaging, dispatch |
| `lib/config.ts` | Config discovery/parse (toml/yml/json/package.json), default merging, write-back |
| `lib/commands.ts` | `sync`, `install`, `uninstall` orchestration |
| `lib/github.ts` | Octokit REST + streaming downloads, in-process release cache |
| `lib/auth.ts` | Token resolution (env → keyring), device flow login |
| `lib/utils.ts` | Colors/logging, lockfile IO, path helpers, `{version}` templating |

Everything shares a process-global mutable `runOptions` and a memoised `getConfig()`.

## 2. Crate layout

```
vendorfiles-rs/
├── crates/
│   ├── vendorfiles_core/     # library: all behaviour, `thiserror` errors, no process exits
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs      # VendorError - Display == user-facing message
│   │       ├── ui.rs         # ANSI colors, INFO/SUCCESS/WARNING/ERROR routing, RunOptions
│   │       ├── progress/     # the live display, and the plain-line fallback
│   │       ├── model.rs      # FileEntry / FileTarget / VendorDependency / VendorConfig
│   │       ├── template.rs   # {version} / {release} / {vendorFolder} substitution, trims
│   │       ├── config/
│   │       │   ├── mod.rs      # Workspace: discovery, defaults merge, mutation, write-back
│   │       │   ├── format.rs   # ConfigFormat, detect-indent, per-format (de)serialisation
│   │       │   ├── document.rs # ConfigDocument: format-preserving editable document
│   │       │   └── yaml_emit.rs # block-style YAML matching the `yaml` npm package
│   │       ├── lockfile.rs   # vendor-lock.json read/write, config→lock file mapping
│   │       ├── fsx.rs        # delete-file-and-empty-parents, stream-to-file
│   │       ├── archive.rs    # magic-byte sniffing + zip/tar/tar.gz/gz/crx extraction,
│   │       │                 # selective by member with a full-extraction fallback
│   │       ├── remote_zip.rs # HTTP-range reader: named members out of a remote ZIP
│   │       ├── github/
│   │       │   ├── mod.rs    # GitHubClient: releases (cached), commits, search, downloads
│   │       │   ├── auth.rs   # token resolution, OAuth device flow
│   │       │   ├── credentials.rs # the platform's native credential store
│   │       │   └── http.rs   # the reqwest client + its rustls provider
│   │       └── ops/
│   │           ├── mod.rs        # Session
│   │           ├── install.rs    # prepare / download / commit
│   │           ├── sync.rs       # the three-pass traversal
│   │           ├── uninstall.rs
│   │           └── version.rs    # version resolution, staleness check
│   └── vendorfiles/          # binary `vendor`: clap derive, commander-compatible help/errors
│       └── src/
│           ├── main.rs       # exit codes, ERROR: prefix
│           ├── cli.rs        # clap derive + Commander error wording
│           ├── run.rs        # command dispatch
│           ├── source.rs     # splitting an `install` operand into source and version
│           ├── spec.rs       # the command grammar, for help routing and operand counting
│           ├── help.rs       # help/version interception
│           └── help/*.txt    # help text captured from vendorfiles@1.4.2
└── xtask/                    # `cargo xtask release`
```

Rationale: the library never calls `process::exit` and never prints usage - it returns
`VendorError`. The binary owns the terminal contract (exit codes, help text, `ERROR:` prefix).
This is the only split that lets integration tests assert on behaviour without spawning
processes, while keeping exit-code parity in one small place.

## 3. Data & ownership model

### 3.1 Config file `files` array

The TS type is
`(string | { [input: string]: string | string[] | { [in: string]: string } })[]`.

```rust
pub enum FileEntry {
    Simple(String),                        // "dist/coloris.min.js"
    Mapped(IndexMap<String, FileTarget>),  // { "LICENSE": "COPYING", ... }
}

pub enum FileTarget {
    Rename(String),                        // "COPYING"
    ExtractList(Vec<String>),              // ["fzf"]                (archive, keep names)
    ExtractMap(IndexMap<String, String>),  // { "fzf.exe": "f.exe" } (archive, rename)
}
```

`IndexMap` everywhere: lockfile key order is observable output, and the TS `Object.assign`
merge order must be reproduced exactly (first insertion wins position, later value wins).

### 3.2 Workspace - the single owner

```rust
pub struct Workspace {
    pub config: VendorConfig,                          // { vendorFolder }
    pub dependencies: IndexMap<String, VendorDependency>, // defaults already merged
    pub file: ConfigFile,                              // path, format, indent, newline, document
}
```

`Workspace` owns all config data for the process lifetime, inside a `Session` that pairs it
with the client:

```rust
pub struct Session {
    pub github: Arc<GitHubClient>,   // shared so download tasks can hold it
    pub workspace: Workspace,        // owned; only ever reached through &mut self
}
```

Operations *clone the single `RawDependency`* they act on (a handful of small `String`s)
rather than juggling a split borrow of `dependencies` against `file`. This keeps every
signature lifetime-free at a cost that is invisible next to a network round-trip, and turns
`&mut self` into the mechanism that serialises config writes (see §4) - no lock required.

Borrowed data is used where it is free and unambiguous: `&Dependency`, `&VendorConfig` and
`&Path` flow down into pure helpers (`dependency_folder`, `config_files_to_lock_files`). The
only process-global state is `--pr` mode, an `AtomicBool` set once from `main`, mirroring the
TS module-level singleton.

### 3.3 Config document (write-back)

Write-back must preserve keys the tool does not model (`package.json` has dozens).
`ConfigDocument` therefore keeps the *whole* parsed document, not a projection:

```rust
pub enum ConfigDocument {
    /// JSON and YAML: structural round-trip through an order-preserving JSON value.
    Structural(serde_json::Value),
    /// TOML: `toml_edit` document - preserves comments and layout.
    Toml(Box<toml_edit::DocumentMut>),
}
```

Mutation surface is deliberately tiny (three operations), which is what makes keeping the
two representations honest tractable:

* `set_dependency_version(name, version)`
* `upsert_dependency(name, &VendorDependency)`
* `remove_dependency(name)`

Typed reads go through a canonical `serde_json::Value` produced at load time, so all three
formats share one validation path and one set of error messages.

### 3.4 GitHub client

```rust
pub struct GitHubClient {
    api: octocrab::Octocrab,   // REST: releases, commits, search
    http: reqwest::Client,     // streaming: raw contents + release assets
    token: Option<Token>,      // redacted in Debug
    releases: Mutex<IndexMap<ReleaseKey, Arc<OnceCell<Arc<Release>>>>>,
    warned: Once,              // the rate-limit warning fires at most once
}
```

`reqwest` is built with `rustls-no-provider` and `github::http` installs `ring` as the process
provider. Its `rustls` feature would hard-wire aws-lc-rs, compiling a second crypto backend
next to the `ring` one octocrab already uses; `cargo tree` shows neither aws-lc, OpenSSL, nor
native-tls in the graph.

The cache stores a `OnceCell` *per key* rather than a value, so the concurrent version-resolution
pass collapses duplicate lookups into one request instead of racing - the anonymous rate limit
is 60 requests an hour, so a duplicate is not free. `Arc<Release>` means cache hits do not clone
asset lists. The lookup also reproduces the TS quirk: a request for a tag is satisfied by any
already-resolved release of the same repository whose `tag_name` matches.

### 3.5 Credential storage

There is no cross-platform keyring facade in the dependency graph. `keyring-core` supplies the
`Entry` API and exactly one store crate is compiled in per target, through
`[target.'cfg(target_os = "…")'.dependencies]`:

| Target | Crate | `persistence()` |
| --- | --- | --- |
| Windows | `windows-native-keyring-store` | `UntilDelete` |
| macOS | `apple-native-keyring-store` (`keychain`) | `UntilDelete` |
| Linux, first choice | `zbus-secret-service-keyring-store` | `UntilDelete` |
| Linux, fallback | `linux-keyutils-keyring-store` | `UntilReboot` |
| other | - | store unavailable |

Linux is the only target with a choice to make, and it is made at runtime: the Secret Service
persists to disk but needs a daemon that headless boxes, minimal containers and WSL usually
lack, so `native()` tries it first and falls back to keyutils. The `zbus` implementation is
used rather than the `dbus` one because it is pure Rust - `cargo tree` shows no libdbus - and
its `rt-async-io-crypto-rust` feature drives zbus's blocking API on its own async-io executor,
so it never interacts with the tool's tokio runtime.

Two consequences worth being deliberate about:

* The `keyring` facade would compile every backend's glue on every target and, on Linux, link
  libdbus for a Secret Service backend this tool does not use. Nothing here needs a system
  library on any platform now.
* `github::credentials::transience_warning` reads the chosen store's own `persistence()`, so
  landing on the keyutils fallback tells the user their token will not outlive a reboot at the
  moment it is stored, rather than by looking anonymous later. Nothing hard-codes which store
  that is: the warning follows whatever `native()` actually opened.

`credentials` builds entries from a store handle it owns (`CredentialStoreApi::build`) rather
than registering one with `keyring_core::set_default_store`. That keeps a process-global - and
its initialise-before-first-use ordering hazard - out of the design; the store opens lazily,
once, behind a `OnceLock`.

Keyring access is blocking IPC (on Linux it can prompt to unlock), so the CLI reaches it
through `auth::resolve_token_async`, which hops to the blocking pool.

### 3.6 Errors

`VendorError` (thiserror) in the library; every variant's `Display` is the exact string the
TS tool prints after the `ERROR: ` prefix. `vendorfiles` uses `anyhow` at the boundary and
renders `\x1b[31mERROR: {e}\x1b[0m` to stderr, exit 1.

## 4. Concurrency

The TS tool is sequential across dependencies and concurrent within one. Going wider without
changing what the user sees required splitting an install into three stages, in `ops::install`:

| Stage | Borrow | Concurrency |
| --- | --- | --- |
| `Session::prepare` | `&self` | resolve version + staleness; read-only |
| `install::download` | none (`Arc<GitHubClient>` + owned `Prepared`) | one task per dependency, ≤8 at a time |
| `Session::commit` | `&mut self` | strictly ordered: write lockfile, write config, settle the line |

`sync` then:

1. resolves every version with one `join_all` - single-flight per release key, so two
   dependencies on the same repo still cost one request;
2. `tokio::spawn`s a download task per dependency (a semaphore caps in-flight work);
3. awaits those handles **in order**, committing each as it arrives.

Because step 3 awaits in order, output still streams out dependency by dependency in exactly
the TS tool's sequence while later dependencies are still downloading. The `&mut self` on
`commit` is what serialises config writes - no lock needed, and the borrow checker enforces it.

`Arc<GitHubClient>` is the only shared ownership in the design; everything else is a borrow
from `Session`. The log lines a download would have printed are returned from the stage rather
than printed inside it, which is what makes the ordering property structural rather than
incidental.

Within a dependency, plain files download concurrently and then release assets do - the two
`Promise.all` batches in `commands.ts`. Their log lines come back in declaration order instead
of completion order, a strict narrowing of what the TS tool could emit.

On the first error, remaining download tasks are aborted, so nothing downloads that will never
be committed.

Downloads stream to disk (`reqwest` byte stream → `tokio::fs::File`); nothing is buffered whole
in memory except archives that must be sniffed and extracted.

Measured against `vendorfiles@1.4.2` on a config with 8 dependencies:

| | TypeScript | Rust |
| --- | --- | --- |
| `sync` (nothing downloaded yet) | 3127 ms | 893 ms |
| `sync` (everything up to date) | 721 ms | 66 ms |
| `outdated` | 3426 ms | 908 ms |
| `--version` | 706 ms | 54 ms |

### 4.1 Reporting

-



`progress` owns the display. Each dependency holds an `Arc<progress::Dependency>` - shared


because the download task and the ordered `commit` describe the same line. `fsx::stream_to_file`


advances that line per chunk, so it tracks bytes actually written.
-




The display is a fixed region at the bottom of the terminal, drawn with ratatui's inline viewport
-
-
and redrawn from a snapshot of `RunState` on an 80 ms tick. Four modules, one of which touches a


terminal:-
-




| Module | Responsibility |


| --- | --- |


| `state` | `RunState`, `Stage`, row assignment, byte accounting. Plain data. |


| `view` | `view(&RunState, tick, area, buf)` - pure, so frames are asserted cell by cell in tests. |


| `driver` | The render thread: owns the `Terminal`, ticks, inserts lines above, tears down. |
-

| `ansi` | Parses our own SGR strings back into ratatui text. |





The region is `5 + rows` lines and at most `REGION_WIDTH` columns: frame, summary bar, worker rows,


rule, footer. `rows` is fixed once, after staleness is known, at


`min(MAX_CONCURRENT_DOWNLOADS, stale, rows_that_fit(terminal))`; at zero - `outdated`, or a project


already up to date - the rule and footer go and the box is three lines.-





Rows are places, not a list. `RunState::assign` gives a dependency a row and it keeps that row


until it has nothing left to show; a freed row is refilled in place and an empty row stays empty,


so no row moves for an event that concerned another. Empty rows are filled by `Stage::priority` -


committing, then active, then waiting, then settled - and then by config order. Anything in flight


without a row is counted in the footer.





A settled dependency reports on its own row; its outcome line is held until `Reporter::end` prints


them all as the region comes down. Emitting them as they happen pushes the region a row down the


screen each time, since `insert_before` only scrolls above the region when the region already sits


on the last row. Warnings and errors still go up immediately.
-




Every terminal write goes through `print_out` or `print_err`, which hand the line to the render


thread; a raw `println!` lands wherever the cursor happens to be. `driver::wipe` returns the cursor


to the region's first row after clearing, since `Terminal::clear` leaves it at the bottom.
--




Animation requires **stdout** to be a terminal and `--pr` to be off. Stdout rather than stderr is


forced: anchoring an inline viewport asks for the cursor position, and crossterm sends that query to


stdout w-atever the backend holds. When it does not animate, a dependency buffers its `INFO:` lines


and flushes them as it settles, so piped output keeps the bytes and ordering it had before the


display existed.-




-
The region is wiped and the cu-sor restored on every exit - `end()` on each of `sync`'s error paths,


a `Drop` on the driver, and a panic hook, since `draw` hides the cursor.





Within a dependency, `record` notes destinations after each batch of transfers joins rather than


as each one lands, so the piped record follows the `files` array even though the network decides


completion order. `ui` routes every line through the render thread, so a warning arriving

-
mid-run appears above the region instead of being overwritten.

## 5. Parity contract

Verified against the installed `vendor@1.4.2` binary in isolated fixtures:

* Exit codes: `0` success, `1` for *every* failure including argument-parse errors
  (clap's default `2` is overridden).
* Argument errors use commander's wording: `error: unknown command 'x'`,
  `error: unknown option '--x'`, `error: missing required argument 'url/name'`.
* Bare `vendor` prints the root help to **stderr** and exits 1.
* ANSI colors are emitted unconditionally (no tty/`NO_COLOR` detection) - the TS tool
  hard-codes the escapes.
* `INFO`/`SUCCESS` and the `outdated` listing go to stdout; `WARNING`/`ERROR` to stderr.
  `--pr` suppresses `INFO`/`SUCCESS`/`WARNING` only.
* Lockfile: 2-space JSON, key order `repository, version, files`, trailing `\n` on install,
  **no** trailing newline when rewritten by `uninstall`.
* Config write-back: original indentation and final newline preserved; JSON via
  `serde_json` with detected indent; YAML via block style; TOML via `toml_edit`.
* `{version}` substitution replaces only the **first** occurrence and uses the first
  `\d+\.\d+\.\d+` match in the tag, else the tag with leading `v`s stripped.

### 5.1 How it was verified

Every claim above was checked by running the installed `vendorfiles@1.4.2` binary and this
build over identical fixtures in isolated temp directories, then diffing stdout, stderr, exit
code and the complete resulting file tree (including binary payloads). Covered:

* all seven help screens, every alias, `help <topic>`, and `-v`
* every argument-error form Commander produces, and bare `vendor`
* `sync` / `sync -f` / `outdated` / `update` / `update <name>` / `update --pr` / `install`
  (URL, `owner/repo`, search) / `uninstall`, run repeatedly to catch idempotence drift
* JSON (2-space and tab), YAML, TOML and `package.json` configs, including write-back
* `default` / `defaultVendorOptions`, `hashVersionFile` (`true`, path, `false`), `releaseRegex`,
  `locked`, `vendorFolder` overrides, `{vendorFolder}`, and `../` outputs
* release assets, tar.gz and zip extraction, nested archive members with `{version}`, and the
  zip-based `.crx`/`.xpi` packages
* multi-dependency lockfiles, the trailing-newline asymmetry, and lockfile deletion
* failure paths: missing repository file, missing release asset, nonexistent repo, bad tag
* an unsorted three-dependency cold `sync` to prove log lines stay grouped per dependency and
  in config order under the concurrent pipeline

## 6. Deliberate deviations

1. **`install <new-dep>` works.** The TS tool throws `TypeError: Cannot read properties of
   undefined (reading 'match')` when installing a repo that is not already in the config -
   it discards the CLI-provided URL and files. The Rust port implements the documented
   behaviour: install the files and add the dependency to the config.
2. **Auth header.** The TS tool passes `Authorization` as an endpoint *parameter*, so it
   ends up as a query field and the download requests are effectively anonymous. Rust sends
   a real `Authorization: Bearer` header.
3. **Keyring storage is plaintext** in the OS credential store (Windows Credential Manager /
   macOS Keychain / Secret Service) instead of the TS tool's hostname-derived AES-CBC blob.
   Same service/user (`vendorfiles-cli` / `github_token`); a value that cannot be a GitHub
   token is treated as absent so a TS-era entry degrades to "not logged in" rather than 401.
4. **TOML comments survive** a version bump (`toml_edit`); the TS tool drops them. Removing a
   dependency collapses the trailing blank lines that would otherwise accumulate.
5. **Failures the TS tool never handled** - a nonexistent tag, for instance - print the message
   the source already had for them and exit 1, instead of dumping an unhandled Octokit
   rejection and exiting 127.
6. **`vendor login` needs no config file**, and **`vendor update <name>` honours the `default`
   block** (the TS tool read the un-merged config entry there and reported "No repository
   found").
7. **`vendor install owner/repo` keeps a configured dependency's own repository URL** rather
   than rewriting it to the `https://www.github.com/...` form the shorthand expands to.
8. **`releaseRegex` compiles with `fancy-regex`**, so JavaScript patterns using lookaround keep
   working.
9. **`-p`/`--plain`** turns the live display off, and is the reason `--pr` no longer has a short
   form - a global flag that means one thing everywhere is worth more than the letter the
   reference spent on `update`'s only option. The two help screens differ from the captured
   reference by exactly those two lines; `tests/fixtures/help` keeps the reference text and the
   test applies the delta, so both stay checkable.
10. **Root options are global**, so `-c`/`--config` and `-p`/`--plain` are accepted on either side
   of the subcommand; Commander only reads them before it. `CommandSpec::option_for` falls back to
   `ROOT_OPTIONS` for this reason - without it the operand scanner counts a root option's value as a
   positional and misreports the count in `too many arguments`. `-c` also **requires** its value,
   where the reference declares `[file/folder path]`: an optional value would let `-c` claim the
   next word after a command that takes names, and naming the option is only ever a request for a
   specific config. Help says `<file/folder path>` accordingly.
11. **`vendor` can vendor itself.** `install` resolves a few names without a search - `vendorfiles`,
   `vendorfiles-rs`, `vendor` - to an entry for this repository's release asset, rooted at the
   directory holding the running binary (`crates/vendorfiles/src/known.rs`). When a download's
   destination *is* the running executable, `fsx::replace_running_executable` stages it alongside
   and hands the swap to `self-replace`, because the image is locked on Windows and unsafe to
   overwrite anywhere; `remove_previously_installed` skips it for the same reason. The staged file
   is given the running binary's mode, since the crate does not document what it does with
   permissions.
12. **Programs installable by name.** `registry.yml` at the repository root maps a name or alias to
   a repository and a per-host release asset, fetched from `raw.githubusercontent.com` (no API rate
   limit, and an `ETag` for cheap revalidation) and cached for a day, so the usual `add` makes no
   request. Only `install` reads it; an unreachable registry warns and falls through to the search.
   `{target}`/`{ext}`/`{exe}` are expanded from the *host key being resolved* rather than from
   `cfg!`, which is what lets one machine validate every platform's entry in CI. An entry describes
   either a release asset (extracted, or taken as-is when it names no `member`) or a repository
   `path`; `releaseRegex` picks the tag train. Entries carry no `vendorFolder` and refuse unknown
   keys, so remote data cannot decide where files land. Two gates: every pull request checks the
   file parses and resolves, and the `registry` workflow checks the named assets exist and that one
   platform's `member` is really inside its archive - neither of which the offline half can know.
13. **Credential storage is a native store per platform** (§3.5). On Linux without a keyring
   daemon the token lands in keyutils rather than the Secret Service, where neither tool sees
   the other's token, and `login` warns that it will not survive a reboot.
14. **`install` takes any number of sources**, so `vendor add rg fd` adds both, and the version
   moved out of the reference's second operand into `source@version`. The `@` separates only
   when it falls after the last `/`, which leaves a userinfo URL intact;
   `crates/vendorfiles/src/source.rs` owns that splitting and nothing else. Sources are installed
   in turn and the first failure stops the run, like the `update <names>` and `uninstall <names>`
   loops - an install writes the config as it goes, so what did happen stays recorded. Two
   argument checks run before any of it, so a mistake never lands half a run: `-n`/`--name` and
   `-f`/`--files` each describe one entry, so passing either alongside more than one source is
   rejected rather than guessed at, and a version-shaped operand after the first is named as the
   reference's old syntax (`did you mean 'owner/repo@v1.0.0'?`) rather than searched for. A lone
   one is left to the search: there is nothing in front of it to suggest attaching it to.
   `install`'s two help screens therefore depart from the captured reference as well; the
   fixtures keep the reference text and the test applies the delta, exactly as for `--plain`
   in §9.
15. **`vendor config|cfg` and `vendor list|ls`**, which the reference has neither of. `config`
   prints the resolved config path alone - no `INFO:` prefix, so `$EDITOR "$(vendor config)"`
   composes - and `config edit [editor]` opens it. Both resolve the path through
   `Workspace::locate`, which runs the same search as `Workspace::load` but stops before parsing:
   a config that no longer loads is exactly when its path is worth asking for, and `config edit`
   is how it gets repaired. `list` (also spelled `config list`) prints the dependencies as a
   `name`/`version`/`repository` table in config order, reading the file and nothing else - no
   credential lookup, no display. These are the only commands with no fixture to check against, so
   `spec.rs` tests instead that every entry in `COMMANDS` appears in the root help in table order.
16. **Only `$EDITOR` falls through.** `config edit` has three candidates - the editor named on the
   command line, `$EDITOR`, and the operating system's own association - but an editor named on
   the command line is what the user asked for, so a failure there is reported rather than papered
   over by opening something else: being told `nano` is not installed beats having a different
   editor appear. `$EDITOR` describes the session rather than this command and can be stale in a
   way its owner would route around, so a value that will not *start* warns and hands on. An
   editor that starts and then exits non-zero never falls through either way - the file was
   opened, so there is nothing left to try - which is why `EditorError` separates "not started"
   from "exited".
17. **Editor settings are split like a shell would split them** (`split_editor_command`). Three
   shapes have to survive: `code --wait`, which is why the value is split at all; a bare path
   containing spaces, which splitting would turn into an attempt to run `C:\Program`; and the two
   together, `"C:\Program Files\...\code.exe" --wait`. Quotes settle the third, and the second
   cannot be told from the first by looking at it, so the filesystem breaks the tie - a value that
   names an existing file is the program and takes no arguments.
