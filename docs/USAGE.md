# Usage from session logs

Everything else rigswig shows is what a session *loads*. `rigswig usage` shows what sessions in a repo actually
*invoked*: the agents, skills and MCP servers they called, how many sessions there were, which models answered and
how many tokens went through. `rigswig audit --usage` uses those counts to judge what loads in every session but no
session called.

It reads the harness's own session logs. That is a change from rigswig's original promise of "no transcripts", so
this page says exactly what is read and what is not.

## What changed in the guarantee

| Before | Now |
|---|---|
| rigswig never opens session logs. | rigswig opens a repo's session logs only in a run that asks for them: `rigswig usage`, or `rigswig audit --usage`. Every other command (`status`, `check`, a plain `audit`, `compare`, `tools`, `ui`, `mcp`) does not, by test. |
| | From those logs it keeps metadata only. It never keeps, prints or serves message text, thinking, tool inputs or tool results. |

Neither the page nor the MCP server reads session logs. An agent connected to `rigswig mcp` cannot ask for them, and
the page has no switch that turns it on.

## What is read

Claude Code writes one JSON Lines file per session under `~/.claude/projects/<project folder>/` (or under
`$CLAUDE_CONFIG_DIR`), plus one file per subagent in `<session>/subagents/`. This is the same file a hook receives as
`transcript_path` ([hooks](https://code.claude.com/docs/en/hooks.md)). rigswig opens the files in the repo's project
folder and in the folders of subfolders inside the repo. It counts a line only when its working folder (`cwd`) is
the repo or a folder inside it.

From each line, the parser reads these fields and no others:

| Field | Used for |
|---|---|
| `sessionId`, `timestamp`, `cwd` | counting sessions, the time window, keeping to this repo |
| `message.id`, `message.model`, `message.usage` (the four token counts) | tokens and models, each response counted once |
| a `tool_use` block's `type`, `id` and `name` | which tool was called: `Skill`, `Agent` / `Task`, or `mcp__<server>__<tool>` |
| that block's `input.skill` and `input.subagent_type` | which skill or agent it was |
| an `invoked_skills` attachment's skill `name`s | skills the log lists as invoked in a session |

Every other field is skipped by the JSON parser without being stored: message text, thinking, every other key of a
tool's input (the `command`, the `prompt`, file contents), tool results, and attachments' `content`. The structs the
lines are read into have no field for any of it (`crates/rigswig-core/src/adapters/claude_code/usage.rs`). The
tests plant a canary in each of those places and check that it never reaches the output, in core
(`crates/rigswig-core/tests/usage.rs`) and through the binary (`crates/rigswig/tests/cli.rs`).

Two more limits:

- **Size.** Logs are streamed, so memory holds one line at a time. Lines over 16 MB, and lines that are not UTF-8
  or not JSON, are skipped and counted.
- **No content in errors.** A file that cannot be read is counted. Neither the file nor any of its text is printed.

## What is not counted

- **Skills and commands you type as `/name`.** They appear only in message text, which is never read. A skill
  counts as invoked when the model called it through the `Skill` tool, or when the log lists it as invoked for the
  session. That is why a `not-invoked` finding about skills is never High, and why its evidence says to check before
  archiving.
- **Hooks.** Their runs are not in the parts of the log this reads.
- **Built-in tools** (Bash, Read, Edit and the rest). They are not part of a setup.
- **Sessions started outside the repo's folder**, such as a worktree kept elsewhere.
- **Logs Claude Code has already deleted.** It removes them after `cleanupPeriodDays`, 30 by default
  ([settings](https://code.claude.com/docs/en/settings.md)).
- **The log format is not documented by Anthropic.** The shapes read are inferred from the files Claude Code
  writes. A line in any other shape is skipped and counted, so a format change shows up as skipped lines rather than
  wrong figures.

Only Claude Code's logs are read for now. Every other harness refuses (`rigswig does not read <tool> session logs
yet`) until it has its own metadata-only reader, a canary test and an entry here.

## The commands

```sh
rigswig usage [--repo <path>] [--tool <id>] [--days <n>|all] [--json]
rigswig audit [--repo <path>] [--tool <id>] --usage [--days <n>|all] [--json]
```

The window is the last 30 days by default. Both commands say on stderr what they are about to read before they open
a log. `--usage` is refused with `--github`, `--archive` and `--repos`, since those are about repos that are not this
machine's, or about many repos at once.

`rigswig audit --usage` changes two things:

- **`heavy-user-scope`:** an item invoked in the window counts as needed, even if nothing in the repo's files names
  it.
- **`not-invoked`** (new): agents, skills and MCP servers that load in every session and that none of the window's
  sessions invoked. There is one finding per kind, and an item an earlier finding already names is left to that
  finding. A plugin's `plugin:name` also counts as invoked when called by its bare `name`. The finding needs at
  least 5 sessions in the window; with fewer it is a note instead.
