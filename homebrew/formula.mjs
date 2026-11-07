#!/usr/bin/env node
// Write the Homebrew formula for one published rigswig release, from that release's SHA256SUMS. The formula
// installs the prebuilt binaries the release workflow (.github/workflows/release.yml) attaches: macOS on Apple
// Silicon and Intel, and the static-musl Linux builds on arm64 and x86_64. Zero dependencies, node >= 18.
// The formula lives in the tap (github.com/overfocus/homebrew-tap, `Formula/rigswig.rb`); docs/RELEASING.md says
// when to run this:
//
//   gh release download v0.2.0 -p SHA256SUMS -D /tmp/sums
//   node homebrew/formula.mjs --version v0.2.0 --sums /tmp/sums/SHA256SUMS > Formula/rigswig.rb
//
// No `version` line: Homebrew reads it from the URLs, and `brew audit --strict` calls one redundant. A missing
// line for any of the four tarballs is an error, never a formula with a hole in it.
import { readFileSync } from 'node:fs';

export const TARGETS = {
  macos: { arm: 'aarch64-apple-darwin', intel: 'x86_64-apple-darwin' },
  linux: { arm: 'aarch64-unknown-linux-musl', intel: 'x86_64-unknown-linux-musl' },
};

const REPO = 'https://github.com/overfocus/rigswig';

/** The checksum lines of a `sha256sum` output, by file name. */
export function parseSums(text) {
  const sums = new Map();
  for (const line of text.split('\n')) {
    const m = line.trim().match(/^([0-9a-f]{64}) [ *](\S+)$/);
    if (m) {
      sums.set(m[2], m[1]);
    }
  }
  return sums;
}

/** The formula's Ruby source for release `tag` (`vX.Y.Z`), given its checksums. */
export function formula(tag, sums) {
  if (!/^v\d+\.\d+\.\d+$/.test(tag ?? '')) {
    throw new Error(`--version must be a release tag like v0.2.0, not ${JSON.stringify(tag)}`);
  }
  const asset = (target) => {
    const name = `rigswig-${tag}-${target}.tar.gz`;
    const sha = sums.get(name);
    if (!sha) {
      throw new Error(`SHA256SUMS has no line for ${name}`);
    }
    return `      url "${REPO}/releases/download/${tag}/${name}"\n      sha256 "${sha}"`;
  };
  return `class Rigswig < Formula
  desc "Read-only explorer for what a coding-agent session loads"
  homepage "${REPO}"
  license "Apache-2.0"

  on_macos do
    on_arm do
${asset(TARGETS.macos.arm)}
    end
    on_intel do
${asset(TARGETS.macos.intel)}
    end
  end

  on_linux do
    on_arm do
${asset(TARGETS.linux.arm)}
    end
    on_intel do
${asset(TARGETS.linux.intel)}
    end
  end

  def install
    bin.install "rigswig"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rigswig --version")
    system bin/"rigswig", "--help"
  end
end
`;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    args[process.argv[i].replace(/^--/, '')] = process.argv[i + 1];
  }
  try {
    if (!args.sums) {
      throw new Error('--sums must name the release SHA256SUMS file');
    }
    process.stdout.write(formula(args.version, parseSums(readFileSync(args.sums, 'utf8'))));
  } catch (e) {
    process.stderr.write(`homebrew/formula.mjs: ${e.message}\n`);
    process.exit(2);
  }
}
