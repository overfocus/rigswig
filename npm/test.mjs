// Tests for the npm distribution: `node npm/test.mjs` (node >= 18, no dependencies).
// Covers npm/build.mjs (the package builder), npm/rigswig/bin/rigswig.js (the picker shim) and their
// lockstep with the release workflow (.github/workflows/release.yml). Everything runs against
// temporary directories; nothing here publishes or touches the network.
import { strict as assert } from 'node:assert';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as build from './build.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, '..');
const SHIM = join(HERE, 'rigswig', 'bin', 'rigswig.js');
const RELEASE_YML = join(ROOT, '.github', 'workflows', 'release.yml');
const SCOPE = build.SCOPE;
const PLATFORM_PACKAGES = Object.values(build.TARGETS).map((t) => `${SCOPE}/${t.package}`);
const shim = createRequire(SHIM)(SHIM);

function withTmp(label, fn) {
  const dir = mkdtempSync(join(os.tmpdir(), `rigswig-npm-${label}-`));
  try {
    fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function runBuild(args) {
  const res = spawnSync(process.execPath, [join(HERE, 'build.mjs'), ...args], {
    encoding: 'utf8',
  });
  return {
    ok: res.status === 0,
    out: String(res.stdout ?? ''),
    err: String(res.stderr ?? ''),
  };
}

function fakeBinary(path, contents = '#!/bin/sh\nexit 0\n') {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, contents);
  chmodSync(path, 0o755);
  return path;
}

function packageJsonOf(dir) {
  return JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
}

function fakePackage(out, pkg, { withBinary = true } = {}) {
  const dir = join(out, 'node_modules', ...pkg.split('/'));
  mkdirSync(join(dir, 'bin'), { recursive: true });
  writeFileSync(
    join(dir, 'package.json'),
    JSON.stringify({ name: pkg, bin: { rigswig: 'bin/rigswig' } }),
  );
  if (withBinary) fakeBinary(join(dir, 'bin', 'rigswig'), '#!/bin/sh\nexit 0\n');
  return dir;
}

// A stand-in for require.resolve with the same contract: a file path that exists, or MODULE_NOT_FOUND.
function fakeResolve(out) {
  return (id) => {
    const path = join(out, 'node_modules', id);
    if (!existsSync(path)) {
      const err = new Error(`Cannot find module '${id}'`);
      err.code = 'MODULE_NOT_FOUND';
      throw err;
    }
    return path;
  };
}

// -- targets ---------------------------------------------------------------

test('the five platform packages match the targets the release workflow builds', () => {
  const yml = readFileSync(RELEASE_YML, 'utf8');
  const workflow = [...yml.matchAll(/target:\s*([a-z0-9][a-z0-9_-]*[a-z0-9])/g)]
    .map((m) => m[1])
    .filter((t) => t.includes('-') || t.includes('_'));
  assert.equal(workflow.length, 5, `expected 5 matrix targets in release.yml, got: ${workflow}`);
  assert.deepEqual(
    new Set(workflow),
    new Set(Object.keys(build.TARGETS)),
    'release.yml matrix and build.mjs TARGETS disagree',
  );
});

test('release.yml keeps the npm packages off the GitHub release: two artifact sets, two pinned downloads', () => {
  const yml = readFileSync(RELEASE_YML, 'utf8');
  assert.match(yml, /name: binary-\$\{\{ matrix\.target \}\}/, 'the tarball artifacts carry the binary- prefix');
  assert.match(yml, /name: npm-\$\{\{ matrix\.target \}\}/, 'the npm artifacts carry the npm- prefix');
  assert.match(yml, /pattern: binary-\*/, 'the release job downloads only the binary- artifacts');
  assert.match(yml, /pattern: npm-\*/, 'the npm job downloads only the npm- artifacts');
  const downloads = yml.split('\n');
  const steps = downloads
    .map((line, index) => (line.includes('download-artifact') ? index : null))
    .filter((index) => index !== null);
  assert.equal(steps.length, 2, 'both download steps exist');
  for (const step of steps) {
    const around = downloads.slice(step, step + 6).join('\n');
    assert.match(around, /pattern: (binary-\*|npm-\*)/, 'every download pins its pattern');
  }
});

test('the workflow publishes exactly the picker and the five platform packages', () => {
  const yml = readFileSync(RELEASE_YML, 'utf8');
  const list = /for pkg in ([^;]+); do/.exec(yml)?.[1].split(/\s+/);
  assert.ok(list, 'the publish job names its packages in one list');
  assert.deepEqual(
    [...list].sort(),
    [...PLATFORM_PACKAGES, 'rigswig'].sort(),
  );
});

test('no platform package carries the picker\'s bin name: failed siblings must not roll it back', () => {
  // Regression guard for the npm exec failure this session: with `bin` on the platform packages,
  // npm's install rollback deleted `node_modules/.bin/rigswig` for every sibling optional
  // dependency that failed its os/cpu check, leaving npx with no command to run.
  const out = mkdtempSync(join(os.tmpdir(), 'rigswig-npm-nobin-'));
  try {
    assert.ok(runBuild(['--version', 'v0.1.1', '--target', 'x86_64-pc-windows-msvc', '--binary', fakeBinary(join(out, 'rigswig.exe')), '--out', join(out, 'stage')]).ok);
    assert.ok(runBuild(['--version', 'v0.1.1', '--target', 'aarch64-apple-darwin', '--binary', fakeBinary(join(out, 'rigswig')), '--out', join(out, 'stage')]).ok);
    for (const pkg of ['cli-win32-x64', 'cli-darwin-arm64']) {
      assert.ok(!('bin' in packageJsonOf(join(out, 'stage', SCOPE, pkg))), `${pkg} declares no bin`);
    }
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test('every target maps os/cpu/npm-name/bin-extension, and only windows has .exe', () => {
  assert.equal(Object.keys(build.TARGETS).length, 5);
  for (const [triplet, spec] of Object.entries(build.TARGETS)) {
    assert.ok(spec.package.startsWith('cli-'), `${triplet}: package name ${spec.package}`);
    assert.ok(Array.isArray(spec.os) && spec.os.length === 1, `${triplet}: one os`);
    assert.ok(Array.isArray(spec.cpu) && spec.cpu.length === 1, `${triplet}: one cpu`);
    assert.equal(
      spec.ext === '.exe',
      triplet === 'x86_64-pc-windows-msvc',
      `${triplet}: only the windows target carries .exe`,
    );
  }
});

// -- version and crate metadata --------------------------------------------

test('npmVersion accepts a tag or a version and rejects the rest', () => {
  assert.equal(build.npmVersion('v0.1.1'), '0.1.1');
  assert.equal(build.npmVersion('v12.34.56'), '12.34.56');
  assert.equal(build.npmVersion('0.1.1'), '0.1.1');
  assert.throws(() => build.npmVersion('v0.1'));
  assert.throws(() => build.npmVersion('abc'));
});

test('crateMeta reads description, repository and keywords from Cargo.toml', () => {
  const meta = build.crateMeta();
  const toml = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
  assert.ok(toml.includes(meta.description), 'description must come from Cargo.toml');
  assert.match(meta.repository, /^https:\/\/github\.com\/[\w.-]+\/[\w.-]+$/);
  assert.ok(meta.keywords.length > 0);
});

// -- build.mjs: platform packages -------------------------------------------

test('buildPlatform lays out a linux package: manifest, binary, executable bit, README, Licence/NOTICE', () => {
  withTmp('plat', (tmp) => {
    const out = join(tmp, 'stage');
    const res = runBuild([
      '--version', 'v0.1.1',
      '--target', 'x86_64-unknown-linux-musl',
      '--binary', fakeBinary(join(tmp, 'rigswig')),
      '--out', out,
    ]);
    assert.ok(res.ok, res.err);

    const dir = join(out, SCOPE, 'cli-linux-x64');
    const manifest = packageJsonOf(dir);
    assert.equal(manifest.name, `${SCOPE}/cli-linux-x64`);
    assert.equal(manifest.version, '0.1.1');
    assert.deepEqual(manifest.os, ['linux']);
    assert.deepEqual(manifest.cpu, ['x64']);
    assert.ok(!('bin' in manifest), 'platform packages declare no bin field');
    assert.equal(manifest.license, 'Apache-2.0');
    assert.ok(manifest.description.includes('rigswig'));
    assert.ok(manifest.repository.url.includes('github.com/overfocus/rigswig'));
    assert.ok(manifest.files.includes('bin') && manifest.files.includes('NOTICE'));

    const readme = readFileSync(join(dir, 'README.md'), 'utf8');
    assert.equal(readme.split('\n')[0], `# ${SCOPE}/cli-linux-x64`);
    assert.match(readme, /npx rigswig/);

    const bin = join(dir, 'bin', 'rigswig');
    assert.ok(existsSync(bin));
    if (process.platform !== 'win32') {
      assert.equal(statSync(bin).mode & 0o755, 0o755, 'the binary must keep its executable bit');
    }
    assert.ok(existsSync(join(dir, 'LICENSE')));
    assert.ok(existsSync(join(dir, 'NOTICE')));
  });
});

test('buildPlatform for windows names the binary rigswig.exe', () => {
  withTmp('win', (tmp) => {
    const out = join(tmp, 'stage');
    const res = runBuild([
      '--version', '0.2.0',
      '--target', 'x86_64-pc-windows-msvc',
      '--binary', fakeBinary(join(tmp, 'rigswig.exe')),
      '--out', out,
    ]);
    assert.ok(res.ok, res.err);
    const dir = join(out, SCOPE, 'cli-win32-x64');
    assert.deepEqual(packageJsonOf(dir).os, ['win32']);
    assert.ok(existsSync(join(dir, 'bin', 'rigswig.exe')));
    assert.ok(!('bin' in packageJsonOf(dir)), 'the windows platform package declares no bin either');
  });
});

test('buildPlatform refuses an unknown target, and a missing binary', () => {
  withTmp('bad', (tmp) => {
    const badTarget = runBuild(['--version', '0.1.0', '--target', 'powerpc-unknown-none', '--binary', 'x', '--out', tmp]);
    assert.ok(!badTarget.ok);
    assert.match(badTarget.err, /unknown target/);

    const noBinary = runBuild(['--version', '0.1.0', '--target', 'x86_64-unknown-linux-musl', '--binary', join(tmp, 'nope'), '--out', tmp]);
    assert.ok(!noBinary.ok);
    assert.match(noBinary.err, /binary/);
  });
});

test('buildPicker emits the rigswig package: five pinned optionalDependencies, bin, engines, the checked-in shim', () => {
  withTmp('pick', (tmp) => {
    const out = join(tmp, 'stage');
    const res = runBuild(['--version', 'v0.1.1', '--picker', '--out', out]);
    assert.ok(res.ok, res.err);

    const manifest = packageJsonOf(join(out, 'rigswig'));
    assert.equal(manifest.name, 'rigswig');
    assert.equal(manifest.version, '0.1.1');
    assert.deepEqual(Object.keys(manifest.optionalDependencies).sort(), [...PLATFORM_PACKAGES].sort());
    assert.ok(Object.values(manifest.optionalDependencies).every((v) => v === '0.1.1'));
    assert.deepEqual(manifest.bin, { rigswig: 'bin/rigswig.js' });
    assert.deepEqual(manifest.engines, { node: '>=18' });
    assert.equal(manifest.license, 'Apache-2.0');
    assert.ok(manifest.files.includes('bin') && manifest.files.includes('NOTICE'));

    const staged = join(out, 'rigswig', 'bin', 'rigswig.js');
    assert.ok(existsSync(staged), 'the shim binary is staged');
    assert.equal(readFileSync(staged, 'utf8'), readFileSync(SHIM, 'utf8'));
    assert.equal(readFileSync(join(out, 'rigswig', 'README.md'), 'utf8'), readFileSync(join(ROOT, 'README.md'), 'utf8'));
    assert.ok(existsSync(join(out, 'rigswig', 'LICENSE')));
    assert.ok(existsSync(join(out, 'rigswig', 'NOTICE')));
  });
});

test('buildFromReleases turns unpacked release tarballs into all five platform packages', () => {
  withTmp('rel', (tmp) => {
    const out = join(tmp, 'stage');
    const base = join(tmp, 'release');
    for (const triplet of Object.keys(build.TARGETS)) {
      fakeBinary(join(base, `rigswig-v9.9.9-${triplet}`, build.TARGETS[triplet].ext === '.exe' ? 'rigswig.exe' : 'rigswig'));
    }
    const res = runBuild(['--version', 'v9.9.9', '--releases', base, '--out', out]);
    assert.ok(res.ok, res.err);
    for (const name of PLATFORM_PACKAGES) {
      const dir = join(out, ...name.split('/'));
      assert.equal(packageJsonOf(dir).version, '9.9.9');
      assert.ok(existsSync(join(dir, 'package.json')));
    }
  });
});

test('every built package packs cleanly (npm pack --dry-run)', { skip: process.platform === 'win32' }, () => {
  withTmp('pack', (tmp) => {
    const out = join(tmp, 'stage');
    assert.ok(runBuild(['--version', 'v0.1.1', '--target', 'x86_64-unknown-linux-musl', '--binary', fakeBinary(join(tmp, 'rigswig')), '--out', out]).ok);
    assert.ok(runBuild(['--version', 'v0.1.1', '--picker', '--out', out]).ok);
    for (const dir of [join(out, SCOPE, 'cli-linux-x64'), join(out, 'rigswig')]) {
      const res = spawnSync('npm', ['pack', '--dry-run'], { cwd: dir, encoding: 'utf8' });
      assert.equal(res.status, 0, `npm pack failed in ${dir}:\n${res.stdout}\n${res.stderr}`);
    }
  });
});

// -- the shim ---------------------------------------------------------------

test('the shim and build.mjs agree on the platform -> package mapping', () => {
  assert.deepEqual(
    Object.entries(shim.PACKAGES).sort((a, b) => a[1].localeCompare(b[1])),
    Object.values(build.TARGETS)
      .map((t) => [t.package.slice('cli-'.length), `${SCOPE}/${t.package}`])
      .sort((a, b) => a[1].localeCompare(b[1])),
  );
});

test('unsupported platform: a clear error naming the supported builds', () => {
  const res = shim.resolveBinary('sunos-x64', () => '/nowhere/package.json');
  assert.ok(res.error.includes('sunos-x64'));
  assert.ok(res.error.includes('darwin-arm64'));
  assert.ok(!('path' in res));
});

test('optional dependency is missing: the error explains --omit=optional and points at the releases page', () => {
  const res = shim.resolveBinary('darwin-arm64', () => {
    const err = new Error(`Cannot find module '${SCOPE}/cli-darwin-arm64/package.json'`);
    err.code = 'MODULE_NOT_FOUND';
    throw err;
  });
  assert.ok(res.error.includes(`${SCOPE}/cli-darwin-arm64`));
  assert.match(res.error, /omit=optional/);
  assert.ok(res.error.includes('https://github.com/overfocus/rigswig/releases'));
});

test('package installed but binary gone: its own error', () => {
  withTmp('gone', (out) => {
    const dir = fakePackage(out, `${SCOPE}/cli-darwin-x64`, { withBinary: false });
    assert.ok(!existsSync(join(dir, 'bin', 'rigswig')), 'the fake package has no binary');
    const res = shim.resolveBinary('darwin-x64', fakeResolve(out));
    assert.ok(res.error.includes(`${SCOPE}/cli-darwin-x64`));
    assert.ok(res.error.includes('releases'));
  });
});

test('happy path: resolveBinary finds the binary by package-relative path', () => {
  withTmp('happy', (out) => {
    const dir = fakePackage(out, `${SCOPE}/cli-linux-x64`);
    const res = shim.resolveBinary('linux-x64', fakeResolve(out));
    assert.equal(res.path, join(dir, 'bin', 'rigswig'));
    assert.equal(res.package, `${SCOPE}/cli-linux-x64`);
  });
});

// End-to-end through a child process: the shim finds the fake binary in a real node_modules tree and
// forwards arguments and the exit status. The fakery needs a unix shell, so it is skipped on windows,
// where everything above still runs (the packaging side is fully covered there).
test('the shim runs the binary and forwards arguments, output and exit codes', { skip: process.platform === 'win32' }, () => {
  withTmp('e2e', (out) => {
    const platform = `${process.platform}-${process.arch}`;
    const pkg = shim.PACKAGES[platform];
    if (!pkg) return;
    // The tree sits one level down, so the "lonely" shim later in this test has no node_modules in
    // its lookup chain: require.resolve walks every parent, including this one otherwise.
    const base = join(out, 'tree');
    const tree = fakePackage(base, pkg);
    fakeBinary(join(tree, 'bin', 'rigswig'), '#!/bin/sh\ncase "$1" in fail) echo wrong >&2; exit 3 ;; *) echo "$@" ;; esac\n');

    const staged = join(base, 'node_modules', 'rigswig', 'bin', 'rigswig.js');
    mkdirSync(dirname(staged), { recursive: true });
    copyFileSync(SHIM, staged);
    const run = (args) => spawnSync(process.execPath, [staged, ...args], { encoding: 'utf8' });

    const forward = run(['hello', 'world']);
    assert.equal(forward.status, 0, `status ${forward.status}\n${forward.stderr}`);
    assert.equal(forward.stdout, 'hello world\n');

    const status = run(['fail']);
    assert.equal(status.status, 3);
    assert.equal(status.stderr, 'wrong\n');

    const lonely = join(out, 'lonely-shim', 'rigswig.js');
    mkdirSync(dirname(lonely), { recursive: true });
    copyFileSync(SHIM, lonely);
    const missing = spawnSync(process.execPath, [lonely, 'x'], { encoding: 'utf8' });
    assert.equal(missing.status, 1);
    assert.match(missing.stderr, /omit=optional/);
  });
});
