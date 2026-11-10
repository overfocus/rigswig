# Getting started

rigswig reads a coding agent's **harness** — the instruction files, agents, skills, commands, hooks, MCP servers,
plugins, permissions and model setting that load around the model — and shows what a session in a repo actually
gets, which scope decided each piece, and roughly what it costs before you type a word. Install it, point it at a
repo, and it is useful inside a minute.

## Install

```sh
cargo install rigswig        # crates.io; add --no-default-features for a build with no network code at all
npm install -g rigswig       # npm: installs the right prebuilt binary for your platform
brew install overfocus/tap/rigswig   # Homebrew, macOS or Linux: the same release binaries
```

Or download a macOS, Linux or Windows binary from [the releases](https://github.com/overfocus/rigswig/releases),
each with a `SHA256SUMS` line to check it against, or run without installing: `npx rigswig`. The lines below say
plain `rigswig`; from a checkout, `cargo run --` before each one does the same.

## Three commands

From the checkout you want to look at:

```sh
rigswig status             # what a session loads here, and what it costs
rigswig audit              # ranked findings: what to move, drop or fix, and the tokens each fix saves
rigswig ui                 # all of it as a local page, with compare and write mode
```

`status` names the harness it detected and prints the bill:

```text
claude-code in my-app
  Context   ~2,915 estimated tokens from startup files (2,332..3,886, from 11,657 characters)
            + not measured: tool listings of 1 MCP server (docs-search)
```

`audit` turns that into a to-do list — user-scope agents and skills nothing here references, plugins on everywhere
that the repo's files do not call for, instruction files over budget, repeated sentences, permission rules that
piled up — each finding with its evidence and a proposed change ([more](AUDIT.md)). Add `--repos '~/code/*'` and it
is fleet mode: every user-scope item judged against every repo at once.

`ui` serves the same data as a page on 127.0.0.1 only, behind a random token, and stops by itself after half an
hour idle. In the editor there is an extension: the bill and the audit's findings in the status bar, and
`rigswig: Open the page` for the page itself ([the extension](../vscode/)).

## What to reach for next

| You want to... | Run | Read |
|---|---|---|
| Require complete, unambiguous resolution in CI | `rigswig check --strict` (exit 1 on an incomplete read; unsupported adapters refuse) | [strict checks](STRICT.md) |
| Cut startup tokens in CI | `rigswig check --max-tokens 40k` (exit 1 when the budget is passed) | `rigswig --help` |
| See another harness beside this one | `rigswig compare . --tools claude-code,codex` | the harnesses table in [README](../README.md#harnesses) |
| Ask what past sessions here actually invoked | `rigswig usage` (findings: `rigswig audit --usage`) | [USAGE.md](USAGE.md) |
| Turn a plugin or MCP server off | `rigswig ui --allow-writes` (a preview, a backup and undo first) | [WRITE-MODE.md](WRITE-MODE.md) |
| Let the agent itself ask rigswig | `rigswig mcp` in the harness's MCP settings | [MCP.md](MCP.md) |
| Point rigswig at a repo you do not have | `rigswig audit --github owner/repo` or `--archive repo.tar.gz` | [GITHUB.md](GITHUB.md) |
| Keep a folder out of rigswig | `~/.rigswig/private.json` with `{"private": ["~/notes"]}` | the [guarantees](../README.md#guarantees) |

Each command reads files and does nothing else unless you ask: no network outside the opt-in `--github` download,
no model calls, no writes outside `ui --allow-writes`, and session logs only when a run says `--usage`
([every guarantee](../README.md#guarantees), and what enforces them). `check`, `audit`, `compare` and MCP choose the
harness from repository evidence alone, so live activity elsewhere cannot change an automation result.

## Where the details live

- [README.md](../README.md) — what rigswig is, the guarantees, every harness.
- [docs/AUDIT.md](AUDIT.md) — the findings and fleet mode.
- [docs/STRICT.md](STRICT.md) — incomplete-read checking, adapter readiness and exit behavior.
- [docs/USAGE.md](USAGE.md) — session-log usage, metadata only, and what changed in the guarantee.
- [docs/adapters/](adapters) — the rules modelled per harness, each with its doc sources.
- [docs/ROADMAP.md](ROADMAP.md) — what is done and what is deliberately not.
