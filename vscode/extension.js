'use strict';

const vscode = require('vscode');
const { spawn } = require('node:child_process');
const { basename } = require('node:path');
const { parseStatus, parseAudit, extractUiUrl, uiArgs, statusArgs, auditArgs, pageHtml } = require('./lib');

/** The rigswig binary to spawn: the setting `rigswig.executablePath`, default `rigswig` from the PATH. */
function bin() {
    return vscode.workspace.getConfiguration('rigswig').get('executablePath', 'rigswig');
}

/** The spawn options: the repo's shell quirk on Windows, nothing everywhere else. */
function spawnOpts(cwd) {
    return { cwd, shell: process.platform === 'win32' };
}

function run(binPath, args, cwd) {
    return new Promise((resolve, reject) => {
        const child = spawn(binPath, args, spawnOpts(cwd));
        let stdout = '';
        let stderr = '';
        child.stdout.on('data', (d) => (stdout += d));
        child.stderr.on('data', (d) => (stderr += d));
        child.on('error', reject);
        child.on('close', (code) =>
            code === 0 ? resolve(stdout) : reject(new Error(stderr.trim() || `rigswig exited with ${code}`)),
        );
    });
}

/** Collect stdout until the predicate matches, the process exits, or the deadline passes. */
function until(binPath, args, cwd, match, deadlineMs) {
    return new Promise((resolve, reject) => {
        const child = spawn(binPath, args, spawnOpts(cwd));
        let buffer = '';
        let done = false;
        const finish = (fn, value) => {
            if (!done) {
                done = true;
                clearTimeout(timer);
                fn(value);
            }
        };
        const timer = setTimeout(() => finish(reject, new Error('the page never printed a URL')), deadlineMs);
        child.stdout.on('data', (d) => {
            buffer += d;
            const url = extractUiUrl(buffer);
            if (url) {
                finish(resolve, { child, url });
            }
        });
        child.stderr.on('data', (d) => (buffer = d + buffer));
        child.on('error', (e) => finish(reject, e));
        child.on('close', () => finish(reject, new Error(buffer.trim() || 'the page stopped before it started')));
    });
}

function activate(context) {
    const item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
    item.text = '$(beaker) rigswig';
    item.tooltip = 'What this repo loads for a coding-agent session, and what the audit finds. Click for the read-only page.';
    item.command = 'rigswig.openPage';
    context.subscriptions.push(item);

    // Every trigger re-reads the bar; only a refresh or a folder change also drops the cached findings, so
    // moving between files never re-runs the audit. A drop asked for survives a later editor move inside the
    // same debounce.
    let timer = null;
    let drop = false;
    const schedule = (dropFindings) => {
        drop ||= dropFindings;
        clearTimeout(timer);
        timer = setTimeout(() => {
            if (drop) {
                findings = { repo: null, value: null, error: null };
                drop = false;
            }
            read(item);
        }, 200);
    };
    const refresh = () => schedule(true);
    context.subscriptions.push(vscode.commands.registerCommand('rigswig.refresh', refresh));
    context.subscriptions.push(vscode.workspace.onDidChangeWorkspaceFolders(refresh));
    context.subscriptions.push(vscode.window.onDidChangeActiveTextEditor(() => schedule(false)));

    context.subscriptions.push(vscode.commands.registerCommand('rigswig.openPage', () => openPage(item)));
    context.subscriptions.push({ dispose: stopPage });
    context.subscriptions.push(vscode.workspace.onDidChangeWorkspaceFolders(stopPage));
    refresh();
}

function deactivate() {
    stopPage();
}

/** The workspace the editor is pointed at: the active editor's folder, else the first one. */
function repoOf() {
    const active = vscode.window.activeTextEditor?.document.uri;
    const folder = (active && vscode.workspace.getWorkspaceFolder(active)) || vscode.workspace.workspaceFolders?.[0];
    return folder?.uri.fsPath ?? null;
}

// The two reads behind the bar, shared across refreshes: the harness read of the current repo, and the
// audit's findings for it, so an active file moving does not re-run the audit — only a folder change or a
// refresh does.
let state = null; // { repo, parsed }
let findings = { repo: null, value: null, error: null };

async function read(item) {
    const repo = repoOf();
    if (!repo) {
        item.text = '$(beaker) rigswig';
        item.tooltip = new vscode.MarkdownString('Open a folder, then rigswig reads what its session loads.');
        return;
    }
    item.text = '$(beaker) rigswig…';
    item.tooltip = 'Reading…';
    try {
        const parsed = parseStatus(await run(bin(), statusArgs(repo), repo));
        state = { repo, parsed };
        if (findings.repo !== repo) {
            findings = { repo, value: null, error: null };
            void readAudit(item, findings);
        }
        render(item);
    } catch (e) {
        item.text = '$(beaker) rigswig?';
        item.tooltip = `rigswig could not read this folder: ${e.message}\n\nSet \`rigswig.executablePath\`, or install the CLI (\`cargo install rigswig\`).`;
    }
}

