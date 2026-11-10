// The extension's logic without the editor: the status read, the URL line and the read-only invariants.
// Plain node, no dependencies (`node >= 18`); it runs the real binary when a build is present, so the parse
// contract is checked against the model, not only against this file.

'use strict';

import assert from 'node:assert';
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
const lib = createRequire(import.meta.url)('./lib.js');
const { parseStatus, parseAudit, extractUiUrl, uiArgs, statusArgs, auditArgs, pageHtml } = lib;

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
assert.ok(['claude-code', 'codex', 'copilot', 'cursor', 'gemini', 'hf', 'hermes', 'ollama', 'devin', 'dsh', 'deepseek', 'pi', 'omp'].includes(parsed.tool), parsed.tool);
assert.ok(typeof parsed.chars === 'number' && parsed.chars >= 0, 'chars');
assert.equal(typeof parsed.tokens.method, 'string');
assert.ok(parsed.tokens.value >= parsed.tokens.low, `value below low: ${JSON.stringify(parsed.tokens)}`);
assert.ok(parsed.tokens.low <= parsed.tokens.high, 'low above high');
assert.ok(parsed.tokens.high >= parsed.tokens.value, 'high below value');

// The live-session claim, when the run followed one: a valid shape, or null.
if (parsed.session) {
    assert.ok(parsed.session.scope === 'repo' || parsed.session.scope === 'machine', String(parsed.session.scope));
    assert.ok(parsed.session.evidence.length > 0, JSON.stringify(parsed.session));
}

// The session claim rides `tool.session` when the pick followed one; malformed and absent are both null.
const harnessWith = (tool) =>
    JSON.stringify({
        schema_version: '1.2.3',
        tool,
        target: { repo: 'repo' },
        context: { chars: { value: 40, confidence: 'exact' }, tokens: { method: 'chars/4', value: 10, low: 8, high: 12 } },
    });
const liveRead = parseStatus(
    harnessWith({ id: 'hermes', adapterVersion: '9.9.9', session: { scope: 'machine', evidence: ['Hermes home: state.db-wal'] } }),
);
assert.deepEqual(liveRead.session, { scope: 'machine', evidence: ['Hermes home: state.db-wal'] });
assert.deepEqual(
    parseStatus(harnessWith({ id: 'hermes', session: { scope: 'repo', evidence: ['projects/x: 1 session file'] } })).session,
    { scope: 'repo', evidence: ['projects/x: 1 session file'] },
);
assert.equal(parseStatus(harnessWith({ id: 'hermes', session: { scope: 'weird', evidence: [] } })).session, null);
assert.equal(parseStatus(harnessWith({ id: 'hermes', session: { scope: 'machine' } })).session, null);
assert.equal(parseStatus(harnessWith({ id: 'hermes', session: 'yes' })).session, null);
assert.equal(parseStatus(harnessWith({ id: 'hermes' })).session, null);

// The audit read parses, and a finding without a `saving` adds nothing to the saveable total.
const aud = parseAudit(
    JSON.stringify({
        tool: 'claude-code',
        repo: 'repo',
        tokens: 5,
        findings: [
            { code: 'a', severity: 'high', saving: { tokens: 700 } },
            { code: 'b', severity: 'high' },
            { code: 'c', severity: 'low' },
        ],
    }),
);
assert.equal(aud.tool, 'claude-code');
assert.equal(aud.findings, 3);
assert.equal(aud.high, 2);
assert.equal(aud.medium, 0);
assert.equal(aud.low, 1);
assert.equal(aud.saveable, 700, 'a finding without a saving does not count');
assert.throws(() => parseAudit('not json at all'), TypeError);
// Each check fails on its own: the status JSON's `tool` object is not an audit's, and a tool without a findings
// list is not either. The message pins which check fired.
assert.throws(() => parseAudit('{"tool":{"id":"x"},"findings":[]}'), /no tool/, 'a status-shaped tool is refused');
assert.throws(() => parseAudit('{"tool":"claude-code"}'), /no findings list/, 'a missing findings list is refused');
// The audit model's own names: a misnamed `saving.tokens` would sum to 0 against a repo with no savings, so pin
// it on a fixture shaped like the Rust model's serialized `Saving` (`tokens`, `without`).
assert.equal(
    parseAudit(
        JSON.stringify({ tool: 'claude-code', findings: [{ severity: 'medium', saving: { tokens: 42, without: ['x'] } }] }),
    ).saveable,
    42,
);

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

    // The findings read against the real audit on this repo: the same harness status and audit agree on.
    const auditOut = spawnSync(binPath, auditArgs(root, parsed.tool), { cwd: here, encoding: 'utf8' });
    assert.equal(auditOut.status, 0, `the CLI audit run: ${auditOut.stderr}`);
    const liveAudit = parseAudit(auditOut.stdout);
    assert.equal(liveAudit.tool, parsed.tool, 'the audit names the same harness status does');
    assert.ok(liveAudit.high + liveAudit.medium + liveAudit.low <= liveAudit.findings, 'severities within the count');
    assert.ok(liveAudit.saveable >= 0, 'saveable');
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
// The findings spawn is the audit, and nothing else: never session logs (`--usage` is opt-in per run, and an
// editor is not a run that opted in), and none of the write or download flags either.
assert.deepEqual(auditArgs('/repo', 'claude-code'), ['audit', '--json', '--repo', '/repo', '--tool', 'claude-code']);
for (const banned of ['--usage', '--allow-writes', '--allow-github', '--github', '--archive']) {
    assert.ok(auditArgs('/repo', 'claude-code').every((a) => a !== banned), `the extension's audit never passes ${banned}`);
}

