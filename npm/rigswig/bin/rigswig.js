#!/usr/bin/env node
// The `rigswig` npm shim: finds the rigswig binary that npm installed as an optional dependency
// (@kftcda/cli-<os>-<arch>, laid out by npm/build.mjs) and runs it with the arguments it was given.
// The binary itself answers --version and everything else; this file only locates and forwards.
// The same shape as esbuild's and Biome's npm distribution. Published inside the npm packages -
// keep it small and dependency-free (engines: node >= 18).
'use strict';

const { spawnSync } = require('node:child_process');

// process.platform-process.arch -> the npm package that carries the binary for it. In lockstep with
// TARGETS in npm/build.mjs (npm/test.mjs checks the two against each other).
const PACKAGES = {
  'darwin-arm64': '@kftcda/cli-darwin-arm64',
  'darwin-x64': '@kftcda/cli-darwin-x64',
  'linux-arm64': '@kftcda/cli-linux-arm64',
  'linux-x64': '@kftcda/cli-linux-x64',
  'win32-x64': '@kftcda/cli-win32-x64',
};

// The binary's path inside its package. The platform packages deliberately declare no `bin` field
// (see npm/build.mjs): when sibling optional dependencies fail their os/cpu checks, npm's rollback
// deletes links by bin name, which would remove the picker's own link. So the shim locates the file
// directly instead of through a link or a manifest entry.

const RELEASES = 'https://github.com/overfocus/rigswig/releases';

function defaultResolve(id) {
  return require.resolve(id);
}

// Which package carries this platform, and where its binary file is. Returns { error } or
// { path, package }; `resolve` stands in for require.resolve so tests can inject a tree.
function resolveBinary(platformArch = `${process.platform}-${process.arch}`, resolve = defaultResolve) {
  const name = PACKAGES[platformArch];
  if (!name) {
    return {
      error: `rigswig has no build for ${platformArch}. Builds: ${Object.keys(PACKAGES).join(', ')}. ` +
        `Get a binary for your platform from ${RELEASES}.`,
    };
  }
  const file = `bin/rigswig${platformArch.startsWith('win32') ? '.exe' : ''}`;
  try {
    // The package and its binary file both fail require.resolve with the same code, so tell them
    // apart by resolving the manifest first.
    resolve(`${name}/package.json`);
  } catch (err) {
    return {
      error: `rigswig cannot find the binary package ${name} for your platform (${err.code || err}). ` +
        'If rigswig was installed with optional dependencies omitted (--omit=optional), reinstall without that, ' +
        `or get a binary from ${RELEASES}.`,
    };
  }
  let path;
  try {
    // A file path from require.resolve exists or throws, so this is the existence check too.
    path = resolve(`${name}/${file}`);
  } catch (_err) {
    return {
      error: `${name} is installed, but its binary (${file}) is missing. ` +
        `Reinstall it, or get a binary from ${RELEASES}.`,
    };
  }
  return { path, package: name };
}

if (require.main === module) {
  const found = resolveBinary();
  if (found.error) {
    console.error(`rigswig: ${found.error}`);
    process.exit(1);
  }
  const res = spawnSync(found.path, process.argv.slice(2), { stdio: 'inherit' });
  if (res.error) {
    console.error(`rigswig: failed to run ${found.path} (${res.error.code || res.error})`);
    process.exit(1);
  }
  if (res.signal) {
    // The common shell convention: 128 + the signal that stopped the child.
    const signals = { SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGTERM: 15 };
    process.exit(128 + (signals[res.signal] ?? 1));
  }
  process.exit(res.status ?? 1);
}

module.exports = { PACKAGES, resolveBinary };
