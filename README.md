# vendorfiles-rs <!-- omit from toc -->

[![crates.io Version](https://img.shields.io/crates/v/vendorfiles)](https://crates.io/crates/vendorfiles)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/Araxeus/vendorfiles/blob/main/LICENSE)
[![Maintenance](https://img.shields.io/badge/Maintained%3F-yes-green.svg)](https://github.com/Araxeus/vendorfiles)

A Rust rewrite of [vendorfiles](https://github.com/Araxeus/vendorfiles) - pull files from
GitHub repositories and keep them up to date. Think of it like a package manager, but for
individual files: CSS libraries, binaries, config files, whatever you need.

The CLI surface, config files and lockfiles are byte-compatible with the original
TypeScript/Bun tool, so you can drop this binary onto an existing project and keep going.

- Download files directly from any GitHub repo
- Grab release assets, including extracting members from zip/tar/tar.gz/crx archives
- Track versions via releases or commit hashes
- Configure with TOML, YAML, JSON, or `package.json`
- Parallel, streaming downloads on a Tokio runtime
- Single static binary, no Node runtime

## Table of Contents <!-- omit from toc -->

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
  - [Basic Setup](#basic-setup)
  - [Custom Output Paths](#custom-output-paths)
  - [Renaming Files](#renaming-files)
  - [Commit-Based Versioning](#commit-based-versioning)
  - [GitHub Releases](#github-releases)
  - [Filtering Releases](#filtering-releases)
  - [Locking Dependencies](#locking-dependencies)
  - [Default Options](#default-options)
- [Commands](#commands)
- [Authentication](#authentication)
- [Lockfile](#lockfile)
- [JSON Schema](#json-schema)
- [Differences from the TypeScript version](#differences-from-the-typescript-version)
- [Development](#development)
- [License](#license)

## Installation

**From a release:** download the archive for your platform from the
[releases page](https://github.com/Araxeus/vendorfiles/releases) and put `vendor` on your
`PATH`.
That installs a binary named `vendor`. No system libraries are needed on any platform.

**From [Cargo](https://crates.io/crates/vendorfiles):**

```bash
cargo install vendorfiles
```

**From source:**

```bash
git clone https://github.com/Araxeus/vendorfiles-rs
cd vendorfiles-rs
cargo install --path crates/vendorfiles
```

**Build without installing:**

```bash
cargo build --release        # target/release/vendor
```

## Quick Start

Create a `vendor.json` in your project:

```json
{
    "vendorDependencies": {
        "Coloris": {
            "version": "v0.17.1",
            "repository": "https://github.com/mdbassit/Coloris",
            "files": ["dist/coloris.min.js", "dist/coloris.min.css"]
        }
    }
}
```

Run:

```bash
vendor sync
```

Your files are now in `./vendor/Coloris/`.

## Configuration

Vendorfiles looks for a config file in this order: `vendor.toml`, `vendor.yml`, `vendor.yaml`,
`vendor.json`, `package.json`. Only the current directory is searched - there is no upward
walk. Point somewhere else with `-c` or `VENDOR_CONFIG`.

All examples below are JSON; TOML and YAML work identically. See [`examples/`](./examples/).

### Basic Setup

```json
{
    "vendorDependencies": {
        "Cooltipz": {
            "version": "v2.2.0",
            "repository": "https://github.com/jackdomleo7/Cooltipz.css",
            "files": ["cooltipz.min.css", "LICENSE"]
        }
    }
}
```

By default files are saved to `./vendor/{dependency-name}/`.

### Custom Output Paths

Change the base folder with `vendorConfig`:

```json
{
    "vendorConfig": {
        "vendorFolder": "./my-vendors"
    }
}
```

Each dependency can override its own folder. `{vendorFolder}` expands to the base folder, and
a dependency that sets `vendorFolder` does **not** get its name appended:

```json5
{
    "vendorConfig": { "vendorFolder": "./my-vendors" },
    "vendorDependencies": {
        "Cooltipz": {
            "version": "v2.2.0",
            "repository": "https://github.com/jackdomleo7/Cooltipz.css",
            "files": ["cooltipz.min.css"],
            "vendorFolder": "{vendorFolder}/Cooltipz" // → ./my-vendors/Cooltipz
        },
        "Coloris": {
            "version": "v0.17.1",
            "repository": "https://github.com/mdbassit/Coloris",
            "files": ["dist/coloris.min.js"],
            "vendorFolder": "{vendorFolder}" // → ./my-vendors/
        }
    }
}
```

### Renaming Files

Use an object with `source: destination`:

```json
{
    "vendorDependencies": {
        "Coloris": {
            "version": "v0.17.1",
            "repository": "https://github.com/mdbassit/Coloris",
            "files": [
                "dist/coloris.min.js",
                { "LICENSE": "../licenses/COLORIS_LICENSE" }
            ]
        }
    }
}
```

### Commit-Based Versioning

By default versions track GitHub releases. To track a file's latest commit instead, use
`hashVersionFile`:

```json
{
    "vendorDependencies": {
        "Cooltipz": {
            "repository": "https://github.com/jackdomleo7/Cooltipz.css",
            "version": "f6ec482ea395cead4fd849c05df6edd8da284a52",
            "hashVersionFile": "package.json",
            "files": ["cooltipz.min.css", "package.json"]
        },
        "Coloris": {
            "repository": "https://github.com/mdbassit/Coloris",
            "version": "v0.17.1",
            "hashVersionFile": true,
            "files": ["dist/coloris.min.js"]
        }
    }
}
```

- **String**: track that file's latest commit hash
- **`true`**: track the first entry in `files`

### GitHub Releases

Download release assets with `{release}/` in the path. `{version}` expands to the semver core
of the tag - the first `x.y.z` found, or the tag with leading `v`s stripped:

```json
{
    "vendorDependencies": {
        "fzf": {
            "version": "0.38.0",
            "repository": "https://github.com/junegunn/fzf",
            "files": [
                "LICENSE",
                "{release}/fzf-{version}-linux_amd64.tar.gz",
                { "{release}/fzf-{version}-windows_amd64.zip": "fzf-windows.zip" }
            ]
        }
    }
}
```

**Extracting from archives** - give a list (keep names) or a map (rename):

```json
{
    "vendorDependencies": {
        "fzf": {
            "version": "0.38.0",
            "repository": "https://github.com/junegunn/fzf",
            "files": [
                {
                    "{release}/fzf-{version}-linux_amd64.tar.gz": ["fzf"],
                    "{release}/fzf-{version}-windows_amd64.zip": {
                        "fzf.exe": "my-custom-fzf.exe"
                    }
                }
            ]
        }
    }
}
```

The container format is detected from the file's magic bytes, not its name: zip, tar, gzip,
tar.gz/tgz, and the zip-based `.crx`/`.xpi` extension packages.

### Filtering Releases

`releaseRegex` controls which releases count as "latest". It is tested against each release's
tag and title, newest first.

```json
{
    "vendorDependencies": {
        "fzf": {
            "version": "0.38.0",
            "repository": "https://github.com/junegunn/fzf",
            "releaseRegex": "^v\\d+\\.\\d+\\.\\d+$",
            "files": ["{release}/fzf-{version}-linux_amd64.tar.gz"]
        }
    }
}
```

Common patterns:

- Semver only: `"^v\\d+\\.\\d+\\.\\d+$"`
- Exclude pre-releases: `"^v(?!.*-(?:alpha|beta)).*"`
- Match a title containing "stable": `"stable"`

Patterns are compiled with [`fancy-regex`](https://docs.rs/fancy-regex), so the lookaround and
backreferences a JavaScript pattern may use are supported.

> **Note:** Use double escaping (`\\d`) in JSON strings.

### Locking Dependencies

`locked: true` pins a dependency:

```json
{
    "vendorDependencies": {
        "Coloris": {
            "version": "v0.17.1",
            "repository": "https://github.com/mdbassit/Coloris",
            "files": ["dist/coloris.min.js"],
            "locked": true
        }
    }
}
```

Locked dependencies are still downloaded by `vendor sync` if missing, are skipped by
`vendor update`, and do not appear in `vendor outdated`.

### Default Options

A `default` (or `defaultVendorOptions`) object supplies values for every dependency that does
not set them:

```yml
vendorConfig:
  vendorFolder: .
default:
  vendorFolder: "{vendorFolder}"
  repository: https://github.com/nushell/nu_scripts
  hashVersionFile: true
vendorDependencies:
  nu-winget-completions:
    files: custom-completions/winget/winget-completions.nu
    version: 912bea4588ba089aebe956349488e7f78e56061c
  nu-cargo-completions:
    files: custom-completions/cargo/cargo-completions.nu
    version: afde2592a6254be7c14ccac520cb608bd1adbaf9
```

## Commands

```text
Usage: vendor command [options]

Options:
  -c, --config <file/folder path>             Config file path / Folder containing the config file
  -p, --plain                                 Print plain lines instead of a live display
  -v, --version                               output the current version
  -h, --help                                  display help for command

Commands:
  sync|s [options]                            Sync config file
  update|upgrade [options] [names...]         Update outdated dependencies
  outdated|o                                  List outdated dependencies
  install|add [options] <url/name> [version]  Install a dependency
  uninstall|remove [names...]                 Uninstall dependencies
  login|auth [token]                          Login to GitHub
  help [command]                              display help for command
```

Both root options are global, so they read naturally on either side of the subcommand:
`vendor -c ./conf.json sync` and `vendor sync -c ./conf.json` are the same command. `-c` requires
its value — naming the option is only ever a request for a specific config — so it can never claim
a dependency name by accident. The location can also come from `VENDOR_CONFIG`; `-c` wins if both
are set.

`-p`/`--plain` turns the live display off and prints the plain `INFO:`/`SUCCESS:` lines instead —
the same output a redirected stdout gets. It works on either side of the subcommand
(`vendor -p sync`, `vendor sync --plain`).

| Command | What it does |
| --- | --- |
| `vendor sync` | Download everything the config declares. `-f`/`--force` re-downloads even when the lockfile agrees. |
| `vendor update [names...]` | Resolve each dependency's latest version and install it. `--pr` prints a Markdown bump summary instead of the usual logs (whole-project updates only). |
| `vendor outdated` | List dependencies with a newer version available. |
| `vendor install <url/name> [version]` | Add a dependency. Accepts a full URL, `owner/repo`, or a name to search for. `-n`/`--name` sets the config key; `-f`/`--files` lists the files. |
| `vendor uninstall <names...>` | Delete a dependency's files and remove it from the config and lockfile. |
| `vendor login [token]` | Store a GitHub token. With no argument, runs the OAuth device flow. |

Examples:

```bash
vendor sync
vendor sync -f
vendor update
vendor bump React
vendor outdated
vendor install React -n MyReact -f README.md
vendor add Araxeus/vendorfiles v1.0.0 -f README.md LICENSE
vendor i https://github.com/th-ch/youtube-music -f "{release}/YouTube-Music-{version}.exe"
vendor remove React youtube-music
vendor login
```

Every failure exits with code `1`; success exits `0`.

## Authentication

Anonymous requests to the GitHub API are limited to 60 per hour, so `vendor` warns when it is
running without credentials. Tokens are resolved in this order:

1. `GITHUB_TOKEN` environment variable
2. the platform's credential store
3. anonymous

Native stores are compiled in per platform - there is no cross-platform facade, and no system
library to install:

| Platform | Store | Survives |
| --- | --- | --- |
| Windows | Credential Manager (`windows-native-keyring-store`) | until deleted |
| macOS | login Keychain (`apple-native-keyring-store`) | until deleted |
| Linux | Secret Service (`zbus-secret-service-keyring-store`) | until deleted |
| Linux, no keyring daemon | kernel keyutils (`linux-keyutils-keyring-store`) | **until reboot** |
| anything else | none - `GITHUB_TOKEN` only | - |

On Linux the Secret Service is tried first, so a token persists wherever a keyring daemon is
running - gnome-keyring, KWallet or KeePassXC. Headless boxes, minimal containers and WSL
usually have none, and there the kernel keyutils store takes over: it always works, but it
holds secrets in kernel memory.

`vendor login` stores a token, either by verifying one you paste (`vendor login <token>`) or
through GitHub's OAuth device flow:

```text
$ vendor login
First, copy your one-time code: ABCD-1234
Then press [Enter] to continue in your web browser
Opening your web browser...
SUCCESS: Logged in successfully
```

`login` asks the store it used how long it keeps things, so when you land on the keyutils
fallback it tells you rather than letting the next boot look mysteriously anonymous:

```text
WARNING: this system's credential store keeps secrets in kernel memory, so the token will be gone after a reboot
```

Two ways to get persistence if you see that: run a Secret Service daemon (installing
`gnome-keyring` is usually enough - on a headless box it needs unlocking, see
[this note](https://docs.rs/zbus-secret-service-keyring-store)), or set `GITHUB_TOKEN` from
your shell profile or CI secrets.

`login` is the one command that does not need a config file - it works from any directory.

## Lockfile

Each dependency folder gets a `vendor-lock.json` recording what was written:

```json
{
  "Coloris": {
    "repository": "https://github.com/mdbassit/Coloris",
    "version": "v0.18.0",
    "files": {
      "dist/coloris.min.js": "coloris.min.js",
      "LICENSE": "COLORIS_LICENSE"
    }
  }
}
```

The keys are the inputs from your config (with `{version}` left un-expanded) and the values are
what landed on disk. `vendor sync` treats a dependency as stale when the version differs, a
recorded file is missing, or the `files` declaration changed.

## JSON Schema

[`vendorfiles.schema.json`](./vendorfiles.schema.json) describes the config format, so editors
can validate and autocomplete it:

```json
{
    "$schema": "https://raw.githubusercontent.com/Araxeus/vendorfiles/refs/heads/main/vendorfiles.schema.json",
    "vendorDependencies": {
        //...
    }
}
```

## Differences from the TypeScript version

CLI help text, argument errors, exit codes, log wording, ANSI colours, lockfile bytes and
config write-back formatting are all matched deliberately - the help fixtures in
[`crates/vendorfiles/tests/fixtures/help`](./crates/vendorfiles/tests/fixtures/help) are captured
from `vendorfiles@1.4.2` and asserted byte-for-byte in the test suite. The only departures are the
two lines `-p`/`--plain` adds and takes away: the fixtures keep the reference text, and the test
applies that delta explicitly.

The main difference:
Version lookups run concurrently, and dependencies download
   in parallel (up to 8 at a time) while results are still committed strictly in config order -
   so the log is byte-identical to the original's but arrives sooner. Within one dependency the
   original's log order was left to chance; here it follows the `files` array.

Measured against `vendorfiles@1.4.2` with 8 dependencies:

| | TypeScript | Rust |
| --- | --- | --- |
| `sync` (nothing downloaded yet) | 3127 ms | 893 ms |
| `sync` (everything up to date) | 721 ms | 66 ms |
| `outdated` | 3426 ms | 908 ms |
| `--version` | 706 ms | 54 ms |

## Development

```bash
cargo xtask ci   # every gate CI applies: check, rustfmt, clippy, tests
```

Or one at a time. The lint groups live in `[workspace.lints]`, so a bare `cargo clippy` is the
same gate as the workflow; the flags below are what CI spells out and are redundant locally.

```bash
cargo check --workspace --all-targets
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

The workspace is three crates:

| Crate | Role |
| --- | --- |
| `crates/vendorfiles_core` | Library: config, lockfile, GitHub client, archive handling, operations. Typed errors via `thiserror`; never exits the process. |
| `crates/vendorfiles` | The `vendor` binary: Commander-compatible help and errors, `anyhow` at the boundary. |
| `xtask` | `cargo xtask ci` - the four checks above, stopping at the first failure. `cargo xtask release` - clean-tree check, version prompt, manifest update, format, commit, tag. |

See [`docs/DESIGN.md`](./docs/DESIGN.md) for the module layout, ownership model, and the full
parity contract.

## License

MIT
