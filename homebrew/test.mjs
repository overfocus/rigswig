// Tests for the Homebrew formula generator: `node homebrew/test.mjs` (node >= 18, no dependencies).
// Covers homebrew/formula.mjs and its lockstep with the release workflow (.github/workflows/release.yml).
// Nothing here touches the network or Homebrew; the formula itself was checked with `brew install`, `brew test`,
// `brew audit --strict --online` and `brew style` against a published release.
import { strict as assert } from 'node:assert';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { TARGETS, formula, parseSums } from './formula.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const script = path.join(here, 'formula.mjs');
const targets = [TARGETS.macos.arm, TARGETS.macos.intel, TARGETS.linux.arm, TARGETS.linux.intel];

// An invented release: each file's checksum is its index repeated, so a mix-up between targets shows.
const sumsFor = (tag, names = targets.map((t) => `rigswig-${tag}-${t}.tar.gz`)) =>
  [...names, `rigswig-${tag}-x86_64-pc-windows-msvc.zip`]
    .map((n, i) => `${String(i).repeat(64)}  ${n}`)
    .join('\n') + '\n';

test('every target the formula installs is one the release workflow builds as a tarball', () => {
  const workflow = readFileSync(path.join(here, '..', '.github', 'workflows', 'release.yml'), 'utf8');
  for (const t of targets) {
    assert.match(workflow, new RegExp(`target: ${t}, `), `release.yml builds ${t}`);
  }
  assert.match(workflow, /name="rigswig-\$\{GITHUB_REF_NAME\}-\$\{\{ matrix.target \}\}"/, 'the asset name');
  assert.match(workflow, /tar czf "\$name.tar.gz" "\$name"/, 'a tarball with one top-level folder');
  assert.match(workflow, /cp "target\/\$\{\{ matrix.target \}\}\/release\/rigswig" "\$name\/"/, 'the binary in it');
  assert.match(workflow, /sha256sum \* > SHA256SUMS/, 'the checksums file');
});

test('each platform gets its own url and checksum, and nothing else from the sums', () => {
  const rb = formula('v1.2.3', parseSums(sumsFor('v1.2.3')));
  targets.forEach((t, i) => {
    const url = `https://github.com/overfocus/rigswig/releases/download/v1.2.3/rigswig-v1.2.3-${t}.tar.gz`;
    assert.ok(rb.includes(`url "${url}"\n      sha256 "${String(i).repeat(64)}"`), `${t} pairs with its own sum`);
  });
  assert.ok(!rb.includes('windows'), 'the Windows zip is not a Homebrew asset');
  assert.ok(!/^\s*version /m.test(rb), 'no version line: brew audit --strict calls it redundant');
  assert.match(rb, /^class Rigswig < Formula$/m);
  assert.match(rb, /bin\.install "rigswig"/);
});

test('sha256sum binary-mode lines (`*name`) parse too', () => {
  const sums = parseSums(`${'a'.repeat(64)} *rigswig-v1.0.0-${targets[0]}.tar.gz\n`);
  assert.equal(sums.get(`rigswig-v1.0.0-${targets[0]}.tar.gz`), 'a'.repeat(64));
});

test('a missing asset or a malformed tag is refused, never a formula with a hole', () => {
  assert.throws(() => formula('v1.2.3', parseSums(sumsFor('v1.2.3', targets.slice(1).map((t) => `rigswig-v1.2.3-${t}.tar.gz`)))), /no line for rigswig-v1\.2\.3-aarch64-apple-darwin/);
  assert.throws(() => formula('v1.2.4', parseSums(sumsFor('v1.2.3'))), /no line for/, 'sums from another release');
  for (const bad of ['1.2.3', 'v1.2', 'v1.2.3-rc1', undefined]) {
    assert.throws(() => formula(bad, parseSums(sumsFor('v1.2.3'))), /release tag/, String(bad));
  }
});

test('the command line writes the formula, and fails with a message and exit 2', () => {
  const dir = mkdtempSync(path.join(os.tmpdir(), 'rigswig-homebrew-'));
  try {
    const sums = path.join(dir, 'SHA256SUMS');
    writeFileSync(sums, sumsFor('v1.2.3'));
    const ok = spawnSync(process.execPath, [script, '--version', 'v1.2.3', '--sums', sums], { encoding: 'utf8' });
    assert.equal(ok.status, 0, ok.stderr);
    assert.equal(ok.stdout, formula('v1.2.3', parseSums(sumsFor('v1.2.3'))));
    const bad = spawnSync(process.execPath, [script, '--version', 'v9.9.9', '--sums', sums], { encoding: 'utf8' });
    assert.equal(bad.status, 2);
    assert.equal(bad.stdout, '', 'nothing half-written to stdout');
    assert.match(bad.stderr, /no line for rigswig-v9\.9\.9/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
