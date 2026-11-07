# rigswig

See everything your coding agent loads before you type, and where each piece came from.

**Start here: [getting started](docs/GETTING-STARTED.md) — install it, three commands, and what each one tells
you.**

![rigswig's page for a repo: about 15.9k tokens load before you type, 1.6% of the 1M window its model (opus) has, most of it AGENTS.md, with every kind of item as a tile](docs/images/overview.png)

<table>
  <tr>
    <td width="50%"><img src="docs/images/compare.png" alt="Claude Code as A and Codex as B on the same repo: each one's model and window, two budgets on one scale, and a prompt to pick the window of a model rigswig does not know"></td>
    <td width="50%"><img src="docs/images/write-mode.png" alt="Write mode: turning a plugin off shows the file, the diff and the budget after, before anything is written"></td>
  </tr>
  <tr>
    <td><b>Compare</b> two harnesses on one repo (or two repos): Codex cuts this <code>AGENTS.md</code> at its 32&nbsp;KiB budget.</td>
    <td><b>Write mode</b> (opt-in): every change shows the file, the diff and the new budget first, with a backup and undo.</td>
  </tr>
</table>

<img src="docs/images/agents.png" alt="Every agent found, with its status, layer, tools and file; a user agent overridden by a project agent of the same name">

<table>
  <tr>
    <td width="50%"><img src="docs/images/audit.png" alt="The audit on the page: 10 findings, 2 of them high, about 1.7k tokens a session they could save; an instruction file over 200 lines with its largest sections, risky permission rules, user-scope agents nothing in the repo references"></td>
    <td width="50%"><img src="docs/images/audit-fleet.png" alt="Fleet mode across three repos: 28 user-scope items to archive or move, about 1.9k tokens a session; each repo with its findings, then each item with its recommendation"></td>
  </tr>
  <tr>
    <td><b>Audit</b> a repo: ranked findings, each with its evidence, the tokens it would save and a fix. Filter them, and open the items they name.</td>
    <td><b>Fleet mode</b> across many repos: which user-scope agents, skills and plugins any repo needs, and what to archive or move to project scope.</td>
  </tr>
</table>

<sub>Screenshots of an invented setup, rebuilt with <code>docs/demo/make-demo.sh</code> and taken with <code>docs/demo/shoot.mjs</code>.</sub>

A coding agent's **harness** is everything around the model: instruction files (`CLAUDE.md`, `AGENTS.md`,
`GEMINI.md`, rules), subagents, skills, commands, hooks, MCP servers, plugins, permissions and the model setting,
spread across user, project, local and managed scopes. rigswig reads all of it for a repo and shows what a session
there actually gets, which scope decided each setting, what is overridden, and roughly how much context it costs
before you type a word.