/**
 * The findings half of the read, away from the status bar's fast path. `pending` is the cache entry this run
 * fills: a result lands only while that exact entry is still the cache, so an audit a refresh superseded — even
 * one for the same repo — can never overwrite a newer one, whichever finishes last.
 */
async function readAudit(item, pending) {
    const { repo } = pending;
    try {
        const value = parseAudit(await run(bin(), auditArgs(repo), repo));
        if (findings === pending) {
            findings = { repo, value, error: null };
        }
    } catch (e) {
        if (findings === pending) {
            findings = { repo, value: null, error: e.message };
        }
    }
    if (state?.repo === repo) {
        render(item);
    }
}

const fmt = (n) => n.toLocaleString();

/** The status bar and its tooltip, from what has landed: the harness read first, then the findings. */
function render(item) {
    const { repo, parsed } = state;
    const t = parsed.tokens;
    const f = findings.repo === repo ? findings : { value: null, error: null };
    const tip = new vscode.MarkdownString(
        [
            `**${parsed.tool}** in \`${basename(repo)}\`:`,
            ``,
            `- ~${fmt(t.value)} tokens load before you type (${t.method}: \`${t.low.toLocaleString()}..${t.high.toLocaleString()}\` from ${fmt(parsed.chars)} characters)`,
            `- ${parsed.notMeasured} fixed costs the tool adds (system prompt, built-in tools)`,
        ].join('\n'),
    );
    const parts = [`$(beaker) ${parsed.tool}`, `~${fmt(t.value)} tok`];
    if (f.value) {
        const by = [f.value.high, f.value.medium, f.value.low]
            .map((n, i) => (n > 0 ? `${n} ${['high', 'medium', 'low'][i]}` : null))
            .filter(Boolean)
            .join(', ');
        const save =
            f.value.saveable > 0 ? ` — about ${fmt(f.value.saveable)} tokens a session the fixes could save` : '';
        tip.appendMarkdown(
            `\n\n- ${f.value.findings === 0 ? 'no findings' : `${f.value.findings} findings (${by})`}${save}; the read-only page shows each one.`,
        );
        parts.push(f.value.findings === 0 ? 'no findings' : `${f.value.findings} findings`);
        if (f.value.saveable > 0) {
            parts.push(`~${fmt(f.value.saveable)} saveable`);
        }
    } else if (f.error) {
        tip.appendMarkdown(`\n\n- findings could not be read: ${f.error} — **rigswig: Refresh** tries again.`);
    } else {
        tip.appendMarkdown(`\n\n- the findings read is running; **rigswig: Refresh** re-runs it with the numbers.`);
    }
    tip.appendMarkdown(
        `\n\nClick for the read-only page. **rigswig: Refresh** re-reads; **rigswig: Open the page** reopens this.`,
    );
    // Not trusted: the tooltip links to nothing, and it carries the CLI's stderr and folder names, which are
    // data — no command link in them may run.
    tip.isTrusted = false;
    item.tooltip = tip;
    item.text = parts.join(' · ');
}

let page = { child: null, url: null, panel: null, repo: null };

/** The existing page, in a webview. Started read-only, always. */
async function openPage(item) {
    const repo = repoOf();
    if (!repo) {
        void vscode.window.showInformationMessage('rigswig: open a folder first.');
        return;
    }
    if (page.child && page.repo === repo) {
        page.panel?.reveal();
        return;
    }
    stopPage();
    let start;
    try {
        start = await until(bin(), uiArgs(repo), repo, extractUiUrl, 15_000);
    } catch (e) {
        void vscode.window.showErrorMessage(`rigswig: the page could not start — ${e.message}`);
        return;
    }
    page = { child: start.child, url: start.url, panel: null, repo };
    start.child.on('exit', () => {
        if (page.child === start.child) {
            stopPage('the page stopped (idle); rerun **rigswig: Open the page** to start it again.');
        }
    });
    // The URL as the editor presents it: identity on a desktop, the forwarded port on a remote host.
    const shown = await vscode.env.asExternalUri(vscode.Uri.parse(start.url));
    if (vscode.workspace.getConfiguration('rigswig').get('openInBrowser', false)) {
        await vscode.env.openExternal(shown);
        return;
    }
    const panel = vscode.window.createWebviewPanel('rigswig', `rigswig — ${basename(repo)} (read-only)`, -1, {
        enableScripts: true,
        retainContextWhenHidden: false,
    });
    panel.webview.html = pageHtml(shown.toString());
    panel.onDidDispose(() => {
        if (page.panel === panel) {
            stopPage();
        }
    });
    page.panel = panel;
    item.command = 'rigswig.openPage';
    void item.show();
}

function stopPage(message) {
    if (page.child) {
        page.child.kill();
    }
    const panel = page.panel;
    page = { child: null, url: null, panel: null, repo: null };
    if (panel && message) {
        panel.webview.html = `<!doctype html><body style="font-family: sans-serif; padding: 2em;"><p>${message}</p></body>`;
    }
}

module.exports = { activate, deactivate };
