# vendorfiles-rs — Design

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
│   ├── vendorfiles/          # library: all behaviour, `thiserror` errors, no process exits
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs      # VendorError — Display == user-facing message
│   │       ├── ui.rs         # ANSI colors, INFO/SUCCESS/WARNING/ERROR routing, RunOptions
│   │       ├── model.rs      # FileEntry / FileTarget / VendorDependency / VendorConfig
│   │       ├── template.rs   # {version} / {release} / {vendorFolder} substitution, trims
│   │       ├── config/
│   │       │   ├── mod.rs      # Workspace: discovery, defaults merge, mutation, write-back
│   │       │   ├── format.rs   # ConfigFormat, detect-indent, per-format (de)serialisation
│   │       │   ├── document.rs # ConfigDocument: format-preserving editable document
│   │       │   └── yaml_emit.rs # block-style YAML matching the `yaml` npm package
│   │       ├── lockfile.rs   # vendor-lock.json read/write, config→lock file mapping
│   │       ├── fsx.rs        # delete-file-and-empty-parents, stream-to-file
│   │       ├── archive.rs    # magic-byte sniffing + zip/tar/tar.gz/gz/crx extraction
│   │       ├── github/
│   │       │   ├── mod.rs    # GitHubClient: releases (cached), commits, search, downloads
│   │       │   └── auth.rs   # token resolution, keyring, OAuth device flow
│   │       └── ops/
│   │           ├── mod.rs        # Session
│   │           ├── install.rs    # prepare / download / commit
│   │           ├── sync.rs       # the three-pass traversal
│   │           ├── uninstall.rs
│   │           └── version.rs    # version resolution, staleness check
│   └── vendor-cli/           # binary `vendor`: clap derive, commander-compatible help/errors
│       └── src/
│           ├── main.rs       # exit codes, ERROR: prefix
│           ├── cli.rs        # clap derive + Commander error wording
│           ├── run.rs        # command dispatch
│           ├── spec.rs       # the command grammar, for help routing and operand counting
│           ├── help.rs       # help/version interception
│           └── help/*.txt    # help text captured from vendorfiles@1.4.2
└── xtask/                    # `cargo xtask release`
```

Rationale: the library never calls `process::exit` and never prints usage — it returns
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

### 3.2 Workspace — the single owner

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
`&mut self` into the mechanism that serialises config writes (see §4) — no lock required.

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
    /// TOML: `toml_edit` document — preserves comments and layout.
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

The cache stores a `OnceCell` *per key* rather than a value, so the concurrent version-resolution
pass collapses duplicate lookups into one request instead of racing — the anonymous rate limit
is 60 requests an hour, so a duplicate is not free. `Arc<Release>` means cache hits do not clone
asset lists. The lookup also reproduces the TS quirk: a request for a tag is satisfied by any
already-resolved release of the same repository whose `tag_name` matches.

### 3.5 Errors

`VendorError` (thiserror) in the library; every variant's `Display` is the exact string the
TS tool prints after the `ERROR: ` prefix. `vendor-cli` uses `anyhow` at the boundary and
renders `\x1b[31mERROR: {e}\x1b[0m` to stderr, exit 1.

## 4. Concurrency

The TS tool is sequential across dependencies and concurrent within one. Going wider without
changing what the user sees required splitting an install into three stages, in `ops::install`:

| Stage | Borrow | Concurrency |
| --- | --- | --- |
| `Session::prepare` | `&self` | resolve version + staleness; read-only |
| `install::download` | none (`Arc<GitHubClient>` + owned `Prepared`) | one task per dependency, ≤8 at a time |
| `Session::commit` | `&mut self` | strictly ordered: print, write lockfile, write config |

`sync` then:

1. resolves every version with one `join_all` — single-flight per release key, so two
   dependencies on the same repo still cost one request;
2. `tokio::spawn`s a download task per dependency (a semaphore caps in-flight work);
3. awaits those handles **in order**, committing each as it arrives.

Because step 3 awaits in order, output still streams out dependency by dependency in exactly
the TS tool's sequence while later dependencies are still downloading. The `&mut self` on
`commit` is what serialises config writes — no lock needed, and the borrow checker enforces it.

`Arc<GitHubClient>` is the only shared ownership in the design; everything else is a borrow
from `Session`. The log lines a download would have printed are returned from the stage rather
than printed inside it, which is what makes the ordering property structural rather than
incidental.

Within a dependency, plain files download concurrently and then release assets do — the two
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

## 5. Parity contract

Verified against the installed `vendor@1.4.2` binary in isolated fixtures:

* Exit codes: `0` success, `1` for *every* failure including argument-parse errors
  (clap's default `2` is overridden).
* Argument errors use commander's wording: `error: unknown command 'x'`,
  `error: unknown option '--x'`, `error: missing required argument 'url/name'`.
* Bare `vendor` prints the root help to **stderr** and exits 1.
* ANSI colors are emitted unconditionally (no tty/`NO_COLOR` detection) — the TS tool
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
   undefined (reading 'match')` when installing a repo that is not already in the config —
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
5. **Failures the TS tool never handled** — a nonexistent tag, for instance — print the message
   the source already had for them and exit 1, instead of dumping an unhandled Octokit
   rejection and exiting 127.
6. **`vendor login` needs no config file**, and **`vendor update <name>` honours the `default`
   block** (the TS tool read the un-merged config entry there and reported "No repository
   found").
7. **`vendor install owner/repo` keeps a configured dependency's own repository URL** rather
   than rewriting it to the `https://www.github.com/...` form the shorthand expands to.
8. **`releaseRegex` compiles with `fancy-regex`**, so JavaScript patterns using lookaround keep
   working.
