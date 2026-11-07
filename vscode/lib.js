// The pieces of the extension that run outside the editor: the status and audit JSON reads, the page URL line, the
// spawn arguments, the files a finding points at and the files worth watching. Kept plain CommonJS with no
// dependencies so `vscode/test.mjs` can require it under plain node (`node >= 18`), exactly like the CLI's own test
// battery.

'use strict';

const path = require('node:path');

/** The oldest CLI whose `status --json` and `audit --json` carry every field this extension reads. */
const MIN_CLI = '0.2.0';

const str = (v) => (typeof v === 'string' ? v : null);
const strs = (v) => (Array.isArray(v) ? v.filter((s) => typeof s === 'string') : []);

/**
 * A `rigswig status --json` read, checked for the shape the status bar and the views need.
 * The harness JSON is a parse contract: tool, context.chars.value and context.tokens live in every version; items,
 * layers and warnings are read when present and skipped entry by entry when malformed.
 *
 * @returns `{ tool, chars, tokens, notMeasured, items, layerFiles, warnings }`, where `tokens` is
 * `{ method, value, low, high }` and each item is `{ id, kind, name, layer, package, file, line, effective, when,
 * chars }`.
 */
function parseStatus(text) {
    let h;
    try {
        h = JSON.parse(text);
    } catch {
        throw new TypeError('not a rigswig harness JSON');
    }
    if (typeof h !== 'object' || h === null || !h.tool || typeof h.tool.id !== 'string') {
        throw new TypeError('not a rigswig harness JSON: no tool.id');
    }
    const chars = h.context?.chars?.value;
    const tokens = h.context?.tokens ?? null;
    if (typeof chars !== 'number' || chars < 0) {
        throw new TypeError('not a rigswig harness JSON: no context.chars.value');
    }
    if (
        typeof tokens !== 'object' ||
        tokens === null ||
        typeof tokens.method !== 'string' ||
        typeof tokens.value !== 'number' ||
        typeof tokens.low !== 'number' ||
        typeof tokens.high !== 'number'
    ) {
        throw new TypeError('not a rigswig harness JSON: no context.tokens estimate');
    }
    const notMeasured = Array.isArray(h.context?.notMeasured) ? h.context.notMeasured.length : 0;
    const items = (Array.isArray(h.items) ? h.items : [])
        .filter((i) => i && typeof i.id === 'string' && typeof i.kind === 'string')
        .map((i) => ({
            id: i.id,
            kind: i.kind,
            name: str(i.name) ?? i.id,
            layer: str(i.origin?.layer),
            package: str(i.origin?.package),
            file: str(i.origin?.file),
            line: Number.isInteger(i.origin?.line) && i.origin.line > 0 ? i.origin.line : null,
            effective: i.effective !== false,
            when: str(i.load?.when),
            chars: typeof i.load?.chars?.value === 'number' ? i.load.chars.value : 0,
        }));
    const layerFiles = (Array.isArray(h.layers) ? h.layers : []).map((l) => str(l?.path)).filter(Boolean);
    const warnings = (Array.isArray(h.warnings) ? h.warnings : [])
        .filter((w) => w && typeof w.message === 'string')
        .map((w) => ({ code: str(w.code) ?? '', message: w.message }));
    return { tool: h.tool.id, chars, tokens, notMeasured, items, layerFiles, warnings };
}

const SEVERITIES = ['high', 'medium', 'low'];

/**
 * A `rigswig audit --json` read, checked for the shape the findings read needs. The audit JSON is a parse
 * contract: `tool` is the harness id as a plain STRING (unlike `status --json`, whose `tool` is an object with
 * an `id`), `findings` is a list, and a finding carries `severity` and an optional `saving.tokens`.
 *
 * @returns `{ tool, findings, high, medium, low, saveable, list }`, where `saveable` sums every saving's tokens
 * and `list` holds each finding as `{ code, severity, title, proposal, evidence, items, file, saving }`, most severe
 * first (the CLI's order kept within a severity).
 */
function parseAudit(text) {
    let a;
    try {
        a = JSON.parse(text);
    } catch {
        throw new TypeError('not a rigswig audit JSON');
    }
    if (typeof a !== 'object' || a === null || typeof a.tool !== 'string' || a.tool.length === 0) {
        throw new TypeError('not a rigswig audit JSON: no tool');
    }
    if (!Array.isArray(a.findings)) {
        throw new TypeError('not a rigswig audit JSON: no findings list');
    }
    const count = (severity) => a.findings.filter((f) => f && f.severity === severity).length;
    const saving = (f) => (typeof f?.saving?.tokens === 'number' ? f.saving.tokens : 0);
    const rank = (s) => (SEVERITIES.includes(s) ? SEVERITIES.indexOf(s) : SEVERITIES.length);
    const list = a.findings
        .filter((f) => f && typeof f === 'object')
        .map((f) => ({
            code: str(f.code) ?? '',
            severity: SEVERITIES.includes(f.severity) ? f.severity : 'low',
            title: str(f.title) ?? str(f.code) ?? 'finding',
            proposal: str(f.proposal),
            evidence: strs(f.evidence),
            items: strs(f.items),
            file: str(f.file),
            saving: saving(f),
        }))
        .map((f, i) => ({ f, i }))
        .sort((x, y) => rank(x.f.severity) - rank(y.f.severity) || x.i - y.i)
        .map(({ f }) => f);
    return {
        tool: a.tool,
        findings: a.findings.length,
        high: count('high'),
        medium: count('medium'),
        low: count('low'),
        saveable: a.findings.reduce((n, f) => n + saving(f), 0),
        list,
    };
}

