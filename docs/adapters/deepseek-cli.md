# The DeepSeek-CLI adapter: the rules it models

Every rule below is from the DeepSeek-CLI repository (github.com/charsdavy/deepseek-cli) — its README and its
`src/` sources (`config/config.ts`, `config/instructions.ts`, `skills/store.ts`, `agent/hooks.ts`,
`memory/store.ts`), read on 2026-11-06 for the v0.4 era. This is the **community** CLI powered by DeepSeek's API
(a Homebrew install, an agentic tool loop over the chat completions), not DeepSeek AI's own harness — the
adapter after this one (`dsh`, docs/adapters/dsh.md) covers the official DeepSeek Harness. Where the sources are
silent and the adapter has to choose, the rule is marked **inferred** and listed in the output's
`coverage.notModelled`. When the sources change, change this file and the adapter together.

The sources are code, not marketing docs: the loader's exact behaviour (candidate order, first-match-only,
truncation lengths, scan priorities) is quoted from the TypeScript, with the README as the contract.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `env` | 0 | the launch environment: the `DEEPSEEK_*` variables the caller passed (the docs "env vars > flags" head of the chain) |
| `user` | 1 | `~/.deepseek-cli/`: `config.json`, `mcp.json`, `hooks.json`, `skills/`, `memories/` |
| `project` | 2 | the repo's `.deepseek/` (`config.json`, `skills/`, `hooks.json`), the root instruction file, `.mcp.json` |
| `imports_user` | 3 | the home's Claude, Codex and codemaker skill directories |
| `imports_project` | 4 | the repo's Claude, Codex and codemaker skill directories |

The config chain is env > CLI flags > project config (`.deepseek/config.json`) > global config
(`~/.deepseek-cli/config.json`) > defaults (the source's own comment); the flags are unreadable at rest, so they
join the env layer only as far as it reaches.

## Settings

Each with its trail, highest first: `model` (`defaultModel`: `DEEPSEEK_MODEL`, then the project config, then the
global config; the built-in default is `deepseek-chat`), `baseUrl` (`DEEPSEEK_BASE_URL` then the two configs),
`approvalMode` (`ask` / `auto` / `yolo` / `deny-pure-shell`) and `maxContext` from the two configs. The API key
is never read.

## Instructions

The loader walks one fixed list at the repo root — `AGENTS.md`, `deepseek.md`, `.deepseek`, `CLAUDE.md`,
`.cursorrules` — and **the first existing file wins**: injected into the system prompt, truncated at 16,000
characters. The rest of the list is never read when one wins, so only the winner is an item. A file that exists
but reads empty counts as a miss and the walk continues (**inferred**: the loader would inject nothing, so
nothing is measured).

## Memory

Each project's long-term memory loads at session start into the project context (`memoryGeneration` defaults on):
`~/.deepseek-cli/memories/<sha256-of-cwd, first 12 hex>/MEMORY.md`, capped at 24,000 characters by the loader.
One startup instruction item named for its file. Old session summaries expire after seven days — a runtime
concern, not a load.

## Skills

`skillSearchPaths` scans, in order: deepseek global (`DEEPSEEK_SKILLS_DIR` or `~/.deepseek-cli/skills`), deepseek
project (`.deepseek/skills`), then Claude (`CLAUDE_CONFIG_DIR` or `~/.claude`, and `.claude/skills`), then Codex
(`CODEX_HOME`, `.codex/skills`), then codemaker (`CODEMAKER_HOME`, `.codemaker/skills`) — the same global-then-
project pair per tool. Both layouts load: flat `<name>.md` and `<name>/SKILL.md` (symlinks followed; no deeper
recursion). A name clash keeps the earlier source ("deepseek wins first"), and the winners' bodies fold into the
system prompt **when active** — the `/skill` picker is runtime state, so the listing (name + the extracted
first-heading-or-paragraph description, capped at 80 characters) is measured at startup and the body on invoke.

## Hooks

Two flat-array JSON files — `.deepseek/hooks.json` (project; its hooks execute first) and
`~/.deepseek-cli/hooks.json` — each an array of `{ event, command, matcher?, timeout? }` where the events are
`PreToolUse`, `PostToolUse`, `PreCompact` and `PostCompact`. They are additive (project before global, no
shadowing). The events map onto the shared neutral names (`PreToolUse` → `before-tool`,
`PreCompact` → `before-compact`). A matcher is a regular expression against the tool name at runtime.

## MCP servers

`mcpServers` from `~/.deepseek-cli/mcp.json` (global) and the repo's `.mcp.json` (project-specific) — the
Claude-shaped map. A command server keeps its file name; a url server its origin; `args` and `env` never surface.
A same-name server across the two files: the docs do not name a winner, so both load (**inferred**), and the
project one wins in rigswig's model as the more specific file.

## Env vars read

`DEEPSEEK_MODEL`, `DEEPSEEK_BASE_URL`, `DEEPSEEK_SKILLS_DIR` and the skill roots' own relocation variables
(`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `CODEMAKER_HOME`) — the vars `skillSearchPaths` honours, read through the
harness-neutral Ctx, never the machine's environment.
