'use strict';

const vscode = require('vscode');
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const { basename, sep } = require('node:path');
const lib = require('./lib');

const STATUS_TIMEOUT_MS = 30_000;
const AUDIT_TIMEOUT_MS = 120_000;
const PAGE_START_MS = 15_000;
const INSTALL_URL = 'https://github.com/overfocus/rigswig/blob/main/docs/GETTING-STARTED.md#install';

/** @type {vscode.LogOutputChannel} */
let log;

const config = () => vscode.workspace.getConfiguration('rigswig');

/**
 * The rigswig binary to spawn: `rigswig.executablePath`, default `rigswig` from the PATH. In an untrusted
 * workspace the editor ignores a workspace's own value for it (`restrictedConfigurations` in package.json), so a
 * cloned repo cannot point the extension at a script of its own.
 */
const bin = () => config().get('executablePath', 'rigswig') || 'rigswig';

/** A spawn of the CLI, with the platform's quoting (lib.spawnPlan) and every run in the output channel. */
function start(args, cwd) {
    const plan = lib.spawnPlan(bin(), args, process.platform);
    log.info(`run: ${bin()} ${args.join(' ')}`);
    return spawn(plan.command, plan.args, { cwd, shell: plan.shell, windowsHide: true });
}

/** A missing binary, from either the spawn's ENOENT or a Windows shell's "not recognized". */
function isMissing(e, stderr = '') {
    return e?.code === 'ENOENT' || /is not recognized as an internal or external command/i.test(stderr);
}

class CliError extends Error {
    constructor(message, missing = false) {
        super(message);
        this.missing = missing;
    }
}

/** Run to completion and resolve with stdout; reject on a non-zero exit, a missing binary or the deadline. */
function run(args, cwd, timeoutMs) {
    return new Promise((resolve, reject) => {
        const t0 = Date.now();
        let child;
        try {
            child = start(args, cwd);
        } catch (e) {
            reject(new CliError(e.message, isMissing(e)));
            return;
        }
        let stdout = '';
        let stderr = '';
        let done = false;
        const finish = (fn, value) => {
            if (!done) {
                done = true;
                clearTimeout(timer);
                fn(value);
            }
        };
        const timer = setTimeout(() => {
            child.kill();
            log.warn(`timed out after ${timeoutMs / 1000}s: ${args[0]}`);
            finish(reject, new CliError(`rigswig ${args[0]} took longer than ${timeoutMs / 1000}s`));
        }, timeoutMs);
        child.stdout.on('data', (d) => (stdout += d));
        child.stderr.on('data', (d) => (stderr += d));
        child.on('error', (e) => {
            log.error(`spawn failed: ${e.message}`);
            finish(reject, new CliError(e.message, isMissing(e)));
        });
        child.on('close', (code) => {
            log.info(`${args[0]}: exit ${code} in ${Date.now() - t0} ms`);
            if (stderr.trim()) {
                log.info(stderr.trim());
            }
            if (code === 0) {
                finish(resolve, stdout);
            } else {
                finish(reject, new CliError(stderr.trim() || `rigswig exited with ${code}`, isMissing(null, stderr)));
            }
        });
    });
}

/** Start the page and resolve once its URL line is on stdout; reject if it exits first or never prints one. */
function startPage(repo) {
    return new Promise((resolve, reject) => {
        let child;
        try {
            child = start(lib.uiArgs(repo), repo);
        } catch (e) {
            reject(new CliError(e.message, isMissing(e)));
            return;
        }
        let stdout = '';
        let stderr = '';
        let done = false;
        const finish = (fn, value) => {
            if (!done) {
                done = true;
                clearTimeout(timer);
                fn(value);
            }
        };
        const timer = setTimeout(() => {
            child.kill();
            finish(reject, new CliError('the page never printed a URL'));
        }, PAGE_START_MS);
        child.stdout.on('data', (d) => {
            stdout += d;
            const url = lib.extractUiUrl(stdout);
            if (url) {
                log.info(`page: ${url}`);
                finish(resolve, { child, url });
            }
        });
        child.stderr.on('data', (d) => (stderr += d));
        child.on('error', (e) => finish(reject, new CliError(e.message, isMissing(e))));
        child.on('close', () =>
            finish(reject, new CliError(stderr.trim() || 'the page stopped before it started', isMissing(null, stderr))),
        );
    });
}

