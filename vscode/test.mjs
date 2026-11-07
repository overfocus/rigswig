// The extension's logic without the editor: the status read, the URL line and the read-only invariants.
// Plain node, no dependencies (`node >= 18`); it runs the real binary when a build is present, so the parse
// contract is checked against the model, not only against this file.

'use strict';

import assert from 'node:assert';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
const { parseStatus, extractUiUrl, uiArgs, statusArgs } = createRequire(import.meta.url)('./lib.js');

const here = path.dirname(new URL(import.meta.url).pathname);

// A real `status --json` when the repo has a built binary; a fixture with the same shape otherwise.
let text;
let live = false;
for (const bin of ['../target/debug/rigswig', '../target/release/rigswig', '../target/debug/rigswig.exe']) {
    if (fs.existsSync(path.join(here, bin))) {
        const out = spawnSync(
            path.join(here, bin),
            statusArgs(path.join(here, '..')),
            { cwd: here, encoding: 'utf8' },
        );
        assert.equal(out.status, 0, 'the CLI status run');
        text = out.stdout;
        live = true;
        break;
    }
}
if (!live) {
    text = JSON.stringify({
        schema_version: '1.2.3',
        tool: { id: 'claude-code', adapterVersion: '9.9.9' },
        target: { repo: 'repo' },
        context: {
            chars: { value: 11125, confidence: 'exact' },
            tokens: { method: 'chars/4', value: 2782, low: 2225, high: 3709 },
            notMeasured: [{ what: 'x', why: 'y' }, { what: 'z', why: 'w' }],
        },
    });
}

// The status read parses and keeps the numbers the status bar shows.
const parsed = parseStatus(text);
assert.ok(['claude-code', 'codex', 'copilot', 'cursor', 'gemini', 'hf', 'hermes', 'ollama', 'devin', 'dsh', 'deepseek'].includes(parsed.tool), parsed.tool);
assert.ok(typeof parsed.chars === 'number' && parsed.chars >= 0, 'chars');
assert.equal(typeof parsed.tokens.method, 'string');
assert.ok(parsed.tokens.value >= parsed.tokens.low, `value below low: ${JSON.stringify(parsed.tokens)}`);
assert.ok(parsed.tokens.low <= parsed.tokens.high, 'low above high');
assert.ok(parsed.tokens.high >= parsed.tokens.value, 'high below value');

// The full non-editor flow: spawn the page, read its URL line, and fetch the page it serves.
if (live) {
    const root = path.join(here, '..');
    let binPath = null;
    for (const b of ['../target/debug/rigswig', '../target/release/rigswig', '../target/debug/rigswig.exe']) {
        if (fs.existsSync(path.join(here, b))) {
            binPath = path.join(here, b);
            break;
        }
    }
    const url = await new Promise((resolve, reject) => {
        const child = spawn(binPath, uiArgs(root), { cwd: here });
        let buffer = '';
        const timer = setTimeout(() => child.kill(), 8000);
        child.stdout.on('data', (d) => {
            buffer += d;
            const found = extractUiUrl(buffer);
            if (found) {
                clearTimeout(timer);
                resolve({ url: found, done: () => child.kill() });
            }
        });
        child.on('close', () => clearTimeout(timer));
        child.on('error', reject);
    });
    const res = await fetch(url.url);
    assert.ok(res.ok, `the page answers: ${res.status}`);
    const body = await res.text();
    assert.ok(body.includes('<'), 'the page is html');
    url.done();
}

// Garbage is named, not half-read.
assert.throws(() => parseStatus('not json at all'), TypeError);
assert.throws(() => parseStatus('{"tool":{}}'), TypeError);
assert.throws(() => parseStatus('{"tool":{"id":"x"},"context":{"chars":{"value":3}}}'), TypeError);

// The page URL line: the only stdout the extension needs, with any port.
assert.equal(
    extractUiUrl(`not this line\nrigswig ui (read-only, local only): http://127.0.0.1:51327/\nnoise after`),
    'http://127.0.0.1:51327/',
);
assert.equal(extractUiUrl('rigswig ui (WRITE MODE, local only): http://127.0.0.1:1/'), 'http://127.0.0.1:1/');
assert.equal(extractUiUrl('nothing here yet'), null);

// The invariants: the extension's page is the CLI's read-only page, always.
assert.deepEqual(
    uiArgs('/repo'),
    ['ui', '--repo', '/repo', '--port', '0', '--idle', '20'],
    'the page spawn',
);
assert.ok(uiArgs('/repo').every((a) => a !== '--allow-writes'), 'never write mode');
assert.ok(uiArgs('/repo').every((a) => a !== '--allow-github'), 'never github');
assert.deepEqual(statusArgs('/repo'), ['status', '--json', '--repo', '/repo']);

console.log(`vscode: ${live ? 'live binary' : 'fixture'}, all logic checks pass`);
