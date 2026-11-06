#!/usr/bin/env node
// Build the npm packages of the rigswig distribution: one package per platform carrying the prebuilt
// binary (@kftcda/cli-<os>-<arch>, the same shape as esbuild's and Biome's npm packages) and the
// unscoped `rigswig` picker, which installs the right one through optionalDependencies and whose shim
// (npm/rigswig/bin/rigswig.js) runs it. Zero dependencies, node >= 18. The release workflow runs one
// platform build per matrix leg and the picker in its `npm` job; the one-time bootstrap publish after
// the first real release also runs `--releases` by hand (docs/RELEASING.md):
//
//   node npm/build.mjs --version v0.1.1 --target x86_64-unknown-linux-musl \
//     --binary target/x86_64-unknown-linux-musl/release/rigswig --out npm-stage
//   node npm/build.mjs --version v0.1.1 --picker --out npm-stage
//   node npm/build.mjs --version v0.1.1 --releases /tmp/unpacked --out npm-stage
//
// Everything lands under --out, one directory per package, ready for `npm publish`. Only here does
// the description, repository or licence come along; the description and keywords are read from
// Cargo.toml (the workspace metadata) so they cannot drift.
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

export const SCOPE = '@kftcda';
export const PICKER = 'rigswig';
const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, '..');

// The rust target triple -> its npm package, the OS and CPU npm matches installs against, the label
// for its README, and the binary file extension. The Linux binaries are musl and static, so they run
// on any Linux regardless of libc: one cli-linux package each, like esbuild's `-musl` ones.
export const TARGETS = {
  'aarch64-apple-darwin': {
    package: 'cli-darwin-arm64', os: ['darwin'], cpu: ['arm64'], ext: '', label: 'macOS, Apple Silicon',
  },
  'x86_64-apple-darwin': {
    package: 'cli-darwin-x64', os: ['darwin'], cpu: ['x64'], ext: '', label: 'macOS, Intel',
  },
  'x86_64-unknown-linux-musl': {
    package: 'cli-linux-x64', os: ['linux'], cpu: ['x64'], ext: '', label: 'Linux x64, static (musl)',
  },
  'aarch64-unknown-linux-musl': {
    package: 'cli-linux-arm64', os: ['linux'], cpu: ['arm64'], ext: '', label: 'Linux arm64, static (musl)',
  },
  'x86_64-pc-windows-msvc': {
    package: 'cli-win32-x64', os: ['win32'], cpu: ['x64'], ext: '.exe', label: 'Windows x64',
  },
};

// A pushed tag (`v0.1.1`) or a plain version. Published npm versions carry no leading v.
export function npmVersion(tag) {
  const version = /^v?(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)$/.exec(tag)?.[1];
  if (!version) throw new Error(`npm version: ${JSON.stringify(tag)} is not a semver tag (v1.2.3, ...)`);
  return version;
}

// From [workspace.package] in the workspace's own Cargo.toml: the single source for the fields npm
// wants. A tiny slice parse, enough for the one format this repo's Cargo.toml uses.
export function crateMeta() {
  const toml = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
  const value = (key) => new RegExp(`^${key} = "(.*)"\\s*$`, 'm').exec(toml)?.[1];
  const repository = value('repository');
  const description = value('description');
  if (!repository || !description) {
    throw new Error('Cargo.toml: expected repository and description in [workspace.package]');
  }
  const keywords = (/^keywords = \[(.*)\]/m.exec(toml)?.[1] ?? '').match(/"([^"]+)"/g)?.map((quoted) => quoted.slice(1, -1)) ?? [];
  return { repository, description, keywords };
}

function repositoryField() {
  const repository = crateMeta().repository;
  const url = /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+$/.test(repository) ? `git+${repository}.git` : repository;
  return { type: 'git', url };
}

function writeManifest(dir, manifest) {
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'package.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  return dir;
}

function copyIn(dir, files) {
  for (const [from, to] of files) {
    mkdirSync(dirname(join(dir, to)), { recursive: true });
    copyFileSync(from, join(dir, to));
  }
}

function executable(path) {
  chmodSync(path, 0o755);
  return path;
}

function platformReadme(name, spec) {
  const meta = crateMeta();
  return [
    `# ${name}`,
    '',
    `The prebuilt rigswig binary for ${spec.label}. This package is part of the [rigswig](https://github.com/overfocus/rigswig)`,
    'npm distribution: install the `rigswig` package instead, and it brings this one along when your platform matches',
    '(`npx rigswig`, `npm install -g rigswig`). Do not install it directly.',
    '',
    `rigswig ${meta.description.split('.')[0]}. See the [repository](https://github.com/overfocus/rigswig)`,
    'for what it reads, its guarantees and the rest of the documentation.',
    '',
  ].join('\n');
}

