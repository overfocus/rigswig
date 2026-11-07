// The pieces of the extension that run outside the editor: the status JSON read, the page URL line and the
// spawn arguments. Kept plain CommonJS with no dependencies so `vscode/test.mjs` can require it under plain
// node (`node >= 18`), exactly like the CLI's own test battery.

'use strict';

/**
 * A `rigswig status --json` read, checked for the shape the status bar and the page need.
 * The harness JSON is a parse contract: tool, context.chars.value and context.tokens live in every version.
 *
 * @returns `{ tool, chars, tokens, notMeasured }`, where `tokens` is `{ method, value, low, high }`.
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
    return { tool: h.tool.id, chars, tokens, notMeasured };
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

module.exports = { parseStatus, extractUiUrl, uiArgs, statusArgs };
