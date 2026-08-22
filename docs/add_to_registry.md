### Adding a program to the registry

The list lives in [`registry.yml`](./registry.yml) at the root of this repository. Open a pull
request adding an entry and, once merged, `vendor add <name>` works for everyone - no new release
needed. Most projects need a few lines:

```yaml
  fd:
    aliases: [fdfind, fd-find]
    repository: https://github.com/sharkdp/fd
    asset: "{release}/fd-v{version}-{target}{ext}"
    member: "fd-v{version}-{target}/fd{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc
      macos-aarch64: aarch64-apple-darwin
      linux-x86_64: x86_64-unknown-linux-gnu
```

`{target}` is the triple your host maps to, `{ext}` is `.zip` on Windows and `.tar.gz` elsewhere,
and `{exe}` is `.exe` on Windows. Projects that name assets some other way spell each host out
instead, with its own `asset` and `member`.

Projects that publish a bare binary rather than an archive leave `member` out entirely, and use
`as` when the asset's name is not the command you would type:

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

A program whose executable cannot run alone gives `member` a *list* instead, and every path in it
is written out as it stands rather than flattened to its basename. `herdr.exe` loads the `conpty/`
directory shipped beside it, and the two `OpenConsole.exe` builds in there would collide the moment
the directories were dropped:

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

Give `member` a *map* instead of a list to say where each file goes, which is how something
buried in the archive arrives under the name you want. And where a release splits what one host
needs across more than one asset, the host names them as a list:

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

Every path is relative to the archive root, going in as well as coming out; one that climbs out
of it is refused. So is `as` beside a list or a map - it renames a single downloaded file, and
those name several. One `member` still lands under its basename, so the archive's own versioned
directory does not end up in your vendor folder.

Repositories that publish several trains of releases need `releaseRegex` to say which tags count,
and a file from the repository is vendored with `path` instead of an asset:

```yaml
  bitwarden-secrets-cli:
    aliases: [bws]
    repository: https://github.com/bitwarden/sdk
    releaseRegex: '^bws-v\d+\.\d+\.\d+$'
    asset: "{release}/bws-{target}-{version}.zip"
    member: "bws{exe}"
    targets:
      windows-x86_64: x86_64-pc-windows-msvc

  some-theme:
    repository: https://github.com/example/themes
    path: themes/example.json
    hashVersionFile: true      # track the file by commit; no `targets` needed
```

[`registry.schema.json`](./registry.schema.json) describes the format, and `registry.yml` points
at it, so an editor with YAML language-server support flags a wrong field or a malformed host key
as you type - before CI, and before the PR. Test your entry too:

```bash
VENDOR_REGISTRY=./registry.yml vendor add <name> --dry-run   # what it resolves to
VENDOR_REGISTRY=./registry.yml vendor add <name>             # the real thing
```

`--dry-run` answers "what would this put in my config" without contacting GitHub or touching a
file, which makes it the quickest way to check an entry:

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

Two checks guard the file. Every pull request proves it parses, that each entry resolves for every
host it lists, and that anything with a `member` is a container the extractor can open. A change to
`registry.yml` additionally queries GitHub: the asset each entry names must exist for every host it
claims, and the members inside it are downloaded and checked on one platform. The same checks run
weekly, since a project can rename its assets without anyone touching this repository.

Members are verified on one platform rather than all of them - every platform would mean
downloading every asset - so an entry whose layout differs *between* platforms still deserves a
manual look. `microsoft/edit` and `sinelaw/fresh` both nest their binary on one platform and not
another.

A few notes on how it behaves. The registry is only read by `install`/`add`, never by `sync` or
`update`. It is cached for a day, so the usual install makes no request at all; after that the
check is conditional, and `--refresh` forces it. If it cannot be reached, `vendor` says so and
carries on with its normal search, so being offline costs you nothing but the shorthand. And an
entry can only say *what* to fetch - there is no `vendorFolder` in the format, and unknown keys are
refused, so a registry can never redirect writes on your machine.