// The panel toolbar: refresh and the page are editor-title buttons on the rigswig webview, each with an icon.
const manifest = JSON.parse(fs.readFileSync(path.join(here, 'package.json'), 'utf8'));
const title = manifest.contributes?.menus?.['editor/title'] ?? [];
assert.ok(
    title.some((m) => m.command === 'rigswig.refresh' && m.when === "activeWebviewPanelId == 'rigswig'"),
    'the panel toolbar has refresh',
);
assert.ok(title.some((m) => m.command === 'rigswig.openPage'), 'the panel toolbar opens the page');
for (const m of title) {
    const cmd = manifest.contributes.commands.find((c) => c.command === m.command);
    assert.ok(cmd?.icon, `${m.command} carries an icon for the toolbar`);
}

// The hosting document names the frame exactly, so editors that check the CSP against the full URL pass it.
const html = pageHtml('http://127.0.0.1:51327/');
assert.ok(html.includes('<iframe src="http://127.0.0.1:51327/">'), 'the frame src is the page URL');
assert.ok(html.includes('frame-src http://127.0.0.1:51327/ http://127.0.0.1:* '), 'the CSP names the frame exactly and the wildcard');
assert.ok(html.includes('http://localhost:*') && html.includes('http://[::1]:*'), "every localhost spelling is allowed");
assert.ok(html.includes("default-src 'none'"), 'the host document stays closed otherwise');


// ---------------------------------------------------------------------------------------------------------------
// The views' reads: items, findings with their files, and the groups the context view shows.

const statusFixture = JSON.stringify({
    tool: { id: 'claude-code' },
    context: { chars: { value: 4000 }, tokens: { method: 'chars/4', value: 1000, low: 800, high: 1333 } },
    layers: [{ id: 'user', path: '~/.claude/settings.json' }, { id: 'session' }],
    items: [
        { id: 'i:repo', kind: 'instruction', name: 'CLAUDE.md', origin: { layer: 'project', file: 'CLAUDE.md' }, effective: true, load: { when: 'startup', chars: { value: 3000 } } },
        { id: 'i:home', kind: 'instruction', name: '~/.claude/CLAUDE.md', origin: { layer: 'user', file: '~/.claude/CLAUDE.md' }, effective: true, load: { when: 'startup', chars: { value: 1000 } } },
        { id: 'a:off', kind: 'agent', name: 'old', origin: { layer: 'user', file: '~/.claude/agents/old.md', line: 3 }, effective: false, load: { when: 'startup', chars: { value: 500 } } },
        { id: 'h:1', kind: 'hook', name: 'PreToolUse', origin: { layer: 'user', file: '~/.claude/settings.json' }, effective: true, load: { when: 'never', chars: { value: 0 } } },
        { id: 'm:1', kind: 'mcp', name: 'srv', origin: { layer: 'user', file: '~/.claude.json' }, effective: true, load: { when: 'startup', chars: { value: 0 } } },
        { kind: 'agent' },
        null,
    ],
    warnings: [{ code: 'w', message: 'a warning' }, { code: 'bad' }],
});
const st = parseStatus(statusFixture);
assert.equal(st.items.length, 5, 'malformed items are skipped, not fatal');
assert.deepEqual(st.layerFiles, ['~/.claude/settings.json'], 'a layer without a path is no file');
assert.equal(st.warnings.length, 1);
assert.equal(st.items.find((i) => i.id === 'a:off').line, 3);
assert.equal(st.items.find((i) => i.id === 'a:off').effective, false);
assert.equal(lib.tokensFor(3000, st), 750, "an item's share uses the read's own ratio");
assert.equal(lib.tokensFor(10, { chars: 0, tokens: { value: 0 } }), 0, 'an empty read divides by nothing');

