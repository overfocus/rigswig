# rigswig, in your editor

A thin extension for VS Code, Cursor and their forks: the harness's context bill and the audit's findings in the
status bar, a toolbar on the page panel, and rigswig's read-only page in a webview. It spawns the CLI you already
have and implements nothing twice.

## What you see

- **The status bar**, when a folder is open: the harness, the token bill, and — once the audit has run — how many
  findings it found and roughly how many tokens a session the fixes could save:

  ```text
  $(beaker) claude-code · ~11,425 tok · 10 findings · ~1.7k saveable
  ```

  The tooltip breaks it down: the estimate and its range, the fixed costs not measured, the findings by severity.
  Clicking it opens the page. **rigswig: Refresh** re-runs both reads.
- **The panel toolbar**: two editor-title buttons on the page panel — refresh, and open the page — so the reads
  are a click away without the command palette.
- **The page**: the same local page `rigswig ui` serves, in a panel (or in a browser with
  `rigswig.openInBrowser`), compare and write mode included.

## Install

1. Have the [CLI](https://github.com/overfocus/rigswig) on the `PATH` (`cargo install rigswig`, `npm install -g
   rigswig`) or point the `rigswig.executablePath` setting at a binary.
2. Install the extension: a VSIX from [the releases](https://github.com/overfocus/rigswig/releases)
   (`code --install-extension rigswig-<version>.vsix`), or package it yourself from `vscode/` with
   `npx @vscode/vsce package --no-dependencies`.

## Settings

| Setting | Default | What it does |
|---|---|---|
| `rigswig.executablePath` | `rigswig` | The binary to spawn: on the `PATH`, or a full path. |
| `rigswig.openInBrowser` | `false` | Open the page in the system browser instead of a panel. |

## How it reads, and what it never does

The extension spawns two rigswig commands and nothing else: `status --json` for the bill, and `audit --json` for
the findings (cached per repo; the active editor moving between files does not re-run it, a folder change or a
refresh does). Both hold the CLI's own guarantees:

- the page spawn is read-only — it carries neither `--allow-writes` nor `--allow-github`;
- the findings spawn reads files only — it never passes `--usage`, because session logs are opt-in per run and an
  editor is not a run that opted in;
- no writes, no downloads, no model calls.

Every one of these is asserted, not promised: `node vscode/test.mjs` runs the checks against the real binary when
a build is present (`node >= 18`, no dependencies).
