# Adding a program to the registry

The registry is the list of programs that `vendor add <name>` knows by name. It lives in
[`registry.yml`](../registry.yml) at the root of this repository.

To add a program, open a pull request with a new entry. Once it is merged, `vendor add <name>`
works for everyone. No new release of vendorfiles is needed.

## A basic entry

Most programs need only a few lines. This is the entry for `fd`:

```yaml
  fd:
    description: "Fast, user-friendly alternative to find"
    aliases: [fdfind, fd-find]
    repository: https://github.com/sharkdp/fd
    asset: "{release}/fd-v{version}-{target}{ext}"
    member: "fd-v{version}-{target}/fd{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
```

| Field | What it holds |
| --- | --- |
| `description` | One short line on what the program does |
| `aliases` | Other names `vendor add` accepts for the program |
| `repository` | The program's GitHub repository |
| `asset` | The release asset to download |
| `member` | The file to take out of that asset |
| `targets` | The hosts the program supports, each mapped to its `{target}` |

`vendor search` matches the words in `description`, so use the words someone would search for.
The `gdu` entry says "disk usage", which is how `vendor search disk` finds it.

Three placeholders fill in the host's details:

| Placeholder | Becomes |
| --- | --- |
| `{target}` | The value the host maps to in `targets`, like `x86_64-pc-windows-msvc` |
| `{ext}` | `.zip` on Windows and `.tar.gz` elsewhere |
| `{exe}` | `.exe` on Windows and nothing elsewhere |

If a project's asset names don't follow one pattern, give each host its own `asset` and `member`
instead. The `ox`, `herdr` and `programz` entries below do this.

## Programs that ship a bare binary

Some projects publish the executable itself instead of an archive. Leave `member` out for these.
Use `as` when the file's name is not the command you would type.

```yaml
  ox:
    repository: https://github.com/curlpipe/ox
    targets:
      windows-x86_64:
        asset: "{release}/ox.exe"
      macos-x86_64:
        asset: "{release}/ox-macos"
        as: "ox"
```

## Programs that need more than one file

Some executables can't run alone. Give `member` a list, and every path in it keeps its folders
instead of being flattened to a file name.

`herdr.exe` is one of these. It loads the `conpty/` folder shipped next to it, and that folder
holds two builds of `OpenConsole.exe`. Without their folders, the two would overwrite each other.

```yaml
  herdr:
    repository: https://github.com/herdrdev/herdr
    targets:
      windows-x86_64:
        asset: "{release}/herdr-windows-x86_64.zip"
        member:
          - herdr.exe
          - conpty/conpty.dll
          - conpty/x64/OpenConsole.exe
          - conpty/arm64/OpenConsole.exe
      linux-x86_64:
        asset: "{release}/herdr-linux-x86_64"
        as: herdr
```

## Renaming files and using several assets

Give `member` a map to choose where each file goes. That's how a file buried deep in the archive
arrives under the name you want.

When a release splits what one host needs across several assets, list them under that host.

```yaml
  programz:
    repository: https://github.com/example/programz
    targets:
      windows-x86_64:
        - asset: "{release}/programz-win.zip"
          member:
            x64/programz.exe: program.exe    # renamed out of a subdirectory
            data.bin: data.bin               # taken as it stands
        - asset: "{release}/programz-extra.dll"
```

A few rules apply to `member` and `as`:

- Every path is relative to the archive root, on both sides of a map. A path that climbs out of
  the archive is refused.
- `as` renames one downloaded file. It is refused next to a `member` list or map, because those
  name several files.
- A single `member` lands under its file name, so the archive's versioned folder doesn't end up in
  your vendor folder.

## Repositories with more than one kind of release

Some repositories publish releases for several programs. `releaseRegex` says which tags belong to
yours. The Bitwarden SDK repository, for example, tags its CLI releases `bws-v1.2.3`:

```yaml
  bitwarden-secrets-cli:
    aliases: [bws]
    repository: https://github.com/bitwarden/sdk
    releaseRegex: '^bws-v\d+\.\d+\.\d+$'
    asset: "{release}/bws-{target}-{version}.zip"
    member: "bws{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
```

## Files from the repository itself

To vendor a file from the repository instead of a release asset, use `path`. With
`hashVersionFile: true` the file is tracked by commit, so it needs no `targets`.

```yaml
  some-theme:
    repository: https://github.com/example/themes
    path: themes/example.json
    hashVersionFile: true      # track the file by commit; no `targets` needed
```

## Testing your entry

[`registry.schema.json`](../registry.schema.json) describes the format, and `registry.yml` points
at it. An editor with YAML language-server support therefore flags a wrong field or a malformed
host key as you type, before CI and before the pull request.

Then try the entry. `--dry-run` shows what `vendor add` would put in your config. It doesn't
contact GitHub or touch a file, so it's the quickest check:

```bash
VENDOR_REGISTRY=./registry.yml vendor add <name> --dry-run
```

```console
$ vendor add bws --dry-run
INFO: bitwarden-secrets-cli would be added as:
{
  "bitwarden-secrets-cli": {
    "repository": "https://github.com/bitwarden/sdk",
    "files": [
      {
        "{release}/bws-x86_64-unknown-linux-gnu-{version}.zip": {
          "bws": "bws"
        }
      }
    ],
    "releaseRegex": "^bws-v\\d+\\.\\d+\\.\\d+$"
  }
}
INFO: files would be written to /home/you/project/vendor/bitwarden-secrets-cli
INFO: nothing was downloaded or written
```

When the entry looks right, install it for real:

```bash
VENDOR_REGISTRY=./registry.yml vendor add <name>
```

Last, regenerate the README's list of programs. CI fails while the list is out of date.

```bash
cargo xtask readme
```

## What CI checks

| When | What runs |
| --- | --- |
| Every pull request | The file must parse, each entry must resolve for every host it lists, and anything with a `member` must be an archive the extractor can open. |
| A pull request that changes `registry.yml` | The checks also query GitHub. The asset each entry names must exist for every host it claims, and its members are downloaded and checked on one platform. |
| Every week | The same checks run again, because a project can rename its assets without anyone touching this repository. |

Members are checked on one platform only, because checking all of them would mean downloading
every asset. If an entry's layout differs between platforms, check it by hand. `microsoft/edit`
and `sinelaw/fresh` both nest their binary on one platform and not on another.

## How vendor uses the registry

- `install`/`add`, `search` and `completions` read the registry. `sync` and `update` never do.
- The registry is cached for a day, so most installs make no request for it. After a day,
  `vendor` asks whether it changed. `--refresh` forces that check.
- If the registry can't be reached, `vendor` says so and goes on with its normal GitHub search.
  Being offline only costs you the short names.
- An entry can only say what to fetch. The format has no `vendorFolder`, and unknown keys are
  refused, so a registry can never change where files are written on your machine.