/**
 * A file as the CLI shows it (`~/...`, repo-relative, or absolute) as an absolute path, or null when it is not a
 * path at all (an empty string, or a label such as `command line and shell`).
 */
function resolveFile(file, repo, home) {
    if (typeof file !== 'string' || file.length === 0) {
        return null;
    }
    if (file === '~') {
        return home;
    }
    if (file.startsWith('~/') || file.startsWith('~\\')) {
        return home ? path.join(home, file.slice(2)) : null;
    }
    if (path.isAbsolute(file)) {
        return path.normalize(file);
    }
    return repo ? path.join(repo, file) : null;
}

/**
 * Where each finding points: its own file when it names one, then the files of the items it names, resolved, one
 * entry per finding and file.
 *
 * @returns `[{ finding, file, line }]`, `line` 1-based or null.
 */
function findingTargets(list, items, repo, home) {
    const byId = new Map(items.map((i) => [i.id, i]));
    const out = [];
    for (const finding of list) {
        const seen = new Set();
        const own = resolveFile(finding.file, repo, home);
        if (own) {
            seen.add(own);
            out.push({ finding, file: own, line: null });
        }
        for (const id of finding.items) {
            const item = byId.get(id);
            const file = item && resolveFile(item.file, repo, home);
            if (file && !seen.has(file)) {
                seen.add(file);
                out.push({ finding, file, line: item.line });
            }
        }
    }
    return out;
}

/** A share of the bill in tokens: the item's characters at the whole read's own ratio, so it uses its method. */
function tokensFor(chars, parsed) {
    if (!parsed.chars) {
        return 0;
    }
    return Math.round((chars * parsed.tokens.value) / parsed.chars);
}

/**
 * The context view's groups: the items that load, by kind, each kind's items largest first and the kinds by what
 * they cost. Items overridden or turned off are counted per kind as `notLoaded` and left out of the list.
 */
function contextGroups(parsed) {
    const kinds = new Map();
    for (const item of parsed.items) {
        const g = kinds.get(item.kind) ?? { kind: item.kind, chars: 0, items: [], notLoaded: 0 };
        kinds.set(item.kind, g);
        if (!item.effective) {
            g.notLoaded += 1;
            continue;
        }
        g.items.push(item);
        g.chars += item.chars;
    }
    const groups = [...kinds.values()].filter((g) => g.items.length > 0 || g.notLoaded > 0);
    for (const g of groups) {
        g.items.sort((a, b) => b.chars - a.chars || a.name.localeCompare(b.name));
    }
    return groups.sort((a, b) => b.chars - a.chars || a.kind.localeCompare(b.kind));
}

/**
 * The files a harness reads by name, so creating one in the workspace refreshes the bill even before the CLI has
 * reported it. The files it already reports are watched by their exact paths (see `watchTargets`).
 */
const WATCH_GLOB =
    '{**/CLAUDE.md,**/CLAUDE.local.md,**/AGENTS.md,**/AGENTS.local.md,**/AGENTS.override.md,**/AGENT.md,**/GEMINI.md,' +
    '**/HERMES.md,.hermes.md,.mcp.json,.cursorrules,.windsurfrules,.claude/**,.codex/**,.cursor/**,.gemini/**,' +
    '.windsurf/**,.devin/**,.zed/settings.json,.vscode/mcp.json,.github/copilot-instructions.md,' +
    '.github/instructions/**,.github/agents/**,.github/prompts/**,.github/mcp.json}';

/**
 * Files a harness itself rewrites all the time (Claude Code's `~/.claude.json` keeps per-session state next to the
 * MCP servers it lists): watching them would re-run the audit on every session, so the refresh command covers them.
 */
const NOISY = new Set(['.claude.json']);

/**
 * The folders to watch outside the workspace glob, each with the file names in it: every file the read reports
 * (items and settings layers) but the `NOISY` ones, grouped by folder and capped, so a home-scope change refreshes
 * the bill too.
 *
 * @returns `[{ dir, names }]`, at most `max` folders, the repo's own folders left to `WATCH_GLOB`.
 */