**Status: 0.2, pre-1.0.** `rigswig status`, `rigswig audit` and the local page, `rigswig ui`, work for every
harness in the table below. `rigswig usage` reads Claude Code's session logs only, for now; the other harnesses are
[next](docs/ROADMAP.md#next-usage-from-more-harnesses). See [docs/ROADMAP.md](docs/ROADMAP.md).

rigswig is a single binary written in Rust. Install it with `cargo install rigswig`
([crates.io](https://crates.io/crates/rigswig); add `--no-default-features` for a build with no network code), with
`npx rigswig` or `npm install -g rigswig` ([npm](https://www.npmjs.com/package/rigswig): the package installs the
right prebuilt binary for your platform), or download it for macOS, Linux or Windows from
[the releases](https://github.com/overfocus/rigswig/releases), each with a `SHA256SUMS` line to check it against.
On macOS or Linux, `brew install overfocus/tap/rigswig` installs the same release binaries through Homebrew
([the tap](https://github.com/overfocus/homebrew-tap)). From a checkout:

```sh
cargo run -- status --repo path/to/repo          # a summary
cargo run -- status --repo path/to/repo --json   # everything, as schema/harness.schema.json describes
cargo run -- ui --repo path/to/repo              # the same as a page on 127.0.0.1, behind a random token
cargo run -- compare repo-a repo-b               # what differs between two repos, one harness
cargo run -- compare path/to/repo --tools claude-code,codex   # or between two harnesses on one repo
cargo run -- check --repo path/to/repo --max-tokens 40k        # for CI: exit 1 when the budget is passed
cargo run -- status --repo path/to/repo --without layer:user   # what-if: the totals without some items
cargo run -- audit --repo path/to/repo                          # ranked findings, each with a saving and a fix
cargo run -- audit --repos '~/code/*'                           # fleet: each user-scope item against every repo
cargo run -- audit --github owner/repo                          # a public GitHub repo, in memory, without your setup
cargo run -- audit --archive repo.tar.gz                        # the same for an archive you downloaded, offline
cargo run -- usage --repo path/to/repo                          # what past sessions here invoked, metadata only
cargo run -- audit --repo path/to/repo --usage                  # findings that know what the sessions called
```

`rigswig audit` turns the numbers into a to-do list: user-scope agents and skills that nothing in the repo references,
plugins switched on everywhere that the repo's files don't call for, instruction files over budget, sentences repeated
across layers, paths and scripts the instructions name that don't exist, and permission rules that have piled up or
allow too much. Each finding carries its evidence, the tokens it would save (computed like `--without`) and a proposed
change. Fleet mode answers the question a single repo can't: which repos need each user-scope item, so it can move to
those repos' project scope. The page runs both too, with a progress bar, and lets you filter the findings and jump
from one to the items it names. See [docs/AUDIT.md](docs/AUDIT.md).

The page is local: it listens on 127.0.0.1 only, answers only requests that carry its token and the right `Host`,
loads nothing from anywhere else, and stops after 30 minutes without a request. It is read-only unless started with
`--allow-writes`, which lets it turn plugins and project MCP servers on and off, each with a preview (the file, the
diff, the budget before and after), a backup and undo: see [docs/WRITE-MODE.md](docs/WRITE-MODE.md).

## Guarantees

These are the point of the tool, and they will be enforced by tests:

- **No network unless you ask.** rigswig sends no telemetry and makes no request, except one: `--github owner/repo`
  downloads that public repo's archive from `codeload.github.com`, only in the run you give it to, or for each
  GitHub audit you start on a page launched with `ui --allow-github` ([GitHub repos](docs/GITHUB.md)). Build with
  `--no-default-features` for a rigswig with no network code at all.
- **No model calls.** Everything it shows is read from files on your machine.
- **Read-only by default.** Nothing is ever written unless you start the page with `rigswig ui --allow-writes`
  ([write mode](docs/WRITE-MODE.md)).
- **An MCP server.** `rigswig mcp` serves the five read-only surfaces as MCP tools over stdio, so a harness
  can ask rigswig about a checkout from inside its own session — Claude Code, Cursor, Gemini CLI, DeepSeek-CLI,
  VS Code and anything else that launches stdio servers ([the server and its registration](docs/MCP.md)).
- **In your editor.** A thin VS Code / Cursor extension: the bill and the audit's findings in the status bar, a
  refresh/open toolbar on the page panel, and the read-only page itself. It spawns the CLI you already have; it
  starts nothing in write mode and never opts a run into session logs ([the extension](vscode/) and
  [its logic tests](vscode/test.mjs)).
- **No secrets.** MCP server `env`, `headers` and `args`, URL paths and inline hook code are never printed or served.
- **Session logs only when you ask, and only their metadata.** No command opens a session log unless the run asks
  for it: `rigswig usage` or `rigswig audit --usage`, never the page or the MCP server. Even then, rigswig reads
  only ids, times, the working folder, models, token counts, and the names of the agents, skills and MCP servers
  that were called. It never keeps, prints or serves message text, thinking, tool inputs or tool results
  ([what is read, field by field](docs/USAGE.md)). This replaces the earlier "No transcripts" promise
  ([what changed](docs/USAGE.md#what-changed-in-the-guarantee)). (Claude Code's
  auto-memory index, `MEMORY.md`, is read and counted in every run because it loads into every session.)
- **Private folders stay private.** List them in `~/.rigswig/private.json` (`{"private": ["~/notes"]}`): rigswig
  refuses them as a repo, and `audit --repos` skips them.

## Harnesses

Harness-agnostic: each harness is an adapter that reads its own files into one shared data model.

| Harness | Status |
|---|---|
| Claude Code | `rigswig status` ([the rules it models](docs/adapters/claude-code.md)) |
| OpenAI Codex CLI | `rigswig status --tool codex` ([the rules it models](docs/adapters/codex.md)) |
| Hermes Agent | `rigswig status --tool hermes` ([the rules it models](docs/adapters/hermes.md)) |
| Ollama | `rigswig status --tool ollama` ([the rules it models](docs/adapters/ollama.md)) |
| Devin CLI | `rigswig status --tool devin` ([the rules it models](docs/adapters/devin.md)) |
| Cursor | `rigswig status --tool cursor` ([the rules it models](docs/adapters/cursor.md)) |
| Gemini CLI | `rigswig status --tool gemini` ([the rules it models](docs/adapters/gemini.md)) |
| DeepSeek Harness | `rigswig status --tool dsh` ([the rules it models](docs/adapters/dsh.md)) |
| DeepSeek-CLI (community) | `rigswig status --tool deepseek` ([the rules it models](docs/adapters/deepseek-cli.md)) |
| GitHub Copilot CLI | `rigswig status --tool copilot` ([the rules it models](docs/adapters/copilot.md)) |
| Hugging Face CLI | `rigswig status --tool hf` ([the rules it models](docs/adapters/hf.md)) |
| Pi | `rigswig status --tool pi` ([the rules it models](docs/adapters/pi.md)) |
| OpenCode, others | adapters welcome |

## Related tools

Other tools work on the same question, and are worth a look:
[harness-map](https://github.com/codemakim/harness-map) (Codex and Claude Code, with a what-if mode),
[whatloads](https://github.com/sheldor26/whatloads) (Claude Code's context bill),
[claude-config-auditor](https://github.com/emreyildirim/claude-config-auditor) (lint and CI), and Claude Code's own
`/context`.

## Licence

[Apache-2.0](LICENSE).
