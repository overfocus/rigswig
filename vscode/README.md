# rigswig, in your editor

A thin extension for VS Code, Cursor and their forks: what a coding-agent session in the open folder loads, what the
audit finds in it, and rigswig's read-only page in a panel. It spawns the CLI you already have and implements
nothing twice.

## What you see

- **The status bar**, when a folder is open: the harness, the token bill, and, once the audit has run, how many
  findings it found and roughly how many tokens a session the fixes could save:

  ```text
  $(beaker) claude-code · ~11k tok · $(warning) 10 findings · ~1.7k saveable
  ```

  The tooltip breaks it down: the estimate and its range, the fixed costs no file shows, the findings by severity.
  Clicking it opens the page. When the CLI is missing it reads **rigswig: set up** and offers to install it or set
  its path; when a read fails, the click opens the log.
- **The rigswig view** in the activity bar, in two parts:
  - **Findings**, most severe first, each with its proposed fix, its evidence and the files it is about; clicking
    one opens its file. The count shows as a badge on the view.
  - **What loads**: every instruction file, agent, skill, command, MCP server and hook the session gets, grouped
    by kind, the costliest startup groups first, with each one's share of the bill and its layer or plugin; clicking
    one opens its file. Deferred/on-match costs are labelled separately from startup costs. Items that are overridden
    or turned off are counted, not listed.
- **The toolbar**: an open-page button on the editor's title bar whenever a file a harness reads is open
  (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.mcp.json`, a `SKILL.md`, anything under `.claude/`, `.codex/`,
  `.cursor/`…), next to refresh and open on the rigswig view (and on the page panel, when the page opens in one).
- **The Problems panel**: each finding on the files inside the open folders it names (`rigswig.problems`).
- **The page**: the same local page `rigswig ui` serves, in the system browser by default (an editor panel with
  `rigswig.openInBrowser` off), compare included, always read-only.

It keeps itself current: creating, changing or deleting a file the harness reads (`CLAUDE.md`, `AGENTS.md`,
`.claude/`, `.codex/`, `.omp/`, `.pi/`, `.deepseek/`, `.hermes/`, `.agents/`, a skill, a settings file in your home
folder…) re-reads the bill and re-runs the audit,
debounced (`rigswig.autoRefresh`). Moving between files of the same folder re-runs nothing; moving to another
folder of a multi-root workspace reads that one. **rigswig: Refresh** re-reads by hand, and **rigswig: Show the
log** shows every CLI run with its exit code and time.

## Install

1. Have the CLI on the `PATH` (`cargo install rigswig`, `npm install -g rigswig` or `brew install
   overfocus/tap/rigswig`; [getting started](../docs/GETTING-STARTED.md#install)) or point `rigswig.executablePath`
   at a binary. The extension needs rigswig 0.2.0 or newer and says so when it finds an older one.
2. Install the extension from the Marketplace or Open VSX (`kftcda.rigswig`), or a VSIX from
   [the releases](https://github.com/overfocus/rigswig/releases) (`code --install-extension rigswig-<version>.vsix`),
   or package it yourself from `vscode/` with `npx @vscode/vsce package --no-dependencies`.

## Settings

| Setting | Default | What it does |
|---|---|---|
| `rigswig.executablePath` | `rigswig` | The binary to spawn: on the `PATH`, or a full path. In Restricted Mode only your user settings' value counts. |
| `rigswig.autoRefresh` | `true` | Re-read when a file the harness loads changes. |
| `rigswig.problems` | `workspace` | Findings in the Problems panel: on files in the open folders, on `all` files they name, or `off`. |
| `rigswig.statusBar` | `true` | Show the bill in the status bar. |
| `rigswig.openInBrowser` | `true` | The page opens in the system browser; turn off to open it in an editor panel instead (some editors refuse localhost frames). |

## How it reads, and what it never does

The extension spawns three rigswig commands and nothing else: `status --json` for the bill and the What loads
view, `audit --json --tool <harness-status-selected>` for the findings, and `--version` once per binary. All hold
the CLI's own guarantees:

- the page spawn is read-only: it carries neither `--allow-writes` nor `--allow-github`;
- the findings spawn reads files only: it never passes `--usage`, because session logs are opt-in per run and an
  editor is not a run that opted in;
- no writes, no downloads, no model calls;
- a workspace cannot choose the binary: in an untrusted folder the editor ignores the folder's own
  `rigswig.executablePath`, so opening a cloned repo never runs a program it names;
- third-party text (finding titles, item names, the CLI's errors) is shown as text, never as trusted markdown or
  HTML; every CLI run has a deadline, and on Windows every argument is quoted for the shell.

Every one of these is asserted, not promised: `node vscode/test.mjs` runs the checks against the real binary when
a build is present (`node >= 18`, no dependencies).

## Developing

After `cargo build`, start an editor with the extension loaded from the checkout:

```sh
code --extensionDevelopmentPath="$PWD/vscode" .     # or cursor
```

and set `rigswig.executablePath` to the repo's `target/debug/rigswig`. **rigswig: Show the
log** shows each CLI run. `node vscode/test.mjs` covers everything that runs outside the editor.
