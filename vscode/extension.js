'use strict';

const vscode = require('vscode');
const { spawn } = require('node:child_process');
const { basename } = require('node:path');
const { parseStatus, extractUiUrl, uiArgs, statusArgs, pageHtml } = require('./lib');

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
    item.tooltip = 'What this repo loads for a coding-agent session. Click for the read-only page.';
    item.command = 'rigswig.openPage';
    context.subscriptions.push(item);

    let timer = null;
    const refresh = () => {
        clearTimeout(timer);
        timer = setTimeout(() => read(item), 200);
    };
    context.subscriptions.push(vscode.commands.registerCommand('rigswig.refresh', refresh));
    context.subscriptions.push(vscode.workspace.onDidChangeWorkspaceFolders(refresh));
    context.subscriptions.push(vscode.window.onDidChangeActiveTextEditor(refresh));

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
        const t = parsed.tokens;
        item.text =
            `$(beaker) ~${t.value.toLocaleString()} tok ${t.low.toLocaleString()}..${t.high.toLocaleString()}`;
        const tip = new vscode.MarkdownString(
            [
                `**${parsed.tool}** in \`${basename(repo)}\`:`,
                ``,
                `- ${parsed.chars.toLocaleString()} characters of startup items`,
                `- estimated with \`${t.method}\` (\`~${t.value.toLocaleString()} tok\`, \`${t.low.toLocaleString()}..${t.high.toLocaleString()}\`)`,
                `- ${parsed.notMeasured} fixed costs the tool adds (system prompt, built-in tools)`,
                ``,
                `Click for the read-only page. **rigswig: Refresh** re-reads; **rigswig: Open the page** reopens this.`,
            ].join('\n'),
        );
        tip.isTrusted = true;
        item.tooltip = tip;
    } catch (e) {
        item.text = '$(beaker) rigswig?';
        item.tooltip = `rigswig could not read this folder: ${e.message}\n\nSet \`rigswig.executablePath\`, or install the CLI (\`cargo install rigswig\`).`;
    }
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