// One platform package: the manifest, the binary (named and made executable), the licence copies and
// a short README. Returns the directory it wrote, for the caller to `npm publish` from.
// The manifest declares no `bin`: when sibling optional dependencies fail their os/cpu check, npm's
// install rollback deletes bin links by their shared name, which would take the picker's own
// `node_modules/.bin/rigswig` down with them (npm exec then finds no command). The shim locates the
// binary by package-relative path instead (npm/rigswig/bin/rigswig.js), so nothing needs the field.
export function buildPlatform(target, { version, binary, out }) {
  const spec = TARGETS[target];
  if (!spec) {
    throw new Error(`unknown target ${JSON.stringify(target)}, have: ${Object.keys(TARGETS).join(', ')}`);
  }
  const name = `${SCOPE}/${spec.package}`;
  const file = `rigswig${spec.ext}`;
  if (typeof binary !== 'string' || !existsSync(binary)) {
    throw new Error(`no binary at ${binary}; build with cargo build --release --target ${target}, or pass --binary`);
  }
  const dir = writeManifest(join(out, name), {
    name,
    version,
    description: `rigswig for ${spec.label}, as a prebuilt binary. Part of the rigswig npm distribution; install rigswig instead.`,
    license: 'Apache-2.0',
    repository: repositoryField(),
    os: spec.os,
    cpu: spec.cpu,
    files: ['bin', 'README.md', 'LICENSE', 'NOTICE'],
  });
  copyIn(dir, [
    [binary, `bin/${file}`],
    [join(ROOT, 'LICENSE'), 'LICENSE'],
    [join(ROOT, 'NOTICE'), 'NOTICE'],
  ]);
  executable(join(dir, 'bin', file));
  writeFileSync(join(dir, 'README.md'), platformReadme(name, spec));
  return dir;
}

// The unscoped picker: no binary of its own, just the checked-in shim, the repo's README and the five
// platform packages as optional dependencies pinned to the same version.
export function buildPicker({ version, out }) {
  const meta = crateMeta();
  const dir = writeManifest(join(out, PICKER), {
    name: PICKER,
    version,
    description: meta.description,
    license: 'Apache-2.0',
    repository: repositoryField(),
    keywords: meta.keywords,
    engines: { node: '>=18' },
    bin: { rigswig: 'bin/rigswig.js' },
    files: ['bin', 'README.md', 'LICENSE', 'NOTICE'],
    optionalDependencies: Object.fromEntries(
      Object.values(TARGETS).map((spec) => [`${SCOPE}/${spec.package}`, version]),
    ),
  });
  copyIn(dir, [
    [join(HERE, 'rigswig', 'bin', 'rigswig.js'), 'bin/rigswig.js'],
    [join(ROOT, 'README.md'), 'README.md'],
    [join(ROOT, 'LICENSE'), 'LICENSE'],
    [join(ROOT, 'NOTICE'), 'NOTICE'],
  ]);
  executable(join(dir, 'bin', 'rigswig.js'));
  return dir;
}

// The one-time bootstrap in docs/RELEASING.md: point this at a folder of unpacked release tarballs
// (rigswig-<tag>-<target>/rigswig) and get all five platform packages at once.
export function buildFromReleases({ version, rawTag, releases, out }) {
  return Object.keys(TARGETS).map((target) => {
    const spec = TARGETS[target];
    const binary = join(releases, `rigswig-${rawTag}-${target}`, `rigswig${spec.ext}`);
    return buildPlatform(target, { version, binary, out });
  });
}

// -- the command line ---------------------------------------------------------

const USAGE = [
  'usage: node npm/build.mjs --version <vX.Y.Z> --out <dir> one of:',
  '  --target <rust-triple> --binary <path/to/rigswig>',
  '  --releases <dir of unpacked rigswig-<tag>-<target> folders>',
  '  --picker',
].join('\n');

function parseArgs(argv) {
  const flags = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) throw new Error(USAGE);
    const key = arg.slice(2).replace(/-(\w)/g, (_dash, letter) => letter.toUpperCase());
    const next = argv[i + 1];
    if (next && !next.startsWith('--')) {
      flags[key] = next;
      i++;
    } else {
      flags[key] = true;
    }
  }
  return flags;
}

function main(argv) {
  const flags = parseArgs(argv);
  if (!flags.version || !flags.out) throw new Error(USAGE);
  const version = npmVersion(flags.version);
  const rawTag = flags.tag || `v${version}`;
  let built;
  if (flags.picker) {
    built = [buildPicker({ version, out: flags.out })];
  } else if (flags.releases) {
    built = buildFromReleases({ version, rawTag, releases: flags.releases, out: flags.out });
  } else if (flags.target && flags.binary) {
    built = [buildPlatform(flags.target, { version, binary: flags.binary, out: flags.out })];
  } else {
    throw new Error(USAGE);
  }
  for (const dir of built) console.log(dir);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (err) {
    console.error(`npm/build.mjs: ${err.message}`);
    process.exit(1);
  }
}