// ---------------------------------------------------------------------------------------------------------------
// State: the repo in view, its status read, and the audit's findings for it. A status read carries a sequence
// number so an older read that finishes last never overwrites a newer one; the findings cache keeps its own
// identity check (see readAudit).

let state = null; // { repo, parsed }
let findings = { repo: null, tool: null, value: null, error: null };
let lastError = null; // { message, missing }
let readSeq = 0;
let warnedMissing = false;
let checkedVersion = null; // the bin path whose version has been checked

/** The workspace the editor is pointed at: the active editor's folder, else the first one. */
function repoOf() {
    const active = vscode.window.activeTextEditor?.document.uri;
    const folder =
        (active && active.scheme === 'file' && vscode.workspace.getWorkspaceFolder(active)) ||
        vscode.workspace.workspaceFolders?.find((f) => f.uri.scheme === 'file');
    return folder?.uri.fsPath ?? null;
}

/** @type {Set<() => void>} */
const listeners = new Set();
const changed = () => listeners.forEach((fn) => fn());

async function read() {
    const repo = repoOf();
    const seq = ++readSeq;
    if (!repo) {
        state = null;
        lastError = null;
        changed();
        return;
    }
    status.busy();
    try {
        void checkVersion();
        const parsed = lib.parseStatus(await run(lib.statusArgs(repo), repo, STATUS_TIMEOUT_MS));
        if (seq !== readSeq) {
            return;
        }
        state = { repo, parsed };
        lastError = null;
        if (findings.repo !== repo || findings.tool !== parsed.tool) {
            findings = { repo, tool: parsed.tool, value: null, error: null };
            void readAudit(findings);
        }
    } catch (e) {
        if (seq !== readSeq) {
            return;
        }
        state = null;
        lastError = { message: e.message, missing: !!e.missing };
        log.error(`status: ${e.message}`);
        if (e.missing) {
            void offerSetup();
        }
    }
    changed();
}

/**
 * The findings half of the read, away from the status bar's fast path. `pending` is the cache entry this run
 * fills: a result lands only while that exact entry is still the cache, so an audit a refresh superseded — even
 * one for the same repo — can never overwrite a newer one, whichever finishes last.
 */
async function readAudit(pending) {
    const { repo, tool } = pending;
    try {
        const value = lib.parseAudit(await run(lib.auditArgs(repo, tool), repo, AUDIT_TIMEOUT_MS));
        if (findings === pending) {
            findings = { repo, tool, value, error: null };
        }
    } catch (e) {
        log.error(`audit: ${e.message}`);
        if (findings === pending) {
            findings = { repo, tool, value: null, error: e.message };
        }
    }
    if (state?.repo === repo) {
        changed();
    }
}

/** Warn once per binary when it is older than the JSON this extension reads. */
async function checkVersion() {
    const b = bin();
    if (checkedVersion === b) {
        return;
    }
    checkedVersion = b;
    try {
        const out = await run(lib.versionArgs(), undefined, STATUS_TIMEOUT_MS);
        log.info(`cli: ${out.trim()}`);
        if (!lib.versionAtLeast(out)) {
            const pick = await vscode.window.showWarningMessage(
                `rigswig: the CLI is ${out.trim() || 'of an unknown version'}; this extension needs ${lib.MIN_CLI} or newer.`,
                'How to update',
            );
            if (pick) {
                void vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
            }
        }
    } catch {
        checkedVersion = null; // the status read reports a missing binary; try again next time
    }
}

