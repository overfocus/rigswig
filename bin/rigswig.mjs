#!/usr/bin/env node
// rigswig: a harness-agnostic, read-only explorer for coding-agent setups. Nothing is implemented yet (docs/ROADMAP.md).
import { readFileSync } from 'node:fs';

const { version } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const USAGE = `rigswig ${version}

Not implemented yet. Planned:
  rigswig status [--repo <path>] [--json]   what a session in a repo loads, and where each piece came from
  rigswig ui [--repo <path>]                the same as a local, read-only page
  rigswig compare <a> <b>                   two repos, or two harnesses on one repo

rigswig never makes a network request, calls a model, or writes a file.
`;

const [cmd] = process.argv.slice(2);
if (cmd === '--version' || cmd === '-v') {
  process.stdout.write(`${version}\n`);
} else if (cmd === undefined || cmd === '--help' || cmd === '-h') {
  process.stdout.write(USAGE);
} else {
  process.stderr.write(`rigswig: unknown or unimplemented command: ${cmd}\n\n${USAGE}`);
  process.exit(2);
}