const groups = lib.contextGroups(st);
assert.deepEqual(groups.map((g) => g.kind).sort(), ['agent', 'hook', 'instruction', 'mcp'], 'one group per kind');
assert.equal(groups[0].kind, 'instruction', 'the costliest kind first');
assert.deepEqual(groups[0].items.map((i) => i.id), ['i:repo', 'i:home'], 'the largest item first');
const agents = groups.find((g) => g.kind === 'agent');
assert.equal(agents.items.length, 0, 'an item that does not load is not listed');
assert.equal(agents.notLoaded, 1, 'but it is counted');

// Files as the CLI prints them, made absolute: repo-relative, home, absolute, and labels that are no path.
const repo = path.resolve('/work/repo');
const home = path.resolve('/home/me');
assert.equal(lib.resolveFile('CLAUDE.md', repo, home), path.join(repo, 'CLAUDE.md'));
assert.equal(lib.resolveFile('~/.claude/CLAUDE.md', repo, home), path.join(home, '.claude', 'CLAUDE.md'));
assert.equal(lib.resolveFile(path.resolve('/etc/codex/x.toml'), repo, home), path.resolve('/etc/codex/x.toml'));
assert.equal(lib.resolveFile('', repo, home), null);
assert.equal(lib.resolveFile(null, repo, home), null);
assert.equal(lib.resolveFile('~/x', repo, null), null, 'no home, no home path');

// The audit's list: most severe first, the CLI's order kept within a severity, strings checked one by one.
const full = parseAudit(
    JSON.stringify({
        tool: 'claude-code',
        findings: [
            { code: 'l', severity: 'low', title: 'low one', evidence: ['e', 3], proposal: 'p', items: ['i:home'] },
            { code: 'h1', severity: 'high', title: 'high one', evidence: [], proposal: 'fix', items: ['i:repo', 'i:repo', 'nope'], saving: { tokens: 9 } },
            { code: 'm', severity: 'medium', title: 'medium one', evidence: [], proposal: 'q', file: 'AGENTS.md' },
            { code: 'h2', severity: 'high', title: 'high two', evidence: [], proposal: 'r' },
            { code: 'x', severity: 'weird' },
        ],
    }),
);
assert.deepEqual(full.list.map((f) => f.code), ['h1', 'h2', 'm', 'l', 'x']);
assert.deepEqual(full.list.find((f) => f.code === 'l').evidence, ['e'], 'a non-string evidence line is dropped');
assert.equal(full.list.find((f) => f.code === 'x').title, 'x', 'a finding without a title shows its code');
const targets = lib.findingTargets(full.list, st.items, repo, home);
assert.deepEqual(
    targets.map((t) => [t.finding.code, t.file]),
    [
        ['h1', path.join(repo, 'CLAUDE.md')],
        ['m', path.join(repo, 'AGENTS.md')],
        ['l', path.join(home, '.claude', 'CLAUDE.md')],
    ],
    "a finding points at its own file and its items' files, each once; an unknown item points nowhere",
);

// Watching: files outside the repo by folder, never the repo's own (the workspace glob has those), never the
// harness's own constantly rewritten state file, and capped.
const watch = lib.watchTargets(st, repo, home);
assert.deepEqual(watch, [
    { dir: path.join(home, '.claude'), names: ['CLAUDE.md', 'settings.json'] },
    { dir: path.join(home, '.claude', 'agents'), names: ['old.md'] },
]);
assert.ok(!watch.some((w) => w.names.includes('.claude.json')), '~/.claude.json is not watched');
assert.equal(lib.watchTargets(st, repo, home, 1).length, 1, 'the cap holds');
for (const name of ['CLAUDE.md', 'AGENTS.md', 'GEMINI.md', '.mcp.json', '.claude/**', '.codex/**', '.cursor/**', '.github/copilot-instructions.md']) {
    assert.ok(lib.WATCH_GLOB.includes(name), `the workspace glob watches ${name}`);
}
assert.ok(lib.WATCH_GLOB.startsWith('{') && lib.WATCH_GLOB.endsWith('}') && !lib.WATCH_GLOB.includes(' '), 'one brace glob');

// The version check.
assert.ok(lib.versionAtLeast('rigswig 0.2.1', '0.2.0'));
assert.ok(lib.versionAtLeast('rigswig 0.2.0', '0.2.0'));
assert.ok(lib.versionAtLeast('rigswig 1.0.0', '0.2.0'));
assert.ok(!lib.versionAtLeast('rigswig 0.1.9', '0.2.0'));
assert.ok(!lib.versionAtLeast('', '0.2.0'), 'an unreadable version is not new enough');
if (live) {
    for (const b of ['../target/debug/rigswig', '../target/release/rigswig']) {
        if (fs.existsSync(path.join(here, b))) {
            const v = spawnSync(path.join(here, b), lib.versionArgs(), { encoding: 'utf8' });
            assert.ok(lib.versionAtLeast(v.stdout), `the built CLI meets MIN_CLI: ${v.stdout}`);
            break;
        }
    }
}

