# vendorfiles-rs <!-- omit from toc -->

[![crates.io Version](https://img.shields.io/crates/v/vendorfiles)](https://crates.io/crates/vendorfiles)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/Araxeus/vendorfiles-rs/blob/main/LICENSE)
[![Maintenance](https://img.shields.io/badge/Maintained%3F-yes-green.svg)](https://github.com/Araxeus/vendorfiles-rs)
[![Docs](https://img.shields.io/badge/docs-araxeus.github.io-blue)](https://araxeus.github.io/vendorfiles-rs/)

<p align="center">
  <a href="https://araxeus.github.io/vendorfiles-rs/">
    <picture>
      <source media="(prefers-color-scheme: dark)"
              srcset="https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/site/public/readme-banner-dark.svg">
      <img alt="Read the vendorfiles docs at araxeus.github.io/vendorfiles-rs" width="800"
           src="https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/site/public/readme-banner-light.svg">
    </picture>
  </a>
</p>

Get the files and tools you need, and keep them pinned and current. Name what you want from a
GitHub repository, a release or any URL. `vendor` downloads it, writes a lockfile, and tells you
when there's something newer.

There are two ways to use it:

- **In a project.** Pull CSS libraries, scripts, fonts or config files into your repository. Commit
  the config and the lockfile, and everyone gets the same bytes.
- **On your machine.** Install command-line tools and apps by name, like `vendor add rg fd`, on
  Windows, macOS and Linux. See [Global programs](#global-programs).

What it can fetch:

- Single files, [whole folders and glob patterns](#folders-and-globs) from any repository
- [Release assets](#release-assets), unpacking only the files you need from zip, tar, 7z and crx
  archives
- Files from [any URL](#downloading-from-other-sites), with the latest version read from a page, a
  redirect or JSON
- [Programs by name](#program-registry) from the registry, with no asset names to look up
- [Windows installers](#installers), run silently and uninstalled later

How it keeps them current:

- Track versions by release, by git tag or by commit, or [lock](#locking-a-dependency) one in place
- Download in parallel, streaming to disk, and reuse unchanged files on update
- Write the config in TOML, YAML, JSON, JSON5 or `package.json`
- Install one static binary, with nothing else to set up

vendorfiles-rs is a Rust rewrite of [vendorfiles](https://github.com/Araxeus/vendorfiles).

## Table of Contents <!-- omit from toc -->

- [Installation](#installation)
  - [Install script](#install-script)
  - [Other ways to install](#other-ways-to-install)
- [Quick start](#quick-start)
  - [Global programs](#global-programs)
- [Configuration](#configuration)
  - [Where vendor looks for the config](#where-vendor-looks-for-the-config)
  - [Dependency fields](#dependency-fields)
  - [Output folders and file names](#output-folders-and-file-names)
  - [Folders and globs](#folders-and-globs)
  - [Versions and releases](#versions-and-releases)
  - [Downloading from other sites](#downloading-from-other-sites)
  - [Locking a dependency](#locking-a-dependency)
  - [Aliases](#aliases)
  - [Installers](#installers)
  - [Default options](#default-options)
- [Commands](#commands)
  - [Options for every command](#options-for-every-command)
  - [Command list](#command-list)
- [Program registry](#program-registry)
  - [What gets written](#what-gets-written)
  - [Finding a program](#finding-a-program)
  - [Adding to the registry](#adding-to-the-registry)
  - [Programs you can add by name](#programs-you-can-add-by-name)
- [Keeping vendorfiles updated](#keeping-vendorfiles-updated)
- [Authentication](#authentication)
  - [Where the token comes from](#where-the-token-comes-from)
  - [Logging in](#logging-in)
  - [Checking your login](#checking-your-login)
  - [Where the token is stored](#where-the-token-is-stored)
- [Lockfile](#lockfile)
  - [One lockfile for everything](#one-lockfile-for-everything)
- [JSON Schema](#json-schema)
- [Differences from the TypeScript version](#differences-from-the-typescript-version)
- [Development](#development)
  - [Checks](#checks)
  - [Workspace layout](#workspace-layout)
  - [Automation](#automation)
  - [Docs site](#docs-site)
- [License](#license)

## Installation

### Install script

The install script is the quickest way to get `vendor`.

```bash
# Linux (x86_64) and macOS (Apple Silicon)
curl -fsSL https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/install.sh | bash
```

```powershell
# Windows (x86_64)
irm https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/main/install.ps1 | iex
```

On Linux and macOS the script installs to `/usr/local/bin` if that folder is writable, and to
`~/.local/bin` otherwise. On Windows it installs to `~\.local\bin`.

The script also sets up [global programs](#global-programs), so `vendor` can install tools for your
whole machine. It lists every change it makes to your shell or user environment, and it never uses
`sudo`.

### Other ways to install

#### From GitHub Releases

Download the archive for your platform from the
[latest release](https://github.com/Araxeus/vendorfiles-rs/releases/latest), and put `vendor`
(`vendor.exe` on Windows) on your `PATH`. Older versions are on the
[releases page](https://github.com/Araxeus/vendorfiles-rs/releases).

The binary needs no system libraries on any platform. The Linux build is statically linked against
musl, so it runs on any distribution, whatever its glibc version.

#### With Cargo

vendorfiles is [on crates.io](https://crates.io/crates/vendorfiles):

```bash
cargo install vendorfiles
```

#### From source

```bash
git clone https://github.com/Araxeus/vendorfiles-rs
cd vendorfiles-rs
cargo install --path crates/vendorfiles
```

To build without installing, run `cargo build --release`. The binary lands in
`target/release/vendor`.

## Quick start

Create a config file in your project. This one asks for two files from the
[Coloris](https://github.com/mdbassit/Coloris) repository, at release `v0.17.1`:

<!-- formats: files -->
<details open>
<summary>vendor.json</summary>

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

</details>

<details>
<summary>vendor.yml</summary>

```yml
vendorDependencies:
  Coloris:
    version: v0.17.1
    repository: https://github.com/mdbassit/Coloris
    files:
      - dist/coloris.min.js
      - dist/coloris.min.css
```

</details>

<details>
<summary>vendor.toml</summary>

```toml
[vendorDependencies.Coloris]
version = 'v0.17.1'
repository = 'https://github.com/mdbassit/Coloris'
files = ['dist/coloris.min.js', 'dist/coloris.min.css']
```

</details>
<!-- /formats -->

Then download them:

```bash
vendor sync
```

The files are now in `./vendor/Coloris/`. Later, `vendor outdated` tells you when Coloris has a
newer release, and `vendor update` moves you to it.

### Global programs

`vendor` can also install programs for your whole machine, like `rg` and `fd`. The
[install script](#install-script) sets this up for you, or you can do it by hand.

1. Create a config file such as `~/vendor.yml`. Setting `vendorFolder` under `default` puts each
   program straight into `/usr/local/bin`, instead of a folder of its own.

<!-- formats: files, yml first -->
<details open>
<summary>vendor.yml</summary>

```yml
vendorConfig:
  vendorFolder: /usr/local/bin
default:
  vendorFolder: '{vendorFolder}'
  locked: false
```

</details>

<details>
<summary>vendor.json</summary>

```json
{
    "vendorConfig": {
        "vendorFolder": "/usr/local/bin"
    },
    "default": {
        "vendorFolder": "{vendorFolder}",
        "locked": false
    }
}
```

</details>

<details>
<summary>vendor.toml</summary>

```toml
[vendorConfig]
vendorFolder = '/usr/local/bin'

[default]
vendorFolder = '{vendorFolder}'
locked = false
```

</details>
<!-- /formats -->

2. Make sure the `vendorFolder` you chose is on your `PATH`.
3. Set the `DEFAULT_VENDOR_CONFIG` environment variable to `~/vendor.yml`, or to the folder that
   holds it. `vendor` then finds this config from any folder.
4. Install some programs:

   ```bash
   vendor add vendorfiles rg fd
   ```

`vendor`, `rg` and `fd` are now in `/usr/local/bin`, and `vendor update` upgrades all of them.

## Configuration

A config file lists the files you want, grouped into dependencies. Every example in these docs
comes in JSON, YAML and TOML, and the [`examples/`](./examples/) folder has complete files in each
format.

### Where vendor looks for the config

`vendor` looks in the current folder for these files, in this order:

1. `vendor.toml`
2. `vendor.yml`
3. `vendor.yaml`
4. `vendor.json`
5. `vendor.json5`
6. `package.json`

It doesn't search parent folders. To use a config somewhere else, pass `-c <path>` or set
`VENDOR_CONFIG`.

If no config is found, `vendor` tries `DEFAULT_VENDOR_CONFIG` last. Set it to a config file or a
folder to give every folder a fallback config. Paths in that config stay relative to the config
itself, so its dependencies land next to it, not in the folder you ran `vendor` from. An explicit
`-c` skips the fallback.

### Dependency fields

Each dependency names a repository, a version and the files to take from it:

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  Cooltipz:
    version: v2.2.0
    repository: https://github.com/jackdomleo7/Cooltipz.css
    files:
      - cooltipz.min.css
      - LICENSE
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.Cooltipz]
version = 'v2.2.0'
repository = 'https://github.com/jackdomleo7/Cooltipz.css'
files = ['cooltipz.min.css', 'LICENSE']
```

</details>
<!-- /formats -->

| Field | What it holds |
| --- | --- |
| `repository` | The GitHub repository URL. Use `url` instead for anything else |
| `version` | A release tag like `v1.0.0`, or a commit hash when tracking commits |
| `files` | The files to download. A path ending in `/` takes a folder, and `*` makes a pattern ([details](#folders-and-globs)) |
| `vendorFolder` | Where to save this dependency's files ([output folders](#output-folders)) |
| `hashVersionFile` | Track a file's commits instead of releases ([details](#tracking-commits-instead-of-releases)) |
| `releaseRegex` | Which releases count when looking for the latest ([details](#choosing-which-releases-count)) |
| `locked` | Keep `vendor update` away from this dependency ([details](#locking-a-dependency)) |
| `alias` | Other names for the dependency, usable in any command ([details](#aliases)) |
| `installer` | Run the downloaded file as an installer instead of keeping it ([details](#installers)) |
| `url` | Where a dependency that isn't on GitHub downloads from ([details](#downloading-from-other-sites)) |
| `versionUrl` | `url` dependencies only. Where the latest version comes from |
| `versionRegex` | `url` dependencies only. Picks the version out of what `versionUrl` returns |
| `versionSelector` | `url` dependencies only. Picks the element of a `versionUrl` page that holds the version |
| `versionJsonPointer` | `url` dependencies only. Picks the value of a JSON `versionUrl` answer that holds the version |

The first three are all most dependencies need.

### Output folders and file names

#### Output folders

Files land in `./vendor/<dependency name>/` by default. To change the base folder, set
`vendorFolder` under `vendorConfig`:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorConfig": {
        "vendorFolder": "./my-vendors"
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorConfig:
  vendorFolder: ./my-vendors
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorConfig]
vendorFolder = './my-vendors'
```

</details>
<!-- /formats -->

A dependency can also set its own `vendorFolder`. Inside it, `{vendorFolder}` stands for the base
folder. A dependency with its own folder does **not** get its name added to the path:

<!-- formats -->
<details open>
<summary>JSON</summary>

```jsonc
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

</details>

<details>
<summary>YAML</summary>

```yml
vendorConfig:
  vendorFolder: ./my-vendors
vendorDependencies:
  Cooltipz:
    version: v2.2.0
    repository: https://github.com/jackdomleo7/Cooltipz.css
    files:
      - cooltipz.min.css
    vendorFolder: '{vendorFolder}/Cooltipz' # → ./my-vendors/Cooltipz
  Coloris:
    version: v0.17.1
    repository: https://github.com/mdbassit/Coloris
    files:
      - dist/coloris.min.js
    vendorFolder: '{vendorFolder}' # → ./my-vendors/
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorConfig]
vendorFolder = './my-vendors'

[vendorDependencies.Cooltipz]
version = 'v2.2.0'
repository = 'https://github.com/jackdomleo7/Cooltipz.css'
files = ['cooltipz.min.css']
vendorFolder = '{vendorFolder}/Cooltipz' # → ./my-vendors/Cooltipz

[vendorDependencies.Coloris]
version = 'v0.17.1'
repository = 'https://github.com/mdbassit/Coloris'
files = ['dist/coloris.min.js']
vendorFolder = '{vendorFolder}' # → ./my-vendors/
```

</details>
<!-- /formats -->

#### Renaming files

To save a file under another name or in another folder, write it as an object that maps the source
path to the destination:

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  Coloris:
    version: v0.17.1
    repository: https://github.com/mdbassit/Coloris
    files:
      - dist/coloris.min.js
      - LICENSE: ../licenses/COLORIS_LICENSE
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.Coloris]
version = 'v0.17.1'
repository = 'https://github.com/mdbassit/Coloris'
files = ['dist/coloris.min.js', { LICENSE = '../licenses/COLORIS_LICENSE' }]
```

</details>
<!-- /formats -->

An absolute destination is used exactly as written, like an absolute `vendorFolder`.

If every file has a destination, you can leave out the array:

```json
"files": { "LICENSE": "../licenses/COLORIS_LICENSE" }
```

### Folders and globs

A `files` path that ends in `/` takes the whole folder. A path with `*` or `?` is a glob pattern,
and takes every file that matches:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "skills": {
            "version": "e4bb5d960d87126ff3b3a8af1a104b7db67560ba",
            "repository": "https://github.com/anthropics/skills",
            "files": {
                "skills/canvas-design/canvas-fonts/": "fonts/",
                "skills/**/SKILL.md": "{parent}.md"
            }
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  skills:
    version: e4bb5d960d87126ff3b3a8af1a104b7db67560ba
    repository: https://github.com/anthropics/skills
    files:
      skills/canvas-design/canvas-fonts/: fonts/
      skills/**/SKILL.md: '{parent}.md'
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.skills]
version = 'e4bb5d960d87126ff3b3a8af1a104b7db67560ba'
repository = 'https://github.com/anthropics/skills'

[vendorDependencies.skills.files]
'skills/canvas-design/canvas-fonts/' = 'fonts/'
'skills/**/SKILL.md' = '{parent}.md'
```

</details>
<!-- /formats -->

This saves every font under `fonts/`, keeping its subfolders, and each skill's `SKILL.md` as
`<skill>.md`.

Folders and globs also work [inside archives](#inside-archives).

#### Folders

The folder's contents land in the destination, keeping their own subfolders. Without a
destination, the folder keeps its own name, so `"skills/canvas-design/canvas-fonts/"` saves to
`canvas-fonts/`.

#### Patterns

Only `*` and `?` make a path a pattern, so paths like `app/[id].tsx` stay literal. Inside a
pattern, the rest of the glob syntax works too:

| Pattern | Matches |
| --- | --- |
| `dist/*` | Files directly in `dist` |
| `dist/**` | Every file in `dist`, at any depth |
| `dist/**/*.js` | `.js` files in `dist`, at any depth |
| `*.{md,txt}` | `.md` and `.txt` files at the top of the repository |
| `docs/[ab]*.md` | `.md` files in `docs` that start with `a` or `b` |

- Matching is case sensitive, on `/`-separated paths, on every platform.
- Dotfiles match like any other file, and `.gitignore` doesn't apply.
- `\` makes the next character literal, so `a\*b` matches a file named `a*b`.
- `{version}` and `{tag}` are filled in before matching.

#### Destinations

A destination that ends in `/` keeps each file's path below the pattern's fixed folder. With
`"dist/**/*.js": "scripts/"`, `dist/utils/a.js` saves as `scripts/utils/a.js`. A pattern with no
destination, like `"files": ["dist/**/*.js"]`, does the same into the dependency folder.

Any other destination is a file name, built from these placeholders:

| Placeholder | For `skills/pdf/a.tar.gz` matched by `skills/**` |
| --- | --- |
| `{path}` | `pdf/a.tar.gz` |
| `{filename}` | `a.tar.gz` |
| `{stem}` | `a.tar` |
| `{ext}` | `gz` |
| `{parent}` | `pdf` |
| `{version}`, `{version:<joiner>}`, `{tag}` | The version, as for [release assets](#release-assets) |

`{ext}` is empty for names like `Makefile` and `.bashrc`.

#### Errors

`vendor` checks the whole selection before it writes anything, and stops when:

- A folder or pattern matches no files.
- Two files would land on the same destination.
- A match is a symlink or a submodule.
- `hashVersionFile` is `true` and the first file is a folder or pattern. Name the file to track
  instead.

#### Inside archives

An archive's members can be folders and patterns too, for release assets and `url` downloads
alike:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "rg": {
            "version": "14.1.1",
            "repository": "https://github.com/BurntSushi/ripgrep",
            "files": {
                "{release}/ripgrep-{version}-x86_64-pc-windows-msvc.zip": {
                    "ripgrep-{version}-x86_64-pc-windows-msvc/rg.exe": "rg.exe",
                    "ripgrep-{version}-x86_64-pc-windows-msvc/doc/": "doc/",
                    "ripgrep-{version}-x86_64-pc-windows-msvc/complete/*.ps1": "complete/"
                }
            }
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  rg:
    version: 14.1.1
    repository: https://github.com/BurntSushi/ripgrep
    files:
      '{release}/ripgrep-{version}-x86_64-pc-windows-msvc.zip':
        ripgrep-{version}-x86_64-pc-windows-msvc/rg.exe: rg.exe
        ripgrep-{version}-x86_64-pc-windows-msvc/doc/: doc/
        ripgrep-{version}-x86_64-pc-windows-msvc/complete/*.ps1: complete/
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.rg]
version = '14.1.1'
repository = 'https://github.com/BurntSushi/ripgrep'

[vendorDependencies.rg.files.'{release}/ripgrep-{version}-x86_64-pc-windows-msvc.zip']
'ripgrep-{version}-x86_64-pc-windows-msvc/rg.exe' = 'rg.exe'
'ripgrep-{version}-x86_64-pc-windows-msvc/doc/' = 'doc/'
'ripgrep-{version}-x86_64-pc-windows-msvc/complete/*.ps1' = 'complete/'
```

</details>
<!-- /formats -->

Members follow the same rules as repository paths, and a leading `./` in a tarball's member names
is ignored. In a list of members, a folder keeps its own name and a pattern keeps paths below its
fixed folder.

To list its members, `vendor` downloads the whole archive, even a `.zip` it could otherwise read
in pieces.

#### Requests

An install lists each repository folder with one GitHub API request, and downloads the files from
`raw.githubusercontent.com`, which doesn't count against the rate limit. A `sync` with nothing
changed makes no request at all, because the lockfile records every file a folder or pattern
matched.

An update downloads only the repository files that changed. The lockfile records each file's git
blob, and a file whose blob, destination and folder are all the same as before stays in place.
A missing file is downloaded again, and `--force` downloads everything. `vendor` doesn't check
the content of a kept file, so a local edit to one survives the update.

### Versions and releases

By default, `version` holds a release tag, and `vendor update` moves a dependency to the newest
release. You can also [track commits](#tracking-commits-instead-of-releases) instead.

#### Release assets

A file that starts with `{release}/` comes from the release's assets instead of the repository.
`{version}` in a path expands to the version number in the release tag. That's the first dotted
run of numbers in the tag, like `1.2.3` or `2.16.60.2`, or the tag with its leading `v`s removed:

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  fzf:
    version: 0.38.0
    repository: https://github.com/junegunn/fzf
    files:
      - LICENSE
      - '{release}/fzf-{version}-linux_amd64.tar.gz'
      - '{release}/fzf-{version}-windows_amd64.zip': fzf-windows.zip
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.fzf]
version = '0.38.0'
repository = 'https://github.com/junegunn/fzf'
files = [
  'LICENSE',
  '{release}/fzf-{version}-linux_amd64.tar.gz',
  { '{release}/fzf-{version}-windows_amd64.zip' = 'fzf-windows.zip' },
]
```

</details>
<!-- /formats -->

`{version}` stays as written in your config, so the same paths keep working after
`vendor update`. `{tag}` works the same way but expands to the whole tag, like `v0.38.0`.

`{version:<joiner>}` splits the version into its parts and joins them again with `<joiner>`.
A part is a run of letters or digits, so `.`, `-` and `_` all separate parts:

| Version | Template | Result |
| --- | --- | --- |
| `10-1-0-26` | `{version:_}` | `10_1_0_26` |
| `26.06` | `7z{version:}-x64.exe` | `7z2606-x64.exe` |
| `1.86.0` | `boost_{version:_}.tar.gz` | `boost_1_86_0.tar.gz` |
| `3.12` | `python{version:}.zip` | `python312.zip` |

An asset name with `*` or `?` is a glob pattern, for projects whose asset names don't follow the
tag. `{release}/WinMerge-*-x64-PerUser-Setup.exe` finds the setup whatever the version. A
pattern must match exactly one asset in the release, and vendor stops with the matching names
when it matches several. Give a pattern a name to save the asset as, like
`{ "{release}/WinMerge-*-x64-PerUser-Setup.exe": "WinMerge-Setup.exe" }`, or the members to
extract from it.

#### Extracting files from archives

To take files out of an archive, map the archive to the files you want. A list keeps their names,
and a map renames them:

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  fzf:
    version: 0.38.0
    repository: https://github.com/junegunn/fzf
    files:
      - '{release}/fzf-{version}-linux_amd64.tar.gz':
          - fzf
        '{release}/fzf-{version}-windows_amd64.zip':
          fzf.exe: my-custom-fzf.exe
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.fzf]
version = '0.38.0'
repository = 'https://github.com/junegunn/fzf'
files = [
  { '{release}/fzf-{version}-linux_amd64.tar.gz' = ['fzf'], '{release}/fzf-{version}-windows_amd64.zip' = { 'fzf.exe' = 'my-custom-fzf.exe' } },
]
```

</details>
<!-- /formats -->

- The supported formats are zip, tar, gzip, xz, tar.gz, tar.xz, 7z and `.crx`/`.xpi`.
  `vendor` detects the format from the file's contents, not its name.
- Only the files you name are unpacked. Members can also be
  [folders and patterns](#inside-archives).
- For a large `.zip`, only those files are downloaded, using HTTP range requests. The rest of the
  archive never is.

#### Choosing which releases count

`releaseRegex` decides which releases count as the latest. `vendor` tests it against each
release's tag and title, newest first.

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  fzf:
    version: 0.38.0
    repository: https://github.com/junegunn/fzf
    releaseRegex: ^v\d+\.\d+\.\d+$
    files:
      - '{release}/fzf-{version}-linux_amd64.tar.gz'
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.fzf]
version = '0.38.0'
repository = 'https://github.com/junegunn/fzf'
releaseRegex = '^v\d+\.\d+\.\d+$'
files = ['{release}/fzf-{version}-linux_amd64.tar.gz']
```

</details>
<!-- /formats -->

These patterns cover the common cases:

- `"^v\\d+\\.\\d+\\.\\d+$"` accepts plain versions like `v1.2.3` and nothing else.
- `"^v(?!.*-(?:alpha|beta)).*"` skips alpha and beta pre-releases.
- `"stable"` accepts any release with "stable" in its title.

Patterns run on [`fancy-regex`](https://docs.rs/fancy-regex), so the lookarounds and
backreferences a JavaScript pattern may use work too.

> [!TIP]
> In a JSON string, write each backslash twice, like `\\d`.

#### Tracking commits instead of releases

To follow a file's latest commit instead of releases, set `hashVersionFile`. `version` then holds a
commit hash:

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  Cooltipz:
    repository: https://github.com/jackdomleo7/Cooltipz.css
    version: f6ec482ea395cead4fd849c05df6edd8da284a52
    hashVersionFile: package.json
    files:
      - cooltipz.min.css
      - package.json
  Coloris:
    repository: https://github.com/mdbassit/Coloris
    version: v0.17.1
    hashVersionFile: true
    files:
      - dist/coloris.min.js
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.Cooltipz]
repository = 'https://github.com/jackdomleo7/Cooltipz.css'
version = 'f6ec482ea395cead4fd849c05df6edd8da284a52'
hashVersionFile = 'package.json'
files = ['cooltipz.min.css', 'package.json']

[vendorDependencies.Coloris]
repository = 'https://github.com/mdbassit/Coloris'
version = 'v0.17.1'
hashVersionFile = true
files = ['dist/coloris.min.js']
```

</details>
<!-- /formats -->

`hashVersionFile` takes a path or `true`.

- A path tracks that file's latest commit.
- `true` tracks the first file in `files`.

### Downloading from other sites

A dependency that isn't on GitHub sets `url` instead of `repository`. When it downloads just that
URL, `files` is the name to save it under:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "FilePilot": {
            "url": "https://filepilot.tech/download/v{version}",
            "versionUrl": "https://filepilot.tech/download/latest",
            "versionRegex": "v(\\d+\\.\\d+\\.\\d+)",
            "files": "FPilot.exe"
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  FilePilot:
    url: https://filepilot.tech/download/v{version}
    versionUrl: https://filepilot.tech/download/latest
    versionRegex: v(\d+\.\d+\.\d+)
    files: FPilot.exe
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.FilePilot]
url = 'https://filepilot.tech/download/v{version}'
versionUrl = 'https://filepilot.tech/download/latest'
versionRegex = 'v(\d+\.\d+\.\d+)'
files = 'FPilot.exe'
```

</details>
<!-- /formats -->

- `files` that names no URL describes the `url` download. A string is the name it's saved under,
  and a map [extracts files from the archive](#extracting-files-from-archives), taking each member
  to its name.
- To download several URLs, map each one to its name, like
  `"files": { "{url}/a.exe": "a.exe", "https://cdn.example.com/b.js": "b.js" }`. `{url}` in a
  file expands to the `url` value.
- Map a URL to a list to extract members under their own paths, like
  `"files": { "{url}": ["bin/tool.exe"] }`.
- `{version}` and `{version:<joiner>}` work as for releases. `{tag}` is the version exactly as
  found, like `v1.2.3`.
- `vendor` never sends your GitHub token to these sites.

#### Finding the latest version

`versionUrl` says where the latest version comes from:

| `versionUrl` | The version is |
| --- | --- |
| A git repository ending in `.git` | Its newest tag |
| Any other URL | Read from the page it returns |
| `{url}` | Read from the download itself, like a userscript's `// @version 1.4.2` line. Quote it in YAML |
| Not set | `version` as written. Without one, the dependency installs once and `--force` downloads it again |

A git repository works on any host that serves git over HTTPS, like Codeberg or GitLab:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "forgejo": {
            "url": "https://codeberg.org/forgejo/forgejo/releases/download/{tag}",
            "versionUrl": "https://codeberg.org/forgejo/forgejo.git",
            "files": { "{url}/forgejo-{version}-linux-amd64": "forgejo" }
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  forgejo:
    url: https://codeberg.org/forgejo/forgejo/releases/download/{tag}
    versionUrl: https://codeberg.org/forgejo/forgejo.git
    files:
      '{url}/forgejo-{version}-linux-amd64': forgejo
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.forgejo]
url = 'https://codeberg.org/forgejo/forgejo/releases/download/{tag}'
versionUrl = 'https://codeberg.org/forgejo/forgejo.git'

[vendorDependencies.forgejo.files]
'{url}/forgejo-{version}-linux-amd64' = 'forgejo'
```

</details>
<!-- /formats -->

- Tags compare by number, so `v1.10.0` beats `v1.9.0`. `_` separates numbers too, as in
  `curl-8_10_0`. A dotted version beats a bare build number or date like `r1234` or
  `20240101`, but not a bare major like `v263` beside `v262.1`.
- Tags with `@`, like a monorepo's `plugin-legacy@6.1.0`, count only when `versionRegex` matches
  them. Scoped tags like `@astrojs/react@4.0.0` hold a `/`, so they never count.
- A tag is a prerelease, and is skipped, when its numbers are followed by a `-` (`v2.0.0-rc.1`),
  by `rc`, `cr`, `alpha`, `beta`, `pre` or `dev` (`go1.25rc1`), or by `a`, `b` or `m` and a digit
  (`v3.14.0a1`). Date tags like `2024-01-15` are skipped too.
- A `-` and digits alone, like ImageMagick's `7.1.1-41`, is a patch level when most tags look like
  that. It's a prerelease when plain releases outnumber it, or when releases often share its
  numbers, like npm's `v1.0.1-0` beside `v1.0.1`.
- With `versionRegex`, only the tags it matches count, prereleases included. A stable release
  still beats a prerelease of the same version.
- A repository with several tag schemes needs a `versionRegex` to say which one is current.
  golang/go keeps old `release.r60.3` tags beside `go1.25.0`, so it takes `"versionRegex": "^go"`.

#### Reading the version from a page

A page can answer with just the version, like `v1.2.3`. For anything bigger, `versionSelector` and
`versionRegex` pick the version out:

| `versionSelector` | `versionRegex` | `vendor` reads |
| --- | --- | --- |
| - | - | The whole response, which must be just the version |
| - | Set | The URL after redirects, then the page |
| Set | - | The text of the first element the selector matches |
| Set | Set | That element's text, then its HTML |

- `versionRegex` takes its first capture group, or the whole match when it has none.
- A version holds letters, digits and `.`, `_`, `+`, `-` or `@`, with at least one digit unless
  it's a full 40-character commit hash. Anything else, like a path or a JSON body, is refused.
- `versionSelector` takes a CSS selector, like `#latest` or `.release h2`. To get one, right-click
  the version in your browser, choose **Inspect**, then **Copy selector**.
- End the selector with `::attr(name)` to read an attribute instead of the text. A VS Code
  extension's manifest holds `<Identity Version="6.3.5" />`, so `Identity::attr(Version)` gives
  `6.3.5`. This is Scrapy's syntax rather than CSS, so add it after copying a selector.
- The example above follows `/download/latest` to a file named after the version. The
  [File Pilot starlog](https://filepilot.tech/starlog) works too. Its `.starlog-version` element
  reads `Beta v0.8.5`, so `"versionSelector": ".starlog-version"` with the same `versionRegex`
  gives `0.8.5`.
- Searching the element's HTML finds a version kept in an attribute, like
  `<a href="/download/v1.2.3">`.
- `vendor` reads the HTML the server sends. A version that JavaScript fills in later isn't there.

#### Reading the version from JSON

An API that answers in JSON takes `versionJsonPointer`, a
[JSON pointer](https://datatracker.ietf.org/doc/html/rfc6901) to the value that holds the version.
NuGet lists every version of a package:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "libclang": {
            "url": "https://www.nuget.org/api/v2/package/libclang.runtime.win-x64/{tag}",
            "versionUrl": "https://api.nuget.org/v3-flatcontainer/libclang.runtime.win-x64/index.json",
            "versionJsonPointer": "versions",
            "files": { "runtimes/win-x64/native/libclang.dll": "libclang.dll" }
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  libclang:
    url: https://www.nuget.org/api/v2/package/libclang.runtime.win-x64/{tag}
    versionUrl: https://api.nuget.org/v3-flatcontainer/libclang.runtime.win-x64/index.json
    versionJsonPointer: versions
    files:
      runtimes/win-x64/native/libclang.dll: libclang.dll
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.libclang]
url = 'https://www.nuget.org/api/v2/package/libclang.runtime.win-x64/{tag}'
versionUrl = 'https://api.nuget.org/v3-flatcontainer/libclang.runtime.win-x64/index.json'
versionJsonPointer = 'versions'

[vendorDependencies.libclang.files]
'runtimes/win-x64/native/libclang.dll' = 'libclang.dll'
```

</details>
<!-- /formats -->

- `data/0/version` reads `data[0].version`. A leading `/`, as the standard writes it, is optional.
  An empty pointer, `""`, is the whole answer.
- A string or number there is the version, or what `versionRegex` searches.
- A list holds versions, and the latest stable one wins. Versions compare like
  [tags](#finding-the-latest-version), and with `versionRegex` only the versions it matches count.
- A `*` segment matches every entry of a list or object. Go's `https://go.dev/dl/?mode=json` is a
  list of releases, so `*/version` ranks the `version` of each.
- `versionJsonPointer` and `versionSelector` can't both be set.

### Locking a dependency

Set `locked: true` to keep a dependency at its current version:

<!-- formats -->
<details open>
<summary>JSON</summary>

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

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  Coloris:
    version: v0.17.1
    repository: https://github.com/mdbassit/Coloris
    files:
      - dist/coloris.min.js
    locked: true
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.Coloris]
version = 'v0.17.1'
repository = 'https://github.com/mdbassit/Coloris'
files = ['dist/coloris.min.js']
locked = true
```

</details>
<!-- /formats -->

`vendor update` skips a locked dependency, and `vendor outdated` leaves it out. `vendor sync` still
downloads its files if they're missing.

### Aliases

`alias` gives a dependency other names. It takes one name or a list:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "kubectl": {
            "url": "https://dl.k8s.io/release/{tag}/bin/windows/amd64/kubectl.exe",
            "versionUrl": "https://dl.k8s.io/release/stable.txt",
            "files": "kubectl.exe",
            "alias": ["k8s", "kc"]
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  kubectl:
    url: https://dl.k8s.io/release/{tag}/bin/windows/amd64/kubectl.exe
    versionUrl: https://dl.k8s.io/release/stable.txt
    files: kubectl.exe
    alias:
      - k8s
      - kc
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.kubectl]
url = 'https://dl.k8s.io/release/{tag}/bin/windows/amd64/kubectl.exe'
versionUrl = 'https://dl.k8s.io/release/stable.txt'
files = 'kubectl.exe'
alias = ['k8s', 'kc']
```

</details>
<!-- /formats -->

- An alias works wherever a command takes a dependency name, so `vendor update k8s` updates
  `kubectl`.
- Each name and alias belongs to one dependency. A config where two share one doesn't load.

### Installers

On Windows, vendor can run an installer instead of keeping the file. Set `installer: true` on a
dependency whose `files` names exactly one download, the setup program:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "winmerge": {
            "repository": "https://github.com/WinMerge/winmerge",
            "files": "{release}/WinMerge-{version}-x64-PerUser-Setup.exe",
            "installer": true
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  winmerge:
    repository: https://github.com/WinMerge/winmerge
    files: '{release}/WinMerge-{version}-x64-PerUser-Setup.exe'
    installer: true
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.winmerge]
repository = 'https://github.com/WinMerge/winmerge'
files = '{release}/WinMerge-{version}-x64-PerUser-Setup.exe'
installer = true
```

</details>
<!-- /formats -->

`vendor sync` downloads the setup to a temporary folder, runs it silently, and deletes it. The
lockfile records the program's Apps & Features entry instead of any files, and `vendor uninstall`
runs its uninstaller silently. If you remove the program from Settings, the next `vendor sync`
installs it again. `vendor update` installs a newer release over the old one.

If the program is already installed at the release's version, vendor records that install and
skips the installer, so adding a program you installed by hand raises no UAC prompt.
`vendor sync --force` runs the installer anyway.

vendor detects the installer's type from the file:

| Type | Installers | Silent switches vendor adds |
| --- | --- | --- |
| `nsis` | Nullsoft (NSIS) | `/S` |
| `inno` | Inno Setup | `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP-` |
| `msi` | Windows Installer | `msiexec /i … /qn /norestart` |
| `msix` | MSIX and AppX packages | none, installed with `Add-AppxPackage` |
| `exe` | anything else | none, so give the switches in `args` |

`installer` also takes a type name, like `installer: inno`, or an object:

| Key | What it holds |
| --- | --- |
| `type` | One of the types above, when detection should not decide |
| `args` | Extra arguments, passed after the silent switches exactly as written |
| `scope` | `user` to install for you alone, `machine` for every user. Unset leaves it to the installer |
| `relaunch` | Whether to close the program if it is running and start it again after. Unset follows `vendorConfig.relaunchPrograms` |

An installer that needs admin rights raises the usual UAC prompt, and vendor waits for it.
Installers run one at a time, in config order, while the other downloads carry on. An installer
or uninstaller that runs for 3 minutes may be stuck on a window the silent switches couldn't
hide, so vendor says so and rings the terminal bell.

An installer can't replace the files of a program that is running, and many report success
anyway. So vendor won't install over a running program, and asks you to close it first. Set
`vendorConfig.relaunchPrograms: true` to have vendor do that for every installer. It asks the
program to quit the way Windows does at shutdown, installs, and starts it again. An installer's
own `relaunch` overrides the setting either way. If the program's version hasn't changed after
the installer exits, the install fails instead of being recorded.

```yml
vendorConfig:
  relaunchPrograms: true
```

Running an installer runs code from the internet, so vendor only runs installers from your own
config, which is the one `-c` names or the one `DEFAULT_VENDOR_CONFIG` points at. A config vendor
finds by searching, like one in a project you cloned, needs `--allow-installers` on `sync`,
`update` or `install`.

Pick programs that do not update themselves. vendor records the version it installed, and a
program that updates itself leaves that record behind.

### Default options

A `default` object sets values for every dependency that doesn't set them itself.
`defaultVendorOptions` is another name for the same object. In this example both dependencies come
from one repository and track commits:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorConfig": {
        "vendorFolder": "."
    },
    "default": {
        "vendorFolder": "{vendorFolder}",
        "repository": "https://github.com/nushell/nu_scripts",
        "hashVersionFile": true
    },
    "vendorDependencies": {
        "nu-winget-completions": {
            "files": "custom-completions/winget/winget-completions.nu",
            "version": "912bea4588ba089aebe956349488e7f78e56061c"
        },
        "nu-cargo-completions": {
            "files": "custom-completions/cargo/cargo-completions.nu",
            "version": "afde2592a6254be7c14ccac520cb608bd1adbaf9"
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorConfig:
  vendorFolder: .
default:
  vendorFolder: '{vendorFolder}'
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

</details>

<details>
<summary>TOML</summary>

```toml
[vendorConfig]
vendorFolder = '.'

[default]
vendorFolder = '{vendorFolder}'
repository = 'https://github.com/nushell/nu_scripts'
hashVersionFile = true

[vendorDependencies.nu-winget-completions]
files = 'custom-completions/winget/winget-completions.nu'
version = '912bea4588ba089aebe956349488e7f78e56061c'

[vendorDependencies.nu-cargo-completions]
files = 'custom-completions/cargo/cargo-completions.nu'
version = 'afde2592a6254be7c14ccac520cb608bd1adbaf9'
```

</details>
<!-- /formats -->

## Commands

Every command has short aliases, like `vendor s` for `vendor sync`. Run `vendor help <command>` to
see a command's options in your terminal.

### Options for every command

These go before or after the command name.

| Option | What it does |
| --- | --- |
| `-c, --config <file/folder path>` | Use this config file, or the config in this folder |
| `-p, --plain` | Print plain lines instead of the live display |
| `-v, --version` | Print the version |
| `-h, --help` | Print help |

`VENDOR_CONFIG` sets the config path too, and `DEFAULT_VENDOR_CONFIG` is the fallback when no config
is found. [Where vendor looks for the config](#where-vendor-looks-for-the-config) has the details.

### Command list

| Command | Aliases | What it does |
| --- | --- | --- |
| `sync` | `s` | Download everything the config lists |
| `update [names...]` | `upgrade`, `bump`, `up`, `u` | Move dependencies to their latest version |
| `outdated` | `o` | List dependencies that have a newer version |
| `install <url/name...>` | `add`, `i`, `a` | Add dependencies |
| `search [terms...]` | `find` | Find programs you can add by name |
| `uninstall <names...>` | `remove`, `delete`, `del`, `rm` | Delete dependencies and their files |
| `list` | `ls` | Print the dependencies in the config |
| `config [edit]` | `cfg` | Print the config file's path, or open it in an editor |
| `login [token]` | `auth` | Log in to GitHub, or check the login |
| `logout` | | Delete the saved GitHub token |
| `completions <shell>` | | Print a shell completion script |
| `help [command]` | | Print help for a command |

---

### `vendor sync`

`vendor sync` downloads every dependency whose files are missing or out of date. `-f`/`--force`
downloads them again even when they're up to date. `vendor` checks the [lockfile](#lockfile) to see
what's out of date.

---

### `vendor update`

`vendor update` installs the latest version of every dependency. Name dependencies to update only
those:

```bash
vendor update
vendor update React Express
```

`--pr` prints a Markdown summary of the changes, ready to use as a pull request description from a
GitHub Action. [Locked](#locking-a-dependency) dependencies are skipped.

---

### `vendor install`

`vendor install`, or its alias `vendor add`, adds dependencies to the config. Each source can be any
of these:

| Source | Looks like |
| --- | --- |
| A [program name from the registry](#program-registry) | `rg` |
| A repository URL | `https://github.com/th-ch/youtube-music` |
| `owner/repo` | `Araxeus/vendorfiles` |
| A repository name to search GitHub for | `React` |

A registry program needs no file list. For any other source, list the files with `-f`. Pin any
source to a version with `source@version`, like `vendor add fd@v10.2.0`.

| Option | What it does |
| --- | --- |
| `-f, --files <files...>` | The files to download. A registry program needs none. |
| `-n, --name [name]` | The name to give the dependency in the config |
| `--dry-run` | Print the entry it would add, and change nothing |
| `--list` | List the programs you can add by name, like `vendor search` |
| `--refresh` | Check the program registry for changes now |

```bash
vendor add rg fd
vendor add Araxeus/vendorfiles@v1.0.0 -f README.md LICENSE
vendor i https://github.com/th-ch/youtube-music -f "{release}/YouTube-Music-{version}.exe"
```

---

### `vendor search`

`vendor search` finds programs you can add by name. [Finding a program](#finding-a-program) shows how
the search works. `--refresh` checks the registry for changes first.

---

### `vendor uninstall`

`vendor uninstall` deletes a dependency's files and removes it from the config and the lockfile.
For an [installer](#installers), it runs the program's uninstaller instead. `--keep` only removes
it from the config and the lockfile, and leaves its files or program in place.

---

### `vendor list`

`vendor list` prints the dependencies in the config. `vendor config list` does the same. When the
config has an [installer](#installers), a `TYPE` column marks which dependencies run one.

---

### `vendor config`

`vendor config` prints the path of the config file it found. `vendor config edit` opens that file in
the editor you name, or in `$EDITOR`, or in your system's default editor:

```bash
vendor cfg edit "code --wait"
```

---

### `vendor login` and `vendor logout`

`vendor login` saves a GitHub token. With no token given, it logs you in through your browser.
`vendor auth status` shows which token is in use, whether GitHub accepts it, and how many requests
are left. It exits with `1` when there is no token or GitHub rejects it.

`vendor logout` deletes the saved token. [Authentication](#authentication) covers all three in detail.

---

### `vendor completions`

`vendor completions <shell>` prints a completion script for `bash`, `elvish`, `fish`, `powershell`
or `zsh`. The bash and zsh scripts also complete program names after `vendor add`, as the registry
stood when you made the script.

Save the script where your shell loads it:

```bash
vendor completions bash > /etc/bash_completion.d/vendor
vendor completions zsh  > "${fpath[1]}/_vendor"
vendor completions fish > ~/.config/fish/completions/vendor.fish
vendor completions powershell >> $PROFILE
```

## Program registry

The program registry is a list of programs `vendor` knows by name. For those, you don't have to
look up the repository or work out what its release assets are called:

```bash
vendor add fd          # or fdfind, or fd-find
vendor add rg          # keys the entry `ripgrep`, its canonical name
vendor add rg fd       # both at once
vendor add fd@v10.2.0  # pin a version
```

### What gets written

`vendor add` writes an ordinary config entry. Nothing in it depends on the registry, so it keeps
working whatever happens to the registry later:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "fd": {
            "version": "v10.4.2",
            "repository": "https://github.com/sharkdp/fd",
            "files": [
                {
                    "{release}/fd-v{version}-x86_64-pc-windows-msvc.zip": {
                        "fd-v{version}-x86_64-pc-windows-msvc/fd.exe": "fd.exe"
                    }
                }
            ]
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  fd:
    version: v10.4.2
    repository: https://github.com/sharkdp/fd
    files:
      - '{release}/fd-v{version}-x86_64-pc-windows-msvc.zip':
          fd-v{version}-x86_64-pc-windows-msvc/fd.exe: fd.exe
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.fd]
version = 'v10.4.2'
repository = 'https://github.com/sharkdp/fd'
files = [
  { '{release}/fd-v{version}-x86_64-pc-windows-msvc.zip' = { 'fd-v{version}-x86_64-pc-windows-msvc/fd.exe' = 'fd.exe' } },
]
```

</details>
<!-- /formats -->

`vendor add` picks the asset for your platform. `{version}` stays as written, so `vendor update`
keeps working afterwards.

### Finding a program

`vendor search` lists every program the registry knows, and `vendor add --list` does the same. Add
words to narrow the list. Each word has to match a program's name, an alias, a word in its
description, or its repository. Name matches come first. A program that runs an
[installer](#installers) is marked `(installer)`.

```bash
vendor search          # everything
vendor search disk     # dua, duf, dust, gdu, wiztree and more
vendor search git      # git, gh, git-cliff, git-lfs, lazygit and more
```

If you mistype a name the registry nearly knows, `vendor` suggests the right one before it searches
GitHub. `vendor add ripgreb` asks whether you meant `ripgrep`.

Every program the registry knows is listed under
[Programs you can add by name](#programs-you-can-add-by-name).

### Adding to the registry

Anyone can add a program with a pull request. The
[registry guide](./docs/add_to_registry.md) walks through writing and testing an entry. After
changing `registry.yml`, run `cargo xtask readme` to regenerate the program list. CI fails while the
list is out of date.

### Programs you can add by name

<!-- registry -->
<details>
<summary>Programs you can add by name (199)</summary>

| Program | Aliases | Description | Platforms |
| --- | --- | --- | --- |
| [7z](https://github.com/ip7z/7zip) | `7zz`, `7za` | Command-line archiver for 7z, zip, tar, xz and rar files | All |
| [7zip](https://github.com/ip7z/7zip) | `7-zip` | File archiver with a file manager and Explorer menu for 7z and zip | Windows (installer) |
| [act](https://github.com/nektos/act) |  | Run GitHub Actions workflows locally in Docker to test CI before pushing | All |
| [actionlint](https://github.com/rhysd/actionlint) |  | Linter for GitHub Actions workflow YAML/YML files and CI pipelines | All |
| [age](https://github.com/FiloSottile/age) |  | Simple, modern file encryption with small keys and no config | All |
| [angry-ip-scanner](https://github.com/angryip/ipscan) | `ipscan` | Network scanner that finds hosts and open ports on a LAN | Windows x64 (installer) |
| [ansi2txt](https://github.com/gabe565/ansi2txt) | `ansi2text` | Strips ANSI color and escape codes from text, leaving plain text | All |
| [asciinema](https://github.com/asciinema/asciinema) |  | Record and share terminal sessions as lightweight text casts | macOS, Linux |
| [atuin](https://github.com/atuinsh/atuin) |  | Searchable shell and command history, synced across machines | Windows x64, macOS, Linux |
| [audio-bartender](https://github.com/Araxeus/Audio-Bartender) |  | Tray volume control with per-app volume, level meters and media keys | Windows x64 (installer) |
| [autohotkey](https://github.com/AutoHotkey/AutoHotkey) | `ahk` | Scripting language for Windows hotkeys, macros and automation | Windows x64 (installer) |
| [bandwhich](https://github.com/imsnif/bandwhich) |  | Terminal bandwidth monitor showing network use by process and connection | Windows x64, macOS, Linux |
| [bat](https://github.com/sharkdp/bat) |  | cat clone and file viewer with syntax highlighting and a pager | All |
| [betterleaks](https://github.com/betterleaks/betterleaks) |  | Secrets scanner for git repos, files and CI, from the gitleaks author | All |
| [biome](https://github.com/biomejs/biome) |  | JavaScript, TypeScript, JSON, CSS and YAML/YML formatter and linter | All |
| [bitwarden-secrets-cli](https://github.com/bitwarden/sdk) | `bws` | Bitwarden Secrets Manager CLI for fetching secrets in scripts and CI | All |
| [bleachbit](https://github.com/bleachbit/bleachbit) |  | Disk cleaner that frees space and removes caches, cookies and history | Windows x64 (installer) |
| [bombardier](https://github.com/codesenberg/bombardier) |  | Fast HTTP benchmarking and load testing tool | All |
| [bottom](https://github.com/ClementTsang/bottom) | `btm` | System monitor for processes, CPU, memory, disks and network, like htop | All |
| [btop](https://github.com/aristocratos/btop) |  | Resource monitor for CPU, memory, disks, network and processes like htop | Linux |
| [buf](https://github.com/bufbuild/buf) |  | Protobuf toolkit to lint, format, check breaking changes and generate | All |
| [carapace](https://github.com/carapace-sh/carapace-bin) | `carapace-bin` | Shell completion for hundreds of commands in bash, zsh, fish and more | All |
| [cmake](https://github.com/Kitware/CMake) |  | C and C++ build system generator for make, ninja, MSBuild and Xcode | All |
| [copyq](https://github.com/hluk/CopyQ) |  | Clipboard manager with searchable, editable history and scripting | Windows x64 (installer) |
| [coreutils](https://github.com/uutils/coreutils) | `uutils` | Rust rewrite of GNU coreutils: ls, cp, mv, cat, sort and more | Windows, Linux |
| [cosign](https://github.com/sigstore/cosign) |  | Sign and verify container images and blobs with Sigstore | Windows x64, macOS, Linux |
| [croc](https://github.com/schollz/croc) |  | Send files and folders between computers securely with a code phrase | All |
| [csvlens](https://github.com/YS-L/csvlens) |  | Command-line CSV file viewer, like less but for tables | All |
| [curl](https://curl.se/) |  | Transfer data over HTTP, HTTPS, FTP and more; official Windows build | Windows |
| [dekit](https://github.com/pvolok/dekit) | `mprocs` | Run and watch several processes at once in a TUI, for dev servers | Windows x64, macOS, Linux |
| [delta](https://github.com/dandavison/delta) | `git-delta` | Syntax-highlighting pager and diff viewer for git, diff, grep and blame | Windows x64, macOS arm64, Linux |
| [difftastic](https://github.com/Wilfred/difftastic) | `difft` | Structural diff that compares files by syntax tree, not line by line | All |
| [diffutils](https://github.com/uutils/diffutils) |  | Cross-platform Rust rewrite of GNU diffutils: diff, cmp and more | Windows x64, macOS, Linux |
| [direnv](https://github.com/direnv/direnv) |  | Load and unload environment variables per directory from .envrc files | All |
| [ditto](https://github.com/sabrogden/Ditto) |  | Clipboard manager that keeps history of copied text and images | Windows x64 (installer) |
| [dive](https://github.com/wagoodman/dive) |  | Explore Docker and OCI image layers and find wasted space | All |
| [docker-compose](https://github.com/docker/compose) | `compose` | Define and run multi-container Docker apps from a compose.yaml file | All |
| [doggo](https://github.com/mr-karan/doggo) |  | Command-line DNS client, like dig, with DoH, DoT and JSON output | Windows x64, macOS, Linux |
| [dotnet](https://builds.dotnet.microsoft.com/) | `dotnet-sdk` | .NET SDK for C#, F# and ASP.NET apps: dotnet build, run, test, publish | All |
| [dua](https://github.com/Byron/dua-cli) | `dua-cli` | Disk usage analyzer with a TUI to find and delete big folders | All |
| [duckdb](https://github.com/duckdb/duckdb) |  | In-process SQL database for analytics on CSV, Parquet and JSON files | All |
| [duf](https://github.com/muesli/duf) |  | Disk usage and free space of mounted drives in tables, a better df | All |
| [dufs](https://github.com/sigoden/dufs) |  | File server for static serving, uploads, search and WebDAV over HTTP | All |
| [dust](https://github.com/bootandy/dust) |  | Disk usage viewer showing what takes up space, a more intuitive du | All |
| [dysk](https://github.com/Canop/dysk) |  | Lists mounted disks and filesystems with free space, a friendlier df | Windows x64, macOS, Linux |
| [edamame](https://github.com/mijowi/edamame) |  | Terminal text editor for Markdown that stays rendered while you edit | Windows x64, macOS, Linux |
| [edit](https://github.com/microsoft/edit) | `msedit` | Microsoft's small terminal text editor, a modern MS-DOS Edit | Windows, macOS arm64, Linux |
| [everything](https://www.voidtools.com/) |  | Instant file search for Windows that finds files by name as you type | Windows x64 (installer) |
| [fastfetch](https://github.com/fastfetch-cli/fastfetch) |  | Neofetch-like tool that shows system information in the terminal | All |
| [fd](https://github.com/sharkdp/fd) | `fdfind`, `fd-find` | Fast, user-friendly alternative to find for locating files and folders | Windows, macOS arm64, Linux |
| [ffmpeg](https://github.com/GyanD/codexffmpeg) |  | Convert, transcode and stream video and audio media, plus ffprobe | Windows x64 |
| [file-pilot](https://filepilot.tech/) | `fpilot`, `filepilot` | Fast, lightweight file manager and Explorer replacement for Windows | Windows x64 |
| [flow](https://github.com/neurocyte/flow) |  | Flow Control, a programmer's text editor for the terminal | All |
| [flux](https://github.com/fluxcd/flux2) | `flux2`, `fluxcd` | GitOps for Kubernetes that syncs clusters from git, Helm and Kustomize | All |
| [fnm](https://github.com/Schniz/fnm) |  | Fast Node.js version manager that switches node per project, like nvm | Windows x64, macOS, Linux |
| [freecad](https://github.com/FreeCAD/FreeCAD) |  | Parametric 3D CAD modeler for product design and 3D printing | Windows x64 (installer) |
| [freeze](https://github.com/charmbracelet/freeze) |  | Generate images of code and terminal output as PNG, SVG or WebP | Windows x64, macOS, Linux |
| [fresh](https://github.com/sinelaw/fresh) |  | Fast, easy terminal text editor and IDE | All |
| [fzf](https://github.com/junegunn/fzf) |  | Interactive command-line fuzzy finder for files, history and any list | All |
| [gdu](https://github.com/dundee/gdu) |  | Fast disk usage analyzer with a terminal UI | All |
| [gh](https://github.com/cli/cli) | `gh-cli`, `github-cli` | GitHub's official CLI for pull requests, issues, releases and repos | All |
| [git](https://github.com/git-for-windows/git) | `git-for-windows` | Git for Windows: git, Git Bash and Git GUI, with credential manager | Windows (installer) |
| [git-cliff](https://github.com/orhun/git-cliff) |  | Changelog generator from git history and conventional commits | All |
| [git-lfs](https://github.com/git-lfs/git-lfs) |  | Git extension that stores large files like media outside the repo | All |
| [gitleaks](https://github.com/gitleaks/gitleaks) |  | Find secrets like passwords, API keys and tokens in git repos and files | All |
| [glow](https://github.com/charmbracelet/glow) |  | Render Markdown in the terminal, with a TUI to browse files | Windows x64, macOS, Linux |
| [go](https://dl.google.com/go) | `golang` | Go programming language toolchain: compiler, go build, go test, gofmt | All |
| [golangci-lint](https://github.com/golangci/golangci-lint) |  | Fast Go linters runner that runs many linters in parallel, with caching | All |
| [gping](https://github.com/orf/gping) |  | Ping with a live graph in the terminal, for several hosts at once | Windows x64, macOS, Linux |
| [grpcurl](https://github.com/fullstorydev/grpcurl) |  | Like curl, but for gRPC: call and inspect gRPC servers | All |
| [grype](https://github.com/anchore/grype) |  | Vulnerability scanner for container images, filesystems and SBOMs | Windows x64, macOS, Linux |
| [gum](https://github.com/charmbracelet/gum) |  | Prompts, menus, spinners and styled text for glamorous shell scripts | Windows x64, macOS, Linux |
| [hadolint](https://github.com/hadolint/hadolint) |  | Dockerfile linter that checks best practices and shell with ShellCheck | Windows x64, macOS, Linux |
| [handbrake](https://github.com/HandBrake/HandBrake) |  | Video transcoder that converts video to MP4 and MKV with H.265 and AV1 | Windows (installer) |
| [helix](https://github.com/helix-editor/helix) | `hx` | Modal terminal text editor, like Vim or Kakoune, with LSP built in | Windows x64, macOS, Linux |
| [helm](https://get.helm.sh) |  | Package manager for Kubernetes that installs and upgrades charts | All |
| [herdr](https://github.com/herdrdev/herdr) |  | Terminal runtime for running and managing AI coding agents | Windows x64, macOS, Linux |
| [hexyl](https://github.com/sharkdp/hexyl) |  | Hex viewer for the terminal that colors bytes by kind | Windows x64, macOS, Linux |
| [hide-console](https://github.com/Araxeus/hide-console) | `hide` | Run a command on Windows with its console window hidden | Windows x64 |
| [hugo](https://github.com/gohugoio/hugo) |  | Fast static site generator for blogs and docs sites from Markdown | Windows x64, Linux |
| [hurl](https://github.com/Orange-OpenSource/hurl) |  | Run and test HTTP requests from plain text files, for APIs and CI | Windows x64, macOS, Linux |
| [hyperfine](https://github.com/sharkdp/hyperfine) |  | Command-line benchmarking tool that times and compares commands | All |
| [imageglass](https://github.com/d2phap/ImageGlass) |  | Fast image viewer for 90+ formats, including WEBP, AVIF, JXL and HEIC | Windows (installer) |
| [imagemagick](https://github.com/ImageMagick/ImageMagick) | `magick` | Create, edit, convert and resize images in 200+ formats with magick | Windows (installer) |
| [jj](https://github.com/jj-vcs/jj) | `jujutsu` | Jujutsu, a Git-compatible version control system with undo and no index | All |
| [jq](https://github.com/jqlang/jq) |  | Command-line JSON processor for querying, filtering and transforming | All |
| [just](https://github.com/casey/just) |  | Command runner for project tasks and scripts, like make without builds | All |
| [k3d](https://github.com/k3d-io/k3d) |  | Run k3s Kubernetes clusters in Docker for local development | Windows x64, macOS, Linux |
| [k6](https://github.com/grafana/k6) |  | Load testing tool scripted in JavaScript for APIs, sites and performance | Windows x64, macOS, Linux |
| [k9s](https://github.com/derailed/k9s) |  | Terminal UI for Kubernetes clusters, pods, logs and resources | All |
| [kind](https://github.com/kubernetes-sigs/kind) |  | Kubernetes in Docker: local clusters for development and CI testing | Windows x64, macOS, Linux |
| [kopia](https://github.com/kopia/kopia) |  | Fast, encrypted, deduplicating backups with snapshots to cloud or disk | Windows x64, macOS, Linux |
| [krita](https://cdn.download.kde.org/) |  | Digital painting app for illustration, concept art, comics and animation | Windows x64 (installer) |
| [kubectl](https://dl.k8s.io/) | `k8s` | Kubernetes CLI for deploying apps and managing clusters, pods and logs | All |
| [kubectx](https://github.com/ahmetb/kubectx) | `kubens` | Switch Kubernetes contexts and namespaces fast, with kubens included | All |
| [lazydocker](https://github.com/jesseduffield/lazydocker) |  | Terminal UI for Docker and Compose containers, images, logs and stats | All |
| [lazygit](https://github.com/jesseduffield/lazygit) |  | Terminal UI for git that handles staging, commits, branches and rebases | All |
| [lazysql](https://github.com/jorgerojas26/lazysql) |  | Terminal UI SQL client for MySQL, PostgreSQL, SQLite and MSSQL | All |
| [lefthook](https://github.com/evilmartians/lefthook) |  | Fast git hooks manager for pre-commit and pre-push checks | All |
| [lf](https://github.com/gokcehan/lf) |  | Terminal file manager inspired by ranger, fast and in a single binary | Windows x64, macOS, Linux |
| [libclang](https://www.nuget.org/api/v2/package) |  | Clang's C API as a shared library, for bindings and code analysis tools | Windows, macOS arm64, Linux |
| [librecad](https://github.com/LibreCAD/LibreCAD) |  | 2D CAD drafting app for technical drawings, with DXF and DWG support | Windows x64 (installer) |
| [litestream](https://github.com/benbjohnson/litestream) |  | Streams SQLite database changes to S3 and other storage for backup | All |
| [llvm](https://github.com/llvm/llvm-project) | `clang`, `clangd`, `clang-format` | LLVM and Clang C and C++ toolchain: clang, clangd, lld and clang-format | Windows, Linux |
| [lnav](https://github.com/tstack/lnav) |  | Log file viewer that merges, filters and queries logs with SQL | Windows, macOS arm64, Linux |
| [localsend](https://github.com/localsend/localsend) |  | Share files to nearby devices over the local network, like AirDrop | Windows x64 (installer) |
| [ls-interactive](https://github.com/Araxeus/ls-interactive) | `lsi` | Interactive ls for browsing files and navigating directories | All |
| [mdbook](https://github.com/rust-lang/mdBook) |  | Create books and documentation sites from Markdown files | Windows x64, macOS, Linux |
| [micro](https://github.com/zyedidia/micro) |  | Terminal text editor with familiar keys and mouse support, a modern nano | All |
| [miniserve](https://github.com/svenstaro/miniserve) |  | Serve files and folders over HTTP from the command line, with uploads | Windows x64, macOS, Linux |
| [mkcert](https://github.com/FiloSottile/mkcert) |  | Make locally trusted HTTPS certificates for development, no config | All |
| [mpc-be](https://github.com/Aleksoid1978/MPC-BE) |  | Media player for video and audio, based on Media Player Classic | Windows x64 |
| [mpc-hc](https://github.com/clsid2/mpc-hc) |  | Lightweight media player for video and audio, Media Player Classic | Windows x64 (installer) |
| [mpv-net](https://github.com/mpvnet-player/mpv.net) | `mpv.net` | Media player built on mpv, with a context menu and settings GUI | Windows x64 (installer) |
| [neovim](https://github.com/neovim/neovim) | `nvim` | Vim-based text editor with Lua plugins, LSP and tree-sitter | All |
| [ninja](https://github.com/ninja-build/ninja) |  | Small, fast build system focused on speed, used by CMake and Meson | All |
| [node](https://nodejs.org/) | `nodejs`, `npm` | Node.js JavaScript runtime with npm and npx, the LTS release | All |
| [nushell](https://github.com/nushell/nushell) | `nu` | Cross-platform shell that treats command output as structured data | All |
| [oh-my-posh](https://github.com/JanDeDobbeleer/oh-my-posh) | `posh` | Prompt theme engine for any shell: PowerShell, bash, zsh, fish, cmd | All |
| [open-shell](https://github.com/Open-Shell/Open-Shell-Menu) | `classic-shell` | Classic Start menu and Explorer tweaks for Windows 10 and 11 | Windows x64 (installer) |
| [openshot](https://github.com/OpenShot/openshot-qt) |  | Video editor with a timeline, titles, transitions and animation | Windows x64 (installer) |
| [openssh](https://github.com/PowerShell/Win32-OpenSSH) | `ssh`, `win32-openssh` | OpenSSH for Windows: ssh, scp, sftp, ssh-keygen, ssh-agent and sshd | Windows |
| [opentofu](https://github.com/opentofu/opentofu) | `tofu` | Open-source Terraform fork for infrastructure as code in HCL | All |
| [ouch](https://github.com/ouch-org/ouch) |  | Compress and extract zip, tar, gz, xz, zst, 7z and rar with one command | All |
| [ov](https://github.com/noborus/ov) |  | Feature-rich terminal pager and text viewer, a modern take on less | All |
| [pandoc](https://github.com/jgm/pandoc) |  | Document converter for Markdown, HTML, Word docx, PDF, EPUB and LaTeX | Windows x64, macOS, Linux |
| [peazip](https://github.com/peazip/PeaZip) |  | Archiver for 200+ formats, with encryption, split archives and a GUI | Windows x64 (installer) |
| [pinact](https://github.com/suzuki-shunsuke/pinact) |  | Pin GitHub Actions and workflows to commit SHAs and update versions | All |
| [powershell](https://github.com/PowerShell/PowerShell) |  | PowerShell 7, the cross-platform shell and scripting language | Windows (installer) |
| [presenterm](https://github.com/mfontanini/presenterm) |  | Terminal slideshow tool for Markdown presentations, with images and code | Windows x64, macOS, Linux |
| [protoc](https://github.com/protocolbuffers/protobuf) | `protobuf` | Protocol Buffers compiler that generates code from .proto files | Windows x64, macOS, Linux |
| [psmux](https://github.com/psmux/psmux) | `pmux` | Native tmux for Windows, a terminal multiplexer for PowerShell and cmd | Windows |
| [pulumi](https://github.com/pulumi/pulumi) |  | Infrastructure as code in TypeScript, Python, Go or C# for any cloud | All |
| [putty](https://the.earth.li/) |  | SSH and telnet client with PuTTYgen, Pageant, psftp and plink | Windows (installer) |
| [pwsh](https://github.com/PowerShell/PowerShell) |  | PowerShell 7, the cross-platform shell and scripting language, portable | All |
| [python](https://github.com/astral-sh/python-build-standalone/releases/latest/download) | `python3`, `cpython` | Python interpreter with pip, the latest stable, as a portable build | All |
| [qbittorrent](https://github.com/qbittorrent/qBittorrent) |  | BitTorrent client with a built-in search engine and no ads | Windows x64 (installer) |
| [qsv](https://github.com/dathere/qsv) |  | Fast CSV toolkit to slice, join, sort, query, validate and convert data | Windows, macOS arm64, Linux |
| [rainmeter](https://github.com/rainmeter/rainmeter) |  | Desktop customization with skins for widgets, clocks and system meters | Windows x64 (installer) |
| [rclone](https://github.com/rclone/rclone) |  | rsync-style sync to cloud storage like S3, Google Drive and Dropbox | All |
| [restic](https://github.com/restic/restic) |  | Fast, encrypted, deduplicating backups to local disk, S3, SFTP and more | Windows x64 |
| [resvg](https://github.com/linebender/resvg) |  | Fast SVG renderer that converts SVG files to PNG images | All |
| [ripgrep](https://github.com/BurntSushi/ripgrep) | `rg` | Fast recursive regex search of file contents, like grep | All |
| [ripgrep-all](https://github.com/phiresky/ripgrep-all) | `rga` | ripgrep that also searches PDFs, ebooks, Office docs, zip and sqlite | macOS, Linux |
| [ruff](https://github.com/astral-sh/ruff) |  | Fast Python linter and code formatter | All |
| [rufus](https://github.com/pbatard/rufus) |  | Create bootable USB drives from ISO images for Windows and Linux | Windows |
| [rustscan](https://github.com/bee-san/RustScan) |  | Fast port scanner that finds open ports, then hands them to nmap | Windows x64, Linux arm64 |
| [sccache](https://github.com/mozilla/sccache) |  | Compiler cache for Rust, C and C++ builds, like ccache, local or cloud | All |
| [sd](https://github.com/chmln/sd) |  | Find and replace text with regex, an intuitive sed alternative | Windows x64, macOS, Linux |
| [shellcheck](https://github.com/koalaman/shellcheck) |  | Linter for bash and sh shell scripts that finds bugs and pitfalls | Windows x64, macOS, Linux |
| [shfmt](https://github.com/mvdan/sh) |  | Shell script formatter for bash, POSIX sh and mksh | Windows x64, macOS, Linux |
| [shotcut](https://github.com/mltframework/shotcut) |  | Free video editor with a timeline, filters and wide format support | Windows (installer) |
| [skaffold](https://github.com/GoogleContainerTools/skaffold) |  | Continuous development for Kubernetes: build, push and deploy on change | Windows x64, macOS, Linux |
| [sops](https://github.com/getsops/sops) |  | Encrypt secrets in YAML, JSON and ENV files with age, PGP or cloud KMS | All |
| [spotdl](https://github.com/spotDL/spotify-downloader) |  | Download Spotify playlists and songs with album art, via YouTube | Windows x64, macOS arm64, Linux x64 |
| [spotify-player](https://github.com/aome510/spotify-player) | `spotify_player` | Spotify client for the terminal with search, playlists and lyrics | Windows x64, macOS, Linux |
| [sshm](https://github.com/Gu1llaum-3/sshm) |  | Terminal UI for managing and connecting to SSH hosts | Windows x64, macOS, Linux |
| [starship](https://github.com/starship/starship) |  | Fast, customizable prompt for any shell: bash, zsh, fish, PowerShell | All |
| [stylua](https://github.com/JohnnyMorganz/StyLua) |  | Opinionated Lua and Luau code formatter, like prettier for Lua | Windows x64, macOS, Linux |
| [superfile](https://github.com/yorukot/superfile) | `spf` | Modern terminal file manager with panels, previews and a fancy UI | All |
| [surge](https://github.com/surge-downloader/surge) |  | Fast file download manager with a terminal UI | All |
| [syft](https://github.com/anchore/syft) |  | Generate an SBOM from container images and filesystems | All |
| [system-informer](https://github.com/winsiderss/systeminformer) | `systeminformer`, `process-hacker` | Process monitor and task manager replacement, formerly Process Hacker | Windows (installer) |
| [taplo](https://github.com/tamasfe/taplo) |  | TOML toolkit: formatter, linter and language server for TOML files | All |
| [task](https://github.com/go-task/task) | `go-task` | Task runner and build tool driven by a Taskfile.yml, a simpler make | All |
| [temurin](https://api.adoptium.net/) | `java`, `jdk`, `openjdk` | Eclipse Temurin OpenJDK, the latest LTS: java, javac and the JDK tools | Windows x64, macOS, Linux |
| [termscp](https://github.com/veeso/termscp) |  | Terminal file transfer and explorer for SCP, SFTP, FTP, S3 and WebDAV | All |
| [terraform](https://releases.hashicorp.com/) | `tf` | Infrastructure as code that plans and applies cloud resources from HCL | All |
| [terragrunt](https://github.com/gruntwork-io/terragrunt) |  | Terraform and OpenTofu wrapper that keeps configs DRY across modules | Windows x64, macOS, Linux |
| [tflint](https://github.com/terraform-linters/tflint) |  | Terraform linter that finds errors and enforces best practices in HCL | Windows x64, macOS, Linux |
| [tmux](https://github.com/tmux/tmux-builds) |  | Terminal multiplexer with split panes and sessions that outlive detach | macOS, Linux |
| [trippy](https://github.com/fujiapple852/trippy) | `trip` | Network diagnostic tool combining traceroute and ping in a TUI | All |
| [trivy](https://github.com/aquasecurity/trivy) |  | Scanner for vulnerabilities, secrets and misconfig in images and code | Windows x64, macOS, Linux |
| [ttt](https://github.com/eugenioenko/ttt) |  | Terminal text editor that feels like a GUI one, such as VS Code | Windows x64, macOS, Linux |
| [typos](https://github.com/crate-ci/typos) |  | Source code spell checker that finds and fixes typos, fast enough for CI | All |
| [typst](https://github.com/typst/typst) |  | Markup typesetting for papers and documents, a LaTeX alternative | All |
| [upx](https://github.com/upx/upx) |  | Executable packer that compresses binaries to shrink their size | Windows x64, Linux |
| [vault](https://releases.hashicorp.com/) |  | HashiCorp secrets manager CLI and server for keys, tokens and passwords | Windows x64, macOS, Linux |
| [vegeta](https://github.com/tsenart/vegeta) |  | HTTP load testing tool and library at a constant request rate | All |
| [ventoy](https://github.com/ventoy/Ventoy) |  | Make a bootable USB drive once, then copy ISO files onto it to boot them | Windows x64, Linux |
| [vhs](https://github.com/charmbracelet/vhs) |  | Record terminal sessions to GIF and video from a script, for demos | Windows x64, macOS, Linux |
| [viddy](https://github.com/sachaos/viddy) |  | Modern watch command that reruns a command and shows diffs and history | Windows x64, macOS, Linux |
| [watchexec](https://github.com/watchexec/watchexec) |  | Runs a command when files change, for rebuilds, tests and reloads | All |
| [windows-terminal](https://github.com/microsoft/terminal) | `wt` | Microsoft's terminal app with tabs, panes and profiles for every shell | Windows (installer) |
| [winkey-startmenu-disabler](https://github.com/Araxeus/winkey-startmenu-disabler) |  | Stop the Windows key opening Start while keeping Win key shortcuts | Windows x64 (installer) |
| [winmerge](https://github.com/WinMerge/winmerge) |  | Visual diff and merge tool that compares files and folders | Windows x64 (installer) |
| [winrar](https://www.rarlab.com/) |  | Archiver that creates RAR and extracts zip, 7z, tar and more | Windows x64 (installer) |
| [winscp](https://sourceforge.net/) |  | SFTP, SCP, FTP, WebDAV and S3 client with a two-pane file manager | Windows x64 (installer) |
| [wiztree](https://diskanalyzer.com/) |  | Disk space analyzer that finds what fills a drive, fast | Windows x64 (installer) |
| [wsl-notify-send](https://github.com/stuartleeks/wsl-notify-send) |  | Sends Windows desktop notifications from inside WSL, like notify-send | Windows x64 |
| [xh](https://github.com/ducaale/xh) |  | Friendly, fast tool for sending HTTP requests, like HTTPie or curl | Windows x64, macOS, Linux |
| [yamlfmt](https://github.com/google/yamlfmt) |  | YAML file formatter | All |
| [yazi](https://github.com/sxyazi/yazi) | `ya` | Fast terminal file manager with image previews, tabs and plugins | All |
| [yq](https://github.com/mikefarah/yq) |  | Command-line YAML, JSON, XML, CSV and TOML processor, like jq | All |
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) |  | Download video and audio from YouTube and many other sites | All |
| [zellij](https://github.com/zellij-org/zellij) |  | Terminal multiplexer with panes, tabs, layouts and plugins, like tmux | Windows x64, macOS, Linux |
| [zig](https://ziglang.org/) |  | Zig compiler and toolchain, also a drop-in C and C++ cross compiler | All |
| [zls](https://github.com/zigtools/zls) |  | Zig language server for editor completion, diagnostics and navigation | All |
| [zoxide](https://github.com/ajeetdsouza/zoxide) |  | Smarter cd that jumps to the directories you use most | All |

</details>
<!-- /registry -->

## Keeping vendorfiles updated

`vendor` can keep itself up to date. Add it like any other program:

```bash
vendor add vendorfiles
```

That writes an entry for your platform's release asset, set to install next to the running `vendor`
binary:

<!-- formats -->
<details open>
<summary>JSON</summary>

```json
{
    "vendorDependencies": {
        "vendorfiles-rs": {
            "version": "v2.4.0",
            "repository": "https://github.com/Araxeus/vendorfiles-rs",
            "files": [
                {
                    "{release}/vendor_v{version}_windows.zip": ["vendor.exe"]
                }
            ],
            "vendorFolder": "C:/tools/bin"
        }
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
vendorDependencies:
  vendorfiles-rs:
    version: v2.4.0
    repository: https://github.com/Araxeus/vendorfiles-rs
    files:
      - '{release}/vendor_v{version}_windows.zip':
          - vendor.exe
    vendorFolder: C:/tools/bin
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.vendorfiles-rs]
version = 'v2.4.0'
repository = 'https://github.com/Araxeus/vendorfiles-rs'
files = [{ '{release}/vendor_v{version}_windows.zip' = ['vendor.exe'] }]
vendorFolder = 'C:/tools/bin'
```

</details>
<!-- /formats -->

From then on, `vendor update` upgrades `vendor` along with everything else, and safely replaces the
running binary. `vendor uninstall vendorfiles-rs` stops tracking it but leaves the binary in place.

## Authentication

Without a token, GitHub allows 60 API requests an hour, so `vendor` warns when it runs without one.
Log in to raise the limit.

### Where the token comes from

`vendor` uses the first token it finds, in this order:

1. The `GITHUB_TOKEN` environment variable
2. The token `vendor login` saved in your system's credential store
3. No token, with the anonymous limit

`login`, `logout` and `auth status` need no config file, so they work from any folder.

### Logging in

`vendor login` saves a token. Paste one with `vendor login <token>`, or run `vendor login` alone to
log in through GitHub's device flow in your browser:

```text
$ vendor login
One-time code (ABCD-1234) copied to clipboard
Press Enter to open https://github.com/login/device in your browser...
SUCCESS: Logged in successfully
```

`vendor logout` deletes the saved token. If something would still log you in, it tells you. That
can be a `GITHUB_TOKEN` in the environment, or a token the npm CLI saved in its own keyring entry.
`vendor` leaves that entry alone, because it belongs to npm.

### Checking your login

`vendor auth status` shows which token a run would use, and asks GitHub whether it still works. It
prints the account name, the requests left this hour, and the token's expiry date if it has one:

```text
$ vendor auth status
SUCCESS: Logged in to GitHub as octocat with the token from the credential store
INFO: 4990 of 5000 API requests left this hour (resets in 42 min)
```

It exits with `1` when there is no token or GitHub rejects it, so CI can run it as a check before
anything else.

### Where the token is stored

`vendor` has each platform's credential store built in, so there is no system library to install:

| Platform                 | Store                                                | Keeps the token  |
| ------------------------ | ---------------------------------------------------- | ---------------- |
| Windows                  | Credential Manager (`windows-native-keyring-store`)  | until deleted    |
| macOS                    | login Keychain (`apple-native-keyring-store`)        | until deleted    |
| Linux                    | Secret Service (`zbus-secret-service-keyring-store`) | until deleted    |
| Linux, no keyring daemon | kernel keyutils (`linux-keyutils-keyring-store`)     | **until reboot** |
| anything else            | none, so use `GITHUB_TOKEN`                          | -                |

On Linux, `vendor` tries the Secret Service first, so the token lasts wherever a keyring daemon runs,
like gnome-keyring, KWallet or KeePassXC. Headless machines, minimal containers and WSL usually have
no daemon. There `vendor` falls back to the kernel keyutils store. It always works, but it keeps
secrets in kernel memory, so they're gone after a reboot.

When `vendor login` lands on that fallback, it warns you, so the next boot doesn't quietly log you
out:

```text
WARNING: this system's credential store keeps secrets in kernel memory, so the token will be gone after a reboot
```

You can keep the token across reboots in two ways.

- Run a Secret Service daemon. Installing `gnome-keyring` is usually enough. On a headless machine
  it needs unlocking, as [this note](https://docs.rs/zbus-secret-service-keyring-store) explains.
- Set `GITHUB_TOKEN` from your shell profile or your CI secrets.

## Lockfile

`vendor` records what it downloaded in a `vendor-lock.json` file in each dependency's folder:

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

Under `files`, each key is a path from your config, with `{version}` left as written. Each value is
the file that landed on disk. A [`url` dependency](#downloading-from-other-sites) records its `url`
where a GitHub one records `repository`.

A [folder or pattern](#folders-and-globs) records each file it matched, by its path in the
repository. Inside an archive, each declared member maps to the members it matched:

```json
"files": {
  "skills/canvas-design/canvas-fonts/": {
    "skills/canvas-design/canvas-fonts/Lora-Regular.ttf": "fonts/Lora-Regular.ttf"
  },
  "skills/**/SKILL.md": { "skills/pdf/SKILL.md": "pdf.md" }
}
```

```json
"files": {
  "{release}/ripgrep-{version}-x86_64-pc-windows-msvc.zip": {
    "ripgrep-{version}-x86_64-pc-windows-msvc/doc/": {
      "ripgrep-14.1.1-x86_64-pc-windows-msvc/doc/FAQ.md": "doc/FAQ.md"
    }
  }
}
```

A binary older than this format can't read a lockfile that holds an archive entry like this one.

`blobs` records the git blob of each repository file a folder or pattern selected, which is what
lets an update skip the files that did not change.

`vendor sync` downloads a dependency again when any of these is true:

- Its version changed.
- A recorded file is missing.
- Its `files` list changed.

### One lockfile for everything

Set `vendorConfig.singleLockfile` to keep every dependency in one `vendor-lock.json` instead.

- `true` puts it next to the config file.
- A string names the folder for it, relative to the config file or absolute. `{vendorFolder}` in
  the string expands to the base vendor folder.

## JSON Schema

[`vendorfiles.schema.json`](./vendorfiles.schema.json) describes the config format. Point your
config at it, and your editor can check and autocomplete it as you type:

<!-- formats -->
<details open>
<summary>JSON</summary>

```jsonc
{
    "$schema": "https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/refs/heads/main/vendorfiles.schema.json",
    "vendorDependencies": {
        //...
    }
}
```

</details>

<details>
<summary>YAML</summary>

```yml
# yaml-language-server: $schema=https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/refs/heads/main/vendorfiles.schema.json
vendorDependencies:
  #...
```

</details>

<details>
<summary>TOML</summary>

```toml
#:schema https://raw.githubusercontent.com/Araxeus/vendorfiles-rs/refs/heads/main/vendorfiles.schema.json

[vendorDependencies]
#...
```

</details>
<!-- /formats -->

## Differences from the TypeScript version

Configs and lockfiles work with both versions, except
[`url` dependencies](#downloading-from-other-sites), which only this version reads. This version looks up versions and downloads files in
parallel, which is where most of the speedup comes from. These timings compare the two on a config
with 8 dependencies, against `vendorfiles@1.4.2`:

|                                 | TypeScript | Rust   |
| ------------------------------- | ---------- | ------ |
| `sync` (nothing downloaded yet) | 3127 ms    | 893 ms |
| `sync` (everything up to date)  | 721 ms     | 66 ms  |
| `outdated`                      | 3426 ms    | 908 ms |
| `--version`                     | 706 ms     | 54 ms  |

## Development

### Checks

`cargo xtask ci` runs every check CI runs. That's `cargo check`, rustfmt, clippy and the tests, in
that order, and it stops at the first failure.

```bash
cargo xtask ci
```

You can also run them one at a time:

```bash
cargo check --workspace --all-targets
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

The lint groups live in `[workspace.lints]`, so a bare `cargo clippy` applies the same lints as CI.
The flags above are what CI spells out, and you don't need them locally.

### Workspace layout

| Crate                     | What it holds |
| ------------------------- | ------------- |
| `crates/vendorfiles_core` | The library: config, lockfile, GitHub client, archive handling and operations. It returns typed errors through `thiserror` and never exits the process. |
| `crates/vendorfiles`      | The `vendor` binary, with Commander-compatible help and errors, and `anyhow` at the boundary. |
| `xtask`                   | Repository automation, run as `cargo xtask <command>` |

See [`docs/DESIGN.md`](./docs/DESIGN.md) for the module layout, the ownership model and the full
parity contract.

### Automation

| Command               | What it does |
| --------------------- | ------------ |
| `cargo xtask ci`      | Runs the four checks above |
| `cargo xtask readme`  | Regenerates the YAML and TOML versions of the README's examples, and its list of programs. `--check` fails instead of writing. |
| `cargo xtask site`    | Writes the docs site's pages from this README |
| `cargo xtask release` | Makes a release |

`cargo xtask release` checks that the working tree is clean and asks for the new version. It then
updates the manifests and the README, formats, commits and tags. It asks up front whether to push,
run `cargo publish --workspace` and draft a GitHub release. `--push`, `--publish` and
`--draft-release` answer those questions ahead of time, so a release never stops on a prompt.

### Docs site

The [docs site](https://araxeus.github.io/vendorfiles-rs/) is built from this README and
[`docs/add_to_registry.md`](./docs/add_to_registry.md), so edit those files, not the site's pages. The
home page is the one exception, written by hand in `site/src/home.mdx`.

To preview the site, install [Bun](https://bun.sh) and run:

```bash
cd site
bun install
bun run dev
```

`bun run dev` writes the pages first. After you edit the README, `bun run gen` writes them again. A
new `##` section in the README needs a page in `xtask/src/site/pages.rs`, and the build names any
section that has none.

## License

MIT