function watchTargets(parsed, repo, home, max = 48) {
    const dirs = new Map();
    const inRepo = (f) => repo && (f === repo || f.startsWith(repo.endsWith(path.sep) ? repo : repo + path.sep));
    const files = [...parsed.items.map((i) => i.file), ...parsed.layerFiles];
    for (const shown of files) {
        const file = resolveFile(shown, repo, home);
        if (!file || inRepo(file) || NOISY.has(path.basename(file))) {
            continue;
        }
        const dir = path.dirname(file);
        if (!dirs.has(dir)) {
            if (dirs.size >= max) {
                continue;
            }
            dirs.set(dir, new Set());
        }
        dirs.get(dir).add(path.basename(file));
    }
    return [...dirs].map(([dir, names]) => ({ dir, names: [...names].sort() }));
}

/** The version a `rigswig --version` run printed (`rigswig 0.2.1`), or null. */
function parseVersion(text) {
    const m = /(\d+)\.(\d+)\.(\d+)/.exec(String(text));
    return m ? m.slice(1, 4).map(Number) : null;
}

/** Whether a `rigswig --version` output is at least `min`; an unreadable one is not. */
function versionAtLeast(text, min = MIN_CLI) {
    const v = parseVersion(text);
    const m = parseVersion(min);
    if (!v || !m) {
        return false;
    }
    for (let i = 0; i < 3; i++) {
        if (v[i] !== m[i]) {
            return v[i] > m[i];
        }
    }
    return true;
}

/**
 * How to spawn the CLI on a platform. Everywhere but Windows: the binary and its arguments as they are, no shell.
 * On Windows a shell, because an npm install puts a `rigswig.cmd` shim on the PATH that only a shell runs, and so
 * every part double-quoted, or a repo path with a space or `&` in it would split or run.
 *
 * @returns `{ command, args, shell }`
 */
function spawnPlan(bin, args, platform = process.platform) {
    if (platform !== 'win32') {
        return { command: bin, args, shell: false };
    }
    const quote = (s) => `"${String(s).replace(/"/g, '""')}"`;
    return { command: quote(bin), args: args.map(quote), shell: true };
}

/**
 * The URL the page prints on stdout once it listens (`rigswig ui (read-only, local only): <url>`).
 *
 * @returns the first http URL on the rigswig ui line, or null when the process printed none yet.
 */
function extractUiUrl(stdout) {
    const line = stdout.split('\n').find((l) => l.includes('rigswig ui') && l.includes('http'));
    if (!line) {
        return null;
    }
    const match = /https?:\/\/\S+/.exec(line);
    return match ? match[0] : null;
}

/**
 * The arguments for `rigswig ui`: the same page, never the write-mode or GitHub flags — the extension's
 * invariants are the CLI's, and they are read-only by construction.
 */
function uiArgs(repo) {
    return ['ui', '--repo', repo, '--port', '0', '--idle', '20'];
}

/** The arguments for `rigswig status --json` on one repo. */
function statusArgs(repo) {
    return ['status', '--json', '--repo', repo];
}

/**
 * The arguments for `rigswig audit --json` on one repo: the findings read, and nothing else. Never `--usage`
 * (session logs are opt-in per run, and a click in an editor is not that), never any write or download flag.
 */
function auditArgs(repo) {
    return ['audit', '--json', '--repo', repo];
}

/** The arguments for the version check. */
function versionArgs() {
    return ['--version'];
}

const escapeHtml = (s) =>
    String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/**
 * The webview document that hosts the page: the frame gets the exact URL, plus localhost's spellings, because
 * editors refuse frames they do not find named in the CSP.
 */
function pageHtml(url) {
    return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy"
      content="default-src 'none'; frame-src ${url} http://127.0.0.1:* http://localhost:* http://[::1]:*; style-src 'unsafe-inline';">
<style> html, body, iframe { inset: 0; margin: 0; padding: 0; width: 100%; height: 100vh; border: 0; position: fixed; } </style>
</head>
<body>
<iframe src="${url}"></iframe>
</body>
</html>`;
}

/** The panel's document once the page has stopped: a message, escaped, in a document that loads nothing. */
function stoppedHtml(message) {
    return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
</head>
<body style="font-family: var(--vscode-font-family, sans-serif); color: var(--vscode-foreground); padding: 2em;">
<p>${escapeHtml(message)}</p>
</body>
</html>`;
}

module.exports = {
    MIN_CLI,
    SEVERITIES,
    WATCH_GLOB,
    parseStatus,
    parseAudit,
    resolveFile,
    findingTargets,
    tokensFor,
    contextGroups,
    watchTargets,
    parseVersion,
    versionAtLeast,
    spawnPlan,
    extractUiUrl,
    uiArgs,
    statusArgs,
    auditArgs,
    versionArgs,
    pageHtml,
    stoppedHtml,
};
