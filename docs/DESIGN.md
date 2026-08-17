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
│   │       │   ├── mod.rs    # Workspace: discovery, defaults merge, mutation, write-back
│   │       │   ├── format.rs # ConfigFormat, indent detection, per-format (de)serialisation
│   │       │   └── document.rs # ConfigDocument: format-preserving editable document
│   │       ├── lockfile.rs   # vendor-lock.json read/write, config→lock file mapping
│   │       ├── fsx.rs        # delete-file-and-empty-parents, stream-to-file
│   │       ├── archive.rs    # magic-byte sniffing + zip/tar/tar.gz/gz/crx extraction
│   │       ├── github/
│   │       │   ├── mod.rs    # GitHubClient: releases (cached), commits, search, downloads
│   │       │   └── auth.rs   # token resolution, keyring, OAuth device flow
│   │       └── ops/
│   │           ├── mod.rs
│   │           ├── install.rs
│   │           ├── sync.rs
│   │           └── uninstall.rs
│   └── vendor-cli/           # binary `vendor`: clap derive, commander-compatible help/errors
│       └── src/
│           ├── main.rs
│           ├── cli.rs
│           └── help.rs       # verbatim commander help text + error rendering
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

`Workspace` owns all config data for the process lifetime. Operations take `&mut Workspace`
and *clone the single `VendorDependency`* they act on (a handful of small `String`s) rather
than juggling a split borrow of `dependencies` against `file`. This keeps every signature
lifetime-free and is measurably irrelevant next to a network round-trip.

Borrowed data is used where it is free and unambiguous: `&VendorDependency`,
`&VendorConfig`, `&Path` flow down into pure helpers (`dependency_folder`, `lock_files_for`).
The only shared mutable state is `RunOptions` (`--pr` mode) which is a process-global
`OnceLock<RunOptions>` set once from `main`, mirroring the TS module-level singleton.

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
    api: octocrab::Octocrab,        // REST: releases, commits, search
    http: reqwest::Client,          // streaming: raw contents + release assets
    token: Option<SecretToken>,
    releases: Mutex<HashMap<ReleaseKey, Arc<Release>>>,
}
```

`Arc<Release>` so cache hits do not clone asset lists. The cache reproduces the TS
lookup quirk: a `owner/name/tag` key also matches any cached entry for the same repo whose
`tag_name` equals the requested tag.

### 3.5 Errors

`VendorError` (thiserror) in the library; every variant's `Display` is the exact string the
TS tool prints after the `ERROR: ` prefix. `vendor-cli` uses `anyhow` at the boundary and
renders `\x1b[31mERROR: {e}\x1b[0m` to stderr, exit 1.

## 4. Concurrency

* Across dependencies: version resolution runs concurrently (single-flight per repo via the
  release cache), then installs run **in config order** so log output stays deterministic and
  matches the TS tool.
* Within a dependency: all plain files download concurrently, then all release assets are
  fetched/extracted concurrently — exactly the two `Promise.all` batches in `commands.ts`.
* Downloads stream to disk (`reqwest` byte stream → `tokio::fs::File`); nothing is buffered
  whole in memory except archives that must be sniffed and extracted.

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
4. **TOML comments survive** a version bump (`toml_edit`); the TS tool drops them.
