// Examples for the home page console. Terminal output is what `vendor` prints, colors included.

export type Group = 'project' | 'machine';

export interface Recipe {
  id: string;
  group: Group;
  label: string;
  config: string;
  run: string;
  why: string;
  link: string;
}

export const GROUPS: Record<Group, { label: string; file: string }> = {
  project: { label: 'In a project', file: 'vendor.yml' },
  machine: { label: 'On your machine', file: '~/vendor.yml' },
};

const base = '/vendorfiles-rs';
const cyan = (s: string) => `\x1b[36m${s}\x1b[0m`;
const green = (s: string) => `\x1b[32m${s}\x1b[0m`;
const red = (s: string) => `\x1b[31m${s}\x1b[0m`;
const dim = (s: string) => `\x1b[2m${s}\x1b[0m`;
const saved = (path: string) => cyan(`INFO: Saved ${path}`);

export const RECIPES: Recipe[] = [
  {
    id: 'library',
    group: 'project',
    label: 'Library files',
    config: `vendorDependencies:
  Coloris:
    repository: https://github.com/mdbassit/Coloris
    version: v0.17.1
    files:
      - dist/coloris.min.js
      - dist/coloris.min.css`,
    run: [
      '$ vendor sync',
      saved('vendor/Coloris/coloris.min.js'),
      saved('vendor/Coloris/coloris.min.css'),
      green('SUCCESS: Installed Coloris v0.17.1'),
    ].join('\n'),
    why: 'Pick single files from any repository, pinned to a release.',
    link: `${base}/getting-started/quick-start/`,
  },
  {
    id: 'globs',
    group: 'project',
    label: 'Folders and globs',
    config: `vendorDependencies:
  skills:
    repository: https://github.com/anthropics/skills
    version: e4bb5d960d87126ff3b3a8af1a104b7db67560ba
    files:
      skills/canvas-design/canvas-fonts/: fonts/
      skills/**/SKILL.md: '{parent}.md'`,
    run: [
      '$ vendor sync',
      saved('vendor/skills/fonts/…'),
      saved('vendor/skills/canvas-design.md'),
      saved('vendor/skills/pdf.md'),
      dim('…'),
    ].join('\n'),
    why: 'A path ending in / takes the whole folder. A glob takes every match, and {parent} names each file after its folder.',
    link: `${base}/configuration/folders-and-globs/`,
  },
  {
    id: 'commits',
    group: 'project',
    label: 'Track commits',
    config: `vendorDependencies:
  Cooltipz:
    repository: https://github.com/jackdomleo7/Cooltipz.css
    version: f6ec482ea395cead4fd849c05df6edd8da284a52
    hashVersionFile: package.json
    files:
      - cooltipz.min.css
      - package.json`,
    run: [
      '$ vendor outdated',
      `Cooltipz ${red('f6ec482ea395cead4fd849c05df6edd8da284a52')} -> ${green('7c06dd77f573b017e1b12a7afb01947f697f5c33')}`,
    ].join('\n'),
    why: 'hashVersionFile follows the latest commit to a file, for repositories that never tag releases.',
    link: `${base}/configuration/versions/#tracking-commits-instead-of-releases`,
  },
  {
    id: 'lock',
    group: 'project',
    label: 'Lock a version',
    config: `vendorDependencies:
  Coloris:
    repository: https://github.com/mdbassit/Coloris
    version: v0.17.1
    files:
      - dist/coloris.min.js
    locked: true`,
    run: ['$ vendor outdated', dim('# Coloris is locked, so outdated and update skip it')].join('\n'),
    why: 'locked keeps one dependency where it is, while vendor update moves the rest.',
    link: `${base}/configuration/locking/`,
  },
  {
    id: 'byname',
    group: 'machine',
    label: 'Programs by name',
    config: `# Nothing to write by hand.
# vendor add puts each entry in ~/vendor.yml for you.`,
    run: [
      '$ vendor search disk',
      `${cyan('NAME    ALIASES  DESCRIPTION')}`,
      'bottom  btm      System monitor for processes, CPU, memory…',
      'dust    -        Disk usage viewer showing what takes up space…',
      '',
      '$ vendor add rg fd',
    ].join('\n'),
    why: 'The registry knows each program’s release assets for Windows, macOS and Linux.',
    link: `${base}/getting-started/program-registry/`,
  },
  {
    id: 'archive',
    group: 'machine',
    label: 'Release archives',
    config: `vendorDependencies:
  fzf:
    repository: https://github.com/junegunn/fzf
    version: 0.38.0
    files:
      - '{release}/fzf-{version}-linux_amd64.tar.gz':
          - fzf
        '{release}/fzf-{version}-windows_amd64.zip':
          fzf.exe: my-custom-fzf.exe`,
    run: [
      '$ vendor sync',
      saved('vendor/fzf/fzf'),
      saved('vendor/fzf/my-custom-fzf.exe'),
      green('SUCCESS: Installed fzf 0.38.0'),
    ].join('\n'),
    why: 'Map an archive to the files you want. A list keeps their names, and a map renames them.',
    link: `${base}/configuration/versions/#extracting-files-from-archives`,
  },
  {
    id: 'url',
    group: 'machine',
    label: 'Any website',
    config: `vendorDependencies:
  FilePilot:
    url: https://filepilot.tech/download/v{version}
    versionUrl: https://filepilot.tech/download/latest
    versionRegex: v(\\d+\\.\\d+\\.\\d+)
    files: FPilot.exe`,
    run: ['$ vendor sync', saved('vendor/FilePilot/FPilot.exe'), green('SUCCESS: Installed FilePilot')].join('\n'),
    why: 'Download from any site. vendor reads the latest version from a page, a redirect or JSON.',
    link: `${base}/configuration/other-sites/`,
  },
  {
    id: 'installer',
    group: 'machine',
    label: 'Windows installers',
    config: `vendorDependencies:
  winmerge:
    repository: https://github.com/WinMerge/winmerge
    files: '{release}/WinMerge-{version}-x64-PerUser-Setup.exe'
    installer: true`,
    run: ['$ vendor sync', dim('# downloads the setup, runs it silently, then deletes it'), '', '$ vendor uninstall winmerge'].join('\n'),
    why: 'vendor runs NSIS, Inno, MSI and MSIX setups silently, and uninstalls them later.',
    link: `${base}/configuration/installers/`,
  },
];
