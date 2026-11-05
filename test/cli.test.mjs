import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const bin = new URL('../bin/rigswig.mjs', import.meta.url).pathname;
const run = (...args) => spawnSync(process.execPath, [bin, ...args], { encoding: 'utf8' });
const { version } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

test('--version prints the package version', () => {
  const r = run('--version');
  assert.equal(r.status, 0);
  assert.equal(r.stdout, `${version}\n`);
});

test('no command prints usage and exits 0', () => {
  const r = run();
  assert.equal(r.status, 0);
  assert.match(r.stdout, /rigswig status/);
});

test('an unknown command exits 2 with usage on stderr', () => {
  const r = run('nope');
  assert.equal(r.status, 2);
  assert.match(r.stderr, /unknown or unimplemented command: nope/);
});
