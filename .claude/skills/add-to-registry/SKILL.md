---
name: add-to-registry
description: Use when adding a program to registry.yml or changing an existing entry in this repo, including bare requests like "add owner/repo", fixing an entry after upstream repackages its releases, changing an entry's hosts or aliases, or rewording a description for `vendor search`.
---

# Adding to the registry

`registry.yml` tells `vendor add <name>` what to download. The format is documented in the header of
`registry.yml` and in `docs/add_to_registry.md`, so read the header before writing an entry.

An entry must match what upstream publishes today. Let the release assets decide what goes in,
because memory, asset names and the requester's description of upstream can all be out of date.

## 1. Check what exists

Search `registry.yml` for the program and its repository. If an entry exists, verify it against the
latest release and change only what is wrong or what was asked for.

Treat a claim about upstream as something to check. A host stays out if its asset does not exist,
and the report says why.

A change to description or aliases alone skips steps 2 and 3's entry work and the install in step 4.

## 2. Research upstream

```bash
gh release view -R owner/repo --json tagName,assets -q '.tagName, (.assets[].name)'
gh release list -R owner/repo --limit 10
```

Download archives into a new scratch folder and list them from Git Bash with `unzip -l` or `tar tf`.
List one Windows and one Unix archive at least, because the layout often differs between them. A
leading `./` in a listing stays out of `member`, because paths compare normalized.

| What you find | What to do |
| --- | --- |
| Only a source tarball | Look for a builds repo with `gh repo list <org>` and point `repository` there, as `tmux` does with `tmux/tmux-builds` |
| Several kinds of tag in the release list | Add an anchored `releaseRegex`, as `biome` does, because "latest" can land on a tag with no binaries |
| A rolling release marked pre-release (`preview`, `nightly`) | Leave it, because "latest" skips pre-releases. A regex takes the newest match of any kind, so anchor it |
| A rolling release not marked pre-release (neovim's `stable`) | Add an anchored `releaseRegex`, because nothing else excludes it |
| A tag holding `x.y.z` | `{version}` is that `x.y.z` alone: `@biomejs/biome@2.6.4` gives `2.6.4` |
| A tag with no `x.y.z` | `{version}` is the tag minus leading `v`s: `v3.7c` gives `3.7c` |
| Both gnu and musl Linux builds | Use gnu, as `fd` and `bat` do. Use musl where gnu is missing |

### Hosts

The registry uses six host keys: `windows`, `macos` and `linux`, each with `x86_64` and `aarch64`.
List each of the six that upstream publishes. Report any the requester did not name. Leave out other
builds, such as 32-bit, riscv64 or FreeBSD.

### Programs that cannot run alone

Run the program and exercise a real feature, because `--version` can pass on a broken install.
`nvim.exe` with its DLLs passes `--version` but finds no syntax files. Use a headless mode or a real
command, such as `nvim --headless --clean` or `rg needle`.

- A few companion files fit a `member` list, as `herdr` does, when the archive has no top folder.
  A list keeps every directory, so use a map to drop a top folder.
- An optional companion binary, such as yazi's `ya`, is the maintainer's call, so report it.
- A large tree does not fit the format. Stop before writing the entry and report, because the choice
  belongs to the maintainer.

These facts about large trees are already tested, so you need not reproduce them:

- `member` takes exact file paths. A directory as `member` installs once, then the next update fails
  with `Access is denied (os error 5)`, which looks like a locked file. The live test also rejects
  directories.
- Neovim needs about 2,130 files per host, and that list changes between releases.

Offer these options:

- Teach the engine to take directories, which means `vendor` deletes a folder an entry names.
- Generate a full file list, about 2,130 lines per host, which goes stale between releases.
- Use a single-file asset such as an AppImage, which covers Linux only.
- Leave the program out.

## 3. Write the entry

Use the compact form (`asset`, `member`, `targets`) when one pattern fits every host. Placeholders
can sit anywhere, so `{release}/eza{exe}_{target}{ext}` covers `eza.exe_x86_64-pc-windows-gnu.zip`.
Spell hosts out when the layouts differ.

Give a program one key. Aliases are other names for the same program, so a different program gets
its own entry even when it copies a name: `tmux` and `psmux` are separate.

Add a comment only for what the entry cannot show, such as missing hosts or an odd layout. Verify
each claim in it.

### Descriptions

A test caps descriptions at 72 characters, so measure them before applying. Names, aliases and the
repository path match by substring. A description splits on non-alphanumerics and matches the start
of each word, with no stemming. That has consequences:

- `pager` does not match "paging", and `yml` does not match "yaml", so write the words people type.
- `.gitignore` becomes a word starting with `git`, so it matches `vendor search git`.
- A word already in the name adds no search hit, so spend the characters elsewhere.

Name the category, what it replaces, and the formats or services it handles. When the requester
dictates the content, follow it.

Rewording can lose search hits. Find the losses by running `vendor search` with `VENDOR_REGISTRY`
pointed at a copy from `git show HEAD:registry.yml`, and report any you find.

## 4. Test

The default config points at the maintainer's live tools. Pass `-c` to every `vendor` command that
reads or writes a config, `--dry-run` included. Without it, a dry run for a name already in that
config shows the live entry instead of the registry's.

Replace `<scratch>` with the literal path of a new folder holding a `vendor.json` that contains
`{"vendorFolder":"./vendor","vendorDependencies":{}}`.

```bash
export VENDOR_REGISTRY=./registry.yml

cargo build
./target/debug/vendor -p -c <scratch> add <name>
<scratch>/vendor/<name>/<binary> --version
./target/debug/vendor -p -c <scratch> outdated
./target/debug/vendor search <word from the description>

cargo xtask readme
cargo test
GITHUB_TOKEN=$(gh auth token) cargo test -p vendorfiles_core --lib registry -- --ignored
```

The first build in a fresh worktree takes minutes, so run long commands in the background. If a
permission check blocks running a downloaded binary, report it as untested and give the exact
commands to run.

Four limits on those results:

- The live test checks asset names on every host but members on only one. To check another host,
  hand-write its `files` entry in a scratch config and run `vendor sync`, which uses the real
  extractor on any OS's archive. WSL can run the Linux x86_64 binary it produces.
- `cargo xtask readme` regenerates only the program table. The `vendor search` examples near that
  table are hand-written, so run each query again and correct any that are wrong.
- `xtask` compiles its checkout path in. In a worktree, use that worktree's own target folder and
  leave `CARGO_TARGET_DIR` unset, or it can read and write another checkout's README.
- On Windows, Python text mode writes CRLF. Check for LF with `grep -c $'\r' registry.yml`.

## 5. Report

Keep the report to these parts, in order:

1. What changed, and what you left out with the reason.
2. What you tested, with results.
3. What you could not test, such as a binary for another OS. Say so plainly, because a passing live
   test does not prove the program runs.
4. Decisions for the maintainer, such as hosts beyond the request, lost search words, or gnu versus
   musl.
