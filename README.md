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

vendorfiles pulls files from GitHub repositories, git hosts and plain URLs, and keeps them up to
date. List the CSS libraries, binaries or config files you want in a config file, run
`vendor sync`, and they land in your project.

- Download any file from any GitHub repository, or from any URL
- Download release assets, and unpack only the files you need from zip, tar, tar.gz and crx
  archives
- Track versions by release, by commit, by git tag or from a version URL
- Write the config in TOML, YAML, JSON, JSON5 or `package.json`
- Download in parallel, streaming each file straight to disk
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
  - [Versions and releases](#versions-and-releases)
  - [Downloading from other sites](#downloading-from-other-sites)
  - [Locking a dependency](#locking-a-dependency)
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
| `files` | The files to download |
| `vendorFolder` | Where to save this dependency's files ([output folders](#output-folders)) |
| `hashVersionFile` | Track a file's commits instead of releases ([details](#tracking-commits-instead-of-releases)) |
| `releaseRegex` | Which releases count when looking for the latest ([details](#choosing-which-releases-count)) |
| `locked` | Keep `vendor update` away from this dependency ([details](#locking-a-dependency)) |
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

### Versions and releases

By default, `version` holds a release tag, and `vendor update` moves a dependency to the newest
release. You can also [track commits](#tracking-commits-instead-of-releases) instead.

#### Release assets

A file that starts with `{release}/` comes from the release's assets instead of the repository.
`{version}` in a path expands to the version number in the release tag. That's the first `x.y.z`
in the tag, or the tag with its leading `v`s removed:

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

- The supported formats are zip, tar, gzip, xz, tar.gz, tar.xz and `.crx`/`.xpi`. `vendor` detects
  the format from the file's contents, not its name.
- Only the files you name are unpacked.
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

A dependency that isn't on GitHub sets `url` instead of `repository`. Its files are URLs, and
`{url}` in a file expands to the `url` value:

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
            "files": { "{url}": "FPilot.exe" }
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
    files:
      '{url}': FPilot.exe
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.FilePilot]
url = 'https://filepilot.tech/download/v{version}'
versionUrl = 'https://filepilot.tech/download/latest'
versionRegex = 'v(\d+\.\d+\.\d+)'

[vendorDependencies.FilePilot.files]
'{url}' = 'FPilot.exe'
```

</details>
<!-- /formats -->

- Every file maps a URL to the name it's saved under, like `"files": { "{url}": "FPilot.exe" }`.
- A file can also be a full URL, like `https://cdn.example.com/tool.js`.
- Map a URL to a list or a map to [extract files from the archive](#extracting-files-from-archives).
- `{version}` is the version's `x.y.z`, as for releases. `{tag}` is the version exactly as found,
  like `v1.2.3`.
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
            "url": "https://www.nuget.org/api/v2/package/libclang.runtime.win-x64/{version}",
            "versionUrl": "https://api.nuget.org/v3-flatcontainer/libclang.runtime.win-x64/index.json",
            "versionJsonPointer": "versions",
            "files": { "{url}": { "runtimes/win-x64/native/libclang.dll": "libclang.dll" } }
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
    url: https://www.nuget.org/api/v2/package/libclang.runtime.win-x64/{version}
    versionUrl: https://api.nuget.org/v3-flatcontainer/libclang.runtime.win-x64/index.json
    versionJsonPointer: versions
    files:
      '{url}':
        runtimes/win-x64/native/libclang.dll: libclang.dll
```

</details>

<details>
<summary>TOML</summary>

```toml
[vendorDependencies.libclang]
url = 'https://www.nuget.org/api/v2/package/libclang.runtime.win-x64/{version}'
versionUrl = 'https://api.nuget.org/v3-flatcontainer/libclang.runtime.win-x64/index.json'
versionJsonPointer = 'versions'

[vendorDependencies.libclang.files.'{url}']
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

---

### `vendor list`

`vendor list` prints the dependencies in the config. `vendor config list` does the same.

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
description, or its repository. Name matches come first.

```bash
vendor search          # everything
vendor search disk     # bottom, btop, dust, dysk and gdu
vendor search git      # delta, gh, lazygit and actionlint
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
<summary>Programs you can add by name (49)</summary>

| Program | Aliases | Description | Platforms |
| --- | --- | --- | --- |
| [actionlint](https://github.com/rhysd/actionlint) |  | Linter for GitHub Actions workflow YAML/YML files and CI pipelines | All |
| [ansi2txt](https://github.com/gabe565/ansi2txt) | `ansi2text` | Strips ANSI color and escape codes from text, leaving plain text | All |
| [atuin](https://github.com/atuinsh/atuin) |  | Searchable shell and command history, synced across machines | Windows x64, macOS, Linux |
| [bat](https://github.com/sharkdp/bat) |  | cat clone and file viewer with syntax highlighting and a pager | All |
| [biome](https://github.com/biomejs/biome) |  | JavaScript, TypeScript, JSON, CSS and YAML/YML formatter and linter | All |
| [bitwarden-secrets-cli](https://github.com/bitwarden/sdk) | `bws` | Bitwarden Secrets Manager CLI for fetching secrets in scripts and CI | All |
| [bottom](https://github.com/ClementTsang/bottom) | `btm` | System monitor for processes, CPU, memory, disks and network, like htop | All |
| [btop](https://github.com/aristocratos/btop) |  | Resource monitor for CPU, memory, disks, network and processes like htop | Linux |
| [carapace](https://github.com/carapace-sh/carapace-bin) | `carapace-bin` | Shell completion for hundreds of commands in bash, zsh, fish and more | All |
| [delta](https://github.com/dandavison/delta) | `git-delta` | Syntax-highlighting pager and diff viewer for git, diff, grep and blame | Windows x64, macOS arm64, Linux |
| [dust](https://github.com/bootandy/dust) |  | Disk usage viewer showing what takes up space, a more intuitive du | All |
| [dysk](https://github.com/Canop/dysk) |  | Lists mounted disks and filesystems with free space, a friendlier df | Windows x64, macOS, Linux |
| [edamame](https://github.com/mijowi/edamame) |  | Terminal text editor for Markdown that stays rendered while you edit | Windows x64, macOS, Linux |
| [edit](https://github.com/microsoft/edit) | `msedit` | Microsoft's small terminal text editor, a modern MS-DOS Edit | Windows, macOS arm64, Linux |
| [fastfetch](https://github.com/fastfetch-cli/fastfetch) |  | Neofetch-like tool that shows system information in the terminal | All |
| [fd](https://github.com/sharkdp/fd) | `fdfind`, `fd-find` | Fast, user-friendly alternative to find for locating files and folders | Windows, macOS arm64, Linux |
| [file-pilot](https://github.com/Araxeus/fpilot-scraper) | `fpilot`, `filepilot` | Fast, lightweight file manager and Explorer replacement for Windows | Windows x64 |
| [flow](https://github.com/neurocyte/flow) |  | Flow Control, a programmer's text editor for the terminal | All |
| [fresh](https://github.com/sinelaw/fresh) |  | Fast, easy terminal text editor and IDE | All |
| [fzf](https://github.com/junegunn/fzf) |  | Interactive command-line fuzzy finder for files, history and any list | All |
| [gdu](https://github.com/dundee/gdu) |  | Fast disk usage analyzer with a terminal UI | All |
| [gh](https://github.com/cli/cli) | `gh-cli`, `github-cli` | GitHub's official CLI for pull requests, issues, releases and repos | All |
| [gum](https://github.com/charmbracelet/gum) |  | Prompts, menus, spinners and styled text for glamorous shell scripts | Windows x64, macOS, Linux |
| [herdr](https://github.com/herdrdev/herdr) |  | Terminal runtime for running and managing AI coding agents | Windows x64, macOS, Linux |
| [hide-console](https://github.com/Araxeus/hide-console) | `hide` | Run a command on Windows with its console window hidden | Windows x64 |
| [hyperfine](https://github.com/sharkdp/hyperfine) |  | Command-line benchmarking tool that times and compares commands | All |
| [jq](https://github.com/jqlang/jq) |  | Command-line JSON processor for querying, filtering and transforming | All |
| [just](https://github.com/casey/just) |  | Command runner for project tasks and scripts, like make without builds | All |
| [lazygit](https://github.com/jesseduffield/lazygit) |  | Terminal UI for git that handles staging, commits, branches and rebases | All |
| [ls-interactive](https://github.com/Araxeus/ls-interactive) | `lsi` | Interactive ls for browsing files and navigating directories | All |
| [micro](https://github.com/zyedidia/micro) |  | Terminal text editor with familiar keys and mouse support, a modern nano | All |
| [nushell](https://github.com/nushell/nushell) | `nu` | Cross-platform shell that treats command output as structured data | All |
| [ov](https://github.com/noborus/ov) |  | Feature-rich terminal pager and text viewer, a modern take on less | All |
| [psmux](https://github.com/psmux/psmux) | `pmux` | Native tmux for Windows, a terminal multiplexer for PowerShell and cmd | Windows |
| [rclone](https://github.com/rclone/rclone) |  | rsync-style sync to cloud storage like S3, Google Drive and Dropbox | All |
| [ripgrep](https://github.com/BurntSushi/ripgrep) | `rg` | Fast recursive regex search of file contents, like grep | All |
| [ruff](https://github.com/astral-sh/ruff) |  | Fast Python linter and code formatter | All |
| [shfmt](https://github.com/mvdan/sh) |  | Shell script formatter for bash, POSIX sh and mksh | Windows x64, macOS, Linux |
| [sshm](https://github.com/Gu1llaum-3/sshm) |  | Terminal UI for managing and connecting to SSH hosts | Windows x64, macOS, Linux |
| [starship](https://github.com/starship/starship) |  | Fast, customizable prompt for any shell: bash, zsh, fish, PowerShell | All |
| [surge](https://github.com/surge-downloader/surge) |  | Fast file download manager with a terminal UI | All |
| [tmux](https://github.com/tmux/tmux-builds) |  | Terminal multiplexer with split panes and sessions that outlive detach | macOS, Linux |
| [ttt](https://github.com/eugenioenko/ttt) |  | Terminal text editor that feels like a GUI one, such as VS Code | Windows x64, macOS, Linux |
| [upx](https://github.com/upx/upx) |  | Executable packer that compresses binaries to shrink their size | Windows x64, Linux |
| [wsl-notify-send](https://github.com/stuartleeks/wsl-notify-send) |  | Sends Windows desktop notifications from inside WSL, like notify-send | Windows x64 |
| [yamlfmt](https://github.com/google/yamlfmt) |  | YAML file formatter | All |
| [yq](https://github.com/mikefarah/yq) |  | Command-line YAML, JSON, XML, CSV and TOML processor, like jq | All |
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) |  | Download video and audio from YouTube and many other sites | All |
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
