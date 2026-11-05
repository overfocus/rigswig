# rigswig

See everything your coding agent loads before you type, and where each piece came from.

A coding agent's **harness** is everything around the model: instruction files (`CLAUDE.md`, `AGENTS.md`,
`GEMINI.md`, rules), subagents, skills, commands, hooks, MCP servers, plugins, permissions and the model setting,
spread across user, project, local and managed scopes. rigswig reads all of it for a repo and shows what a session
there actually gets, which scope decided each setting, what is overridden, and roughly how much context it costs
before you type a word.

**Status: pre-alpha.** This is the repository skeleton. Nothing is implemented yet; see [docs/ROADMAP.md](docs/ROADMAP.md).

## Guarantees

These are the point of the tool, and they will be enforced by tests:

- **No network.** rigswig never makes a request, and sends no telemetry.
- **No model calls.** Everything it shows is read from files on your machine.
- **Read-only.** It never writes to your configuration.
- **No secrets.** MCP server `env`, `headers` and `args`, URL paths and inline hook code are never printed or served.
- **No transcripts.** It never reads session history.

## Harnesses

Harness-agnostic: each harness is an adapter that reads its own files into one shared data model.

| Harness | Status |
|---|---|
| Claude Code | planned for v0.1 |
| OpenAI Codex CLI | planned for v0.1 |
| Gemini CLI | planned |
| GitHub Copilot (repo files) | planned |
| Cursor, OpenCode, others | adapters welcome |

## Related tools

Other tools work on the same question, and are worth a look:
[harness-map](https://github.com/codemakim/harness-map) (Codex and Claude Code, with a what-if mode),
[whatloads](https://github.com/sheldor26/whatloads) (Claude Code's context bill),
[claude-config-auditor](https://github.com/emreyildirim/claude-config-auditor) (lint and CI), and Claude Code's own
`/context`.

## Licence

[Apache-2.0](LICENSE).