/** Once per session: the CLI is missing, so say how to get it or where to point the extension. */
async function offerSetup() {
    if (warnedMissing) {
        return;
    }
    warnedMissing = true;
    const pick = await vscode.window.showWarningMessage(
        `rigswig: the CLI was not found (\`${bin()}\`). Install it, or set rigswig.executablePath to the binary.`,
        'Install',
        'Set path',
    );
    if (pick === 'Install') {
        void vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
    } else if (pick === 'Set path') {
        void vscode.commands.executeCommand('workbench.action.openSettings', 'rigswig.executablePath');
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The status bar.

const fmt = (n) => n.toLocaleString();
const short = (n) => (n >= 10_000 ? `${(n / 1000).toFixed(0)}k` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : `${n}`);

const status = {
    item: null,
    init(context) {
        this.item = vscode.window.createStatusBarItem('rigswig.bill', vscode.StatusBarAlignment.Right, 100);
        this.item.name = 'rigswig';
        this.item.command = 'rigswig.openPage';
        context.subscriptions.push(this.item);
        listeners.add(() => this.render());
        this.render();
    },
    busy() {
        if (!state && !lastError) {
            this.item.text = '$(loading~spin) rigswig';
            this.item.tooltip = 'rigswig is reading what this folder loads…';
        }
    },
    render() {
        const item = this.item;
        item.backgroundColor = undefined;
        if (!config().get('statusBar', true)) {
            item.hide();
            return;
        }
        item.show();
        if (!repoOf()) {
            item.text = '$(beaker) rigswig';
            item.tooltip = 'Open a folder, then rigswig reads what its coding-agent session loads.';
            item.command = undefined;
            return;
        }
        if (!state) {
            if (!lastError) {
                item.text = '$(loading~spin) rigswig';
                item.tooltip = 'rigswig is reading what this folder loads…';
                return;
            }
            item.text = lastError.missing ? '$(beaker) rigswig: set up' : '$(beaker) rigswig $(error)';
            item.tooltip = lastError.missing
                ? `The rigswig CLI was not found (\`${bin()}\`). Click to install it or set its path.`
                : `rigswig could not read this folder: ${lastError.message}\n\nClick for the output log.`;
            item.command = lastError.missing ? 'rigswig.setup' : 'rigswig.showOutput';
            item.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
            return;
        }
        item.command = 'rigswig.openPage';
        const { repo, parsed } = state;
        const t = parsed.tokens;
        const f = findings.repo === repo ? findings : { value: null, error: null };
        // Not trusted, and every third-party string goes in as text: the tooltip carries the CLI's stderr, folder
        // and harness names, which are data — no command link or markup in them may run.
        const tip = new vscode.MarkdownString();
        tip.isTrusted = false;
        tip.supportThemeIcons = false;
        tip.appendMarkdown('**').appendText(parsed.tool).appendMarkdown('** in `').appendText(basename(repo));
        tip.appendMarkdown('`:\n\n');
        tip.appendText(
            `- ~${fmt(t.value)} estimated tokens from startup files (${t.method}: ${fmt(t.low)}..${fmt(t.high)} from ${fmt(parsed.chars)} characters)\n`,
        );
        if (parsed.session) {
            tip.appendText(`- picked for a live session (${parsed.session.scope}): ${parsed.session.evidence.join(', ')}\n`);
        }
        tip.appendText(`- ${parsed.notMeasured} fixed costs the tool adds that no file shows (system prompt, built-in tools)\n`);
        const live = parsed.session ? ' (session)' : '';
        const parts = [`$(beaker) ${parsed.tool}${live}`, `~${short(t.value)} tok`];
        if (f.value) {
            const by = lib.SEVERITIES.map((s) => (f.value[s] > 0 ? `${f.value[s]} ${s}` : null))
                .filter(Boolean)
                .join(', ');
            const save = f.value.saveable > 0 ? `; about ${fmt(f.value.saveable)} tokens a session the fixes could save` : '';
            tip.appendText(`- ${f.value.findings === 0 ? 'no findings' : `${f.value.findings} findings (${by})`}${save}\n`);
            if (f.value.findings > 0) {
                parts.push(`${f.value.high > 0 ? '$(warning) ' : ''}${f.value.findings} findings`);
            }
            if (f.value.saveable > 0) {
                parts.push(`~${short(f.value.saveable)} saveable`);
            }
        } else if (f.error) {
            tip.appendText(`- findings could not be read: ${f.error}\n`);
        } else {
            tip.appendText('- the audit is running…\n');
        }
        tip.appendMarkdown('\n---\n\nClick for the read-only page; the rigswig view in the activity bar lists each finding.');
        item.tooltip = tip;
        item.text = parts.join(' · ');
    },
};

// ---------------------------------------------------------------------------------------------------------------
// The views: findings, and what loads by kind.

const SEVERITY_ICON = { high: 'error', medium: 'warning', low: 'info' };
const SEVERITY_COLOR = {
    high: 'problemsErrorIcon.foreground',
    medium: 'problemsWarningIcon.foreground',
    low: 'problemsInfoIcon.foreground',
};
const KIND_ICON = {
    instruction: 'book',
    agent: 'hubot',
    skill: 'tools',
    command: 'terminal',
    mcp: 'plug',
    hook: 'zap',
    permission: 'shield',
    plugin: 'extensions',
};

const home = () => os.homedir();

/** A path as a person reads it: workspace-relative inside a folder, `~/...` under home, absolute otherwise. */
function displayPath(file) {
    const rel = vscode.workspace.asRelativePath(file, false);
    if (rel !== file) {
        return rel;
    }
    const h = home();
    return h && file.startsWith(h + sep) ? `~${file.slice(h.length)}` : file;
}

/** Whether a file sits inside one of the open workspace folders. */
const inWorkspace = (file) => !!vscode.workspace.getWorkspaceFolder(vscode.Uri.file(file));

/** A tree node that opens a file at a line when it exists on disk; otherwise null. */
function openCommand(file, line) {
    if (!file || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
        return null;
    }
    return { command: 'rigswig.openFile', title: 'Open', arguments: [file, line ?? null] };
}

function message(view) {
    if (!repoOf()) {
        return 'Open a folder to see what its coding-agent session loads.';
    }
    if (lastError) {
        return lastError.missing
            ? 'The rigswig CLI was not found: run "rigswig: Set up the CLI".'
            : `rigswig could not read this folder: ${lastError.message.split('\n')[0]}`;
    }
    if (!state) {
        return 'Reading…';
    }
    if (view === 'findings') {
        const f = findings.repo === state.repo ? findings : null;
        if (!f || (!f.value && !f.error)) {
            return 'The audit is running…';
        }
        if (f.error) {
            return `The findings could not be read: ${f.error.split('\n')[0]}`;
        }
        return f.value.findings === 0 ? `No findings for ${state.parsed.tool} here.` : undefined;
    }
    return state.parsed.items.length === 0 ? `${state.parsed.tool} loads nothing from files here.` : undefined;
}

class FindingsTree {
    constructor() {
        this._emitter = new vscode.EventEmitter();
        this.onDidChangeTreeData = this._emitter.event;
    }
    refresh() {
        this._emitter.fire();
    }
    getTreeItem(node) {
        return node.tree;
    }
    getChildren(node) {
        if (node) {
            return node.children ?? [];
        }
        const value = state && findings.repo === state.repo ? findings.value : null;
        if (!value) {
            return [];
        }
        const targets = lib.findingTargets(value.list, state.parsed.items, state.repo, home());
        return value.list.map((f) => {
            const files = targets.filter((t) => t.finding === f);
            const children = [];
            if (f.proposal) {
                children.push(leaf(`Fix: ${f.proposal}`, 'lightbulb', f.proposal));
            }
            for (const t of files) {
                const shown = displayPath(t.file);
                const n = leaf(shown, 'go-to-file', t.file);
                n.tree.command = openCommand(t.file, t.line) ?? undefined;
                n.tree.resourceUri = vscode.Uri.file(t.file);
                children.push(n);
            }
            for (const e of f.evidence) {
                children.push(leaf(e, 'circle-small', e));
            }
            const tree = new vscode.TreeItem(
                f.title,
                children.length ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
            );
            tree.iconPath = new vscode.ThemeIcon(SEVERITY_ICON[f.severity], new vscode.ThemeColor(SEVERITY_COLOR[f.severity]));
            tree.description = [f.severity, f.saving > 0 ? `~${fmt(f.saving)} tok` : null].filter(Boolean).join(' · ');
            const tip = new vscode.MarkdownString();
            tip.isTrusted = false;
            tip.appendText(f.title);
            tip.appendMarkdown('\n\n');
            tip.appendText(`${f.severity}${f.code ? ` · ${f.code}` : ''}${f.saving > 0 ? ` · saves ~${fmt(f.saving)} tokens a session` : ''}`);
            if (f.proposal) {
                tip.appendMarkdown('\n\n');
                tip.appendText(f.proposal);
            }
            tree.tooltip = tip;
            tree.contextValue = 'finding';
            const first = files[0];
            if (first) {
                tree.command = openCommand(first.file, first.line) ?? undefined;
            }
            return { tree, children, finding: f };
        });
    }
}

function leaf(label, icon, tooltip) {
    const tree = new vscode.TreeItem(label, vscode.TreeItemCollapsibleState.None);
    tree.iconPath = new vscode.ThemeIcon(icon);
    tree.tooltip = tooltip;
    return { tree };
}

class ContextTree {
    constructor() {
        this._emitter = new vscode.EventEmitter();
        this.onDidChangeTreeData = this._emitter.event;
    }
    refresh() {
        this._emitter.fire();
    }
    getTreeItem(node) {
        return node.tree;
    }
    getChildren(node) {
        if (node) {
            return node.children ?? [];
        }
        if (!state) {
            return [];
        }
        const { parsed, repo } = state;
        return lib.contextGroups(parsed).map((g) => {
            const children = g.items.map((i) => {
                const file = lib.resolveFile(i.file, repo, home());
                const tree = new vscode.TreeItem(i.name, vscode.TreeItemCollapsibleState.None);
                tree.description = [lib.itemCost(i, parsed), i.package ?? i.layer]
                    .filter(Boolean)
                    .join(' · ');
                tree.iconPath = new vscode.ThemeIcon(KIND_ICON[i.kind] ?? 'symbol-misc');
                tree.tooltip = [i.name, i.file, i.when ? `loads: ${i.when}` : null, `${fmt(i.chars)} characters`]
                    .filter(Boolean)
                    .join('\n');
                tree.command = openCommand(file, i.line) ?? undefined;
                if (file) {
                    tree.resourceUri = vscode.Uri.file(file);
                }
                tree.contextValue = 'item';
                return { tree };
            });
            const tree = new vscode.TreeItem(g.kind, vscode.TreeItemCollapsibleState.Collapsed);
            const off = g.notLoaded > 0 ? `${g.notLoaded} not loaded` : '';
            const costs = [
                g.startupChars > 0 ? `~${fmt(lib.tokensFor(g.startupChars, parsed))} tok startup` : null,
                g.deferredChars > 0 ? `~${fmt(lib.tokensFor(g.deferredChars, parsed))} tok deferred` : null,
                g.unknownChars > 0 ? `~${fmt(lib.tokensFor(g.unknownChars, parsed))} tok timing unknown` : null,
            ].filter(Boolean);
            tree.description = [`${g.items.length}`, ...costs, off].filter(Boolean).join(' · ');
            tree.iconPath = new vscode.ThemeIcon(KIND_ICON[g.kind] ?? 'symbol-misc');
            return { tree, children };
        });
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Problems: each finding on the files it names: the workspace's own files by default (`rigswig.problems`), since a
// finding on a plugin's cached file is one the reader cannot fix there.

const SEVERITY_DIAG = {
    high: vscode.DiagnosticSeverity.Warning,
    medium: vscode.DiagnosticSeverity.Warning,
    low: vscode.DiagnosticSeverity.Information,
};

function renderProblems(collection) {
    collection.clear();
    const scope = config().get('problems', 'workspace');
    if (scope === 'off' || !state || findings.repo !== state.repo || !findings.value) {
        return;
    }
    const byFile = new Map();
    for (const t of lib.findingTargets(findings.value.list, state.parsed.items, state.repo, home())) {
        if (scope !== 'all' && !inWorkspace(t.file)) {
            continue;
        }
        const line = Math.max(0, (t.line ?? 1) - 1);
        const msg = t.finding.proposal ? `${t.finding.title}. ${t.finding.proposal}` : t.finding.title;
        const d = new vscode.Diagnostic(new vscode.Range(line, 0, line, 0), msg, SEVERITY_DIAG[t.finding.severity]);
        d.source = 'rigswig';
        if (t.finding.code) {
            d.code = t.finding.code;
        }
        const list = byFile.get(t.file) ?? [];
        list.push(d);
        byFile.set(t.file, list);
    }
    for (const [file, list] of byFile) {
        collection.set(vscode.Uri.file(file), list);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Watching: the workspace's harness files by name, and every file the read reports outside it by its folder.

const watching = { key: null, outside: [], inside: [] };

function rewatch(onChange) {
    const on = config().get('autoRefresh', true);
    const targets = on && state ? lib.watchTargets(state.parsed, state.repo, home()) : [];
    const key = on ? JSON.stringify([state?.repo ?? null, targets]) : 'off';
    if (key === watching.key) {
        return;
    }
    watching.key = key;
    for (const w of [...watching.outside, ...watching.inside]) {
        w.dispose();
    }
    watching.outside = [];
    watching.inside = [];
    if (!on) {
        return;
    }
    const hook = (w) => {
        w.onDidChange(onChange);
        w.onDidCreate(onChange);
        w.onDidDelete(onChange);
        return w;
    };
    for (const folder of vscode.workspace.workspaceFolders ?? []) {
        watching.inside.push(hook(vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(folder, lib.WATCH_GLOB))));
    }
    for (const t of targets) {
        const glob = t.names.length === 1 ? t.names[0] : `{${t.names.join(',')}}`;
        watching.outside.push(
            hook(vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(vscode.Uri.file(t.dir), glob))),
        );
    }
    log.info(`watching ${watching.inside.length} workspace folder(s) and ${watching.outside.length} folder(s) outside it`);
}

// ---------------------------------------------------------------------------------------------------------------
// The page.

let page = { child: null, url: null, panel: null, repo: null };

/** The existing page, in a webview. Started read-only, always. */
async function openPage() {
    const repo = repoOf();
    if (!repo) {
        void vscode.window.showInformationMessage('rigswig: open a folder first.');
        return;
    }
    if (page.child && page.repo === repo) {
        if (page.panel) {
            page.panel.reveal();
        } else {
            void vscode.env.openExternal(await vscode.env.asExternalUri(vscode.Uri.parse(page.url)));
        }
        return;
    }
    stopPage();
    let started;
    try {
        started = await vscode.window.withProgress(
            { location: vscode.ProgressLocation.Window, title: 'rigswig: starting the page' },
            () => startPage(repo),
        );
    } catch (e) {
        log.error(`page: ${e.message}`);
        if (e.missing) {
            warnedMissing = false;
            void offerSetup();
        } else {
            const pick = await vscode.window.showErrorMessage(`rigswig: the page could not start — ${e.message}`, 'Show log');
            if (pick) {
                log.show();
            }
        }
        return;
    }
    page = { child: started.child, url: started.url, panel: null, repo };
    started.child.on('exit', () => {
        if (page.child === started.child) {
            stopPage('The page stopped (idle). Run "rigswig: Open the page" to start it again.');
        }
    });
    // The URL as the editor presents it: identity on a desktop, the forwarded port on a remote host.
    const shown = await vscode.env.asExternalUri(vscode.Uri.parse(started.url));
    if (config().get('openInBrowser', true)) {
        await vscode.env.openExternal(shown);
        return;
    }
    const panel = vscode.window.createWebviewPanel(
        'rigswig',
        `rigswig — ${basename(repo)} (read-only)`,
        { viewColumn: vscode.ViewColumn.Active, preserveFocus: false },
        { enableScripts: true, retainContextWhenHidden: true },
    );
    panel.iconPath = vscode.Uri.joinPath(extensionUri, 'media', 'activity.svg');
    panel.webview.html = lib.pageHtml(shown.toString());
    panel.onDidDispose(() => {
        if (page.panel === panel) {
            stopPage();
        }
    });
    page.panel = panel;
}

function stopPage(message) {
    if (page.child) {
        page.child.kill();
    }
    const panel = page.panel;
    page = { child: null, url: null, panel: null, repo: null };
    if (panel && message) {
        panel.webview.html = lib.stoppedHtml(message);
    }
}

// ---------------------------------------------------------------------------------------------------------------

let extensionUri;

function activate(context) {
    extensionUri = context.extensionUri;
    log = vscode.window.createOutputChannel('rigswig', { log: true });
    context.subscriptions.push(log);

    status.init(context);

    const findingsTree = new FindingsTree();
    const contextTree = new ContextTree();
    const findingsView = vscode.window.createTreeView('rigswig.findings', { treeDataProvider: findingsTree });
    const contextView = vscode.window.createTreeView('rigswig.context', { treeDataProvider: contextTree });
    const problems = vscode.languages.createDiagnosticCollection('rigswig');
    context.subscriptions.push(findingsView, contextView, problems);

    listeners.add(() => {
        findingsTree.refresh();
        contextTree.refresh();
        findingsView.message = message('findings');
        contextView.message = message('context');
        const value = state && findings.repo === state.repo ? findings.value : null;
        findingsView.badge = value && value.findings > 0 ? { value: value.findings, tooltip: `${value.findings} findings` } : undefined;
        findingsView.description = value?.saveable > 0 ? `~${short(value.saveable)} tok saveable` : undefined;
        contextView.description = state
            ? `${state.parsed.tool} · ~${fmt(state.parsed.tokens.value)} tok`
            : undefined;
        renderProblems(problems);
        rewatch(() => schedule(true, 1000));
    });

    // Every trigger re-reads the bill; only a refresh, a folder change or a watched file changing also drops the
    // cached findings, so moving between files never re-runs the audit. A drop asked for survives a later editor
    // move inside the same debounce.
    let timer = null;
    let drop = false;
    const schedule = (dropFindings, delay = 200) => {
        drop ||= dropFindings;
        clearTimeout(timer);
        timer = setTimeout(() => {
            if (drop) {
                findings = { repo: null, tool: null, value: null, error: null };
                drop = false;
            }
            void read();
        }, delay);
    };
    context.subscriptions.push({ dispose: () => clearTimeout(timer) });
    const refresh = () => schedule(true);
    let lastRepo = repoOf();

    context.subscriptions.push(
        vscode.commands.registerCommand('rigswig.refresh', refresh),
        vscode.commands.registerCommand('rigswig.openPage', openPage),
        vscode.commands.registerCommand('rigswig.showOutput', () => log.show()),
        vscode.commands.registerCommand('rigswig.setup', () => {
            warnedMissing = false;
            void offerSetup();
        }),
        vscode.commands.registerCommand('rigswig.openFile', async (file, line) => {
            const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(file));
            const at = new vscode.Position(Math.max(0, (line ?? 1) - 1), 0);
            await vscode.window.showTextDocument(doc, { selection: new vscode.Range(at, at), preview: true });
        }),
        vscode.workspace.onDidChangeWorkspaceFolders(() => {
            stopPage();
            refresh();
        }),
        vscode.window.onDidChangeActiveTextEditor(() => {
            // A file in the same folder changes nothing; only a move to another workspace folder re-reads.
            const repo = repoOf();
            if (repo !== lastRepo) {
                lastRepo = repo;
                schedule(false);
            }
        }),
        vscode.workspace.onDidChangeConfiguration((e) => {
            if (e.affectsConfiguration('rigswig.executablePath')) {
                warnedMissing = false;
                checkedVersion = null;
                refresh();
            } else if (e.affectsConfiguration('rigswig')) {
                watching.key = null;
                changed();
            }
        }),
        vscode.workspace.onDidGrantWorkspaceTrust(refresh),
        { dispose: stopPage },
        { dispose: () => [...watching.outside, ...watching.inside].forEach((w) => w.dispose()) },
    );
    refresh();
}

function deactivate() {
    stopPage();
}

module.exports = { activate, deactivate };