// The spawn: no shell outside Windows; on Windows every part quoted, so a space or & in a path cannot split or run.
assert.deepEqual(lib.spawnPlan('rigswig', ['status', '--repo', '/a b'], 'darwin'), {
    command: 'rigswig',
    args: ['status', '--repo', '/a b'],
    shell: false,
});
const win = lib.spawnPlan('C:\\Program Files\\rigswig.exe', ['status', '--repo', 'C:\\a b&c'], 'win32');
assert.equal(win.shell, true);
assert.equal(win.command, '"C:\\Program Files\\rigswig.exe"');
assert.deepEqual(win.args, ['"status"', '"--repo"', '"C:\\a b&c"']);
assert.equal(lib.spawnPlan('x', ['a"b'], 'win32').args[0], '"a""b"', 'a quote inside is doubled');

// The stopped panel loads nothing and shows its message as text.
const stopped = lib.stoppedHtml('<script>x</script>');
assert.ok(!stopped.includes('<script>x'), 'the message is escaped');
assert.ok(stopped.includes("default-src 'none'"));

// The manifest: a workspace cannot pick the binary in Restricted Mode, the icon and views exist on disk, and every
// command a menu or view names is declared.
assert.ok(
    manifest.capabilities?.untrustedWorkspaces?.restrictedConfigurations?.includes('rigswig.executablePath'),
    'an untrusted workspace cannot choose the binary the extension runs',
);
assert.equal(manifest.capabilities?.virtualWorkspaces?.supported, false);
assert.ok(manifest.icon && fs.existsSync(path.join(here, manifest.icon)), 'the marketplace icon exists');
assert.ok(manifest.icon.endsWith('.png'), 'the marketplace takes a PNG icon, not an SVG');
const png = fs.readFileSync(path.join(here, manifest.icon));
assert.ok(png.readUInt32BE(16) >= 128 && png.readUInt32BE(20) >= 128, 'the icon is at least 128x128');
for (const c of manifest.contributes.viewsContainers.activitybar) {
    assert.ok(fs.existsSync(path.join(here, c.icon)), `the activity bar icon ${c.icon} exists`);
}
const declared = new Set(manifest.contributes.commands.map((c) => c.command));
for (const list of Object.values(manifest.contributes.menus)) {
    for (const m of list) {
        assert.ok(declared.has(m.command), `menu command ${m.command} is declared`);
    }
}
const extSource = fs.readFileSync(path.join(here, 'extension.js'), 'utf8');
for (const c of declared) {
    assert.ok(extSource.includes(`registerCommand('${c}'`), `${c} is registered`);
}
for (const v of manifest.contributes.views.rigswig) {
    assert.ok(extSource.includes(`createTreeView('${v.id}'`), `the ${v.id} view has a provider`);
}
// The extension spawns only through lib's argument builders: no flag literal of its own.
for (const banned of ['--allow-writes', '--allow-github', '--github', '--usage', '--archive']) {
    assert.ok(!extSource.includes(banned), `extension.js never names ${banned}`);
}
// The editor toolbar opens the page from any file a harness reads, and from no other file.
const onFiles = title.find((m) => m.command === 'rigswig.openPage' && m.when.includes('resourceFilename'));
assert.ok(onFiles, 'the editor toolbar has the page button on harness files');
const [nameRe, pathRe] = [...onFiles.when.matchAll(/=~ \/(.+?)\/(?= \|\||\))/g)].map((m) => new RegExp(m[1]));
const shows = (p) => nameRe.test(path.basename(p)) || pathRe.test(p);
for (const p of ['/r/CLAUDE.md', '/r/CLAUDE.local.md', '/r/sub/AGENTS.md', '/r/AGENTS.override.md', '/r/GEMINI.md',
    '/r/.mcp.json', '/r/.cursorrules', '/r/.github/copilot-instructions.md', '/h/.claude/settings.json',
    '/h/.claude/skills/x/SKILL.md', '/r/.codex/config.toml', 'C:\\r\\.cursor\\rules\\a.mdc']) {
    assert.ok(shows(p), `the page button shows on ${p}`);
}
for (const p of ['/r/README.md', '/r/src/main.rs', '/r/MYCLAUDE.md', '/r/CLAUDE.md.bak', '/r/package.json']) {
    assert.ok(!shows(p), `the page button stays off ${p}`);
}
assert.ok(!fs.readFileSync(path.join(here, '.vscodeignore'), 'utf8').split('\n').includes('media/**'), 'the icons ship');

console.log(`vscode: ${live ? 'live binary' : 'fixture'}, all logic checks pass`);
