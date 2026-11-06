# The Cursor adapter: the rules it models

Every rule below is from Cursor's documentation at cursor.com/docs — `context/rules`, `context/skills`,
`context/subagents`, `hooks`, `context/mcp` and `context/ignore-files` — read on 2026-11-06, against the release
the docs describe (the 2.5 era: nested `AGENTS.md`, subagents, commands migrating into skills). Where the docs are
silent and the adapter has to choose, the rule is marked **inferred** and listed in the output's
`coverage.notModelled`. When the docs change, change this file and the adapter together.

Cursor is an editor whose agent reads its instructions, skills, subagents, hooks and MCP servers from the repo's
`.cursor/` directory and the home's `~/.cursor/`. Its own settings (model, tool toggles, auto-run approvals, User
Rules) live in the app's database rather than in config files, so nothing of those is on disk to read: the
adapter's context total is the file-borne context, which is what Cursor's agent loads into the model prompt. For
compatibility the agent also reads files from sibling tool folders in the checkout (Claude, Codex, and the shared
`.agents/skills/`).

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `user` | 0 | `~/.cursor/`: `hooks.json`, `mcp.json`, `skills/`, `agents/`, `commands/` |
| `project` | 1 | the repo's `.cursor/` (`rules/**.mdc`, `skills/`, `agents/`, `hooks.json`, `mcp.json`, `commands/`), plus the repo root's `AGENTS.md` and `.cursorrules` |
| `imports_project` | 2 | the repo's Claude and Codex files Cursor reads for compatibility: `.claude/skills/`, `.claude/agents/`, `.claude/settings.json` (+ `settings.local.json`) hooks, `.codex/skills/`, `.codex/agents/`, `.agents/skills/` |
| `imports_user` | 3 | the home's compat folders: `~/.claude/skills/`, `~/.claude/agents/`, `~/.codex/skills/`, `~/.codex/agents/` |

The imports rank below Cursor's own directories (**inferred**: the docs give no merge order between the compat
sources and Cursor's own files; rigswig keeps a compatibility file from shadowing a Cursor file of the same name).

## Instructions

- Project rules are `.cursor/rules/**.mdc`, including subfolders. The three frontmatter fields decide how each
  loads ([rules](https://cursor.com/docs/rules)): `alwaysApply: true` every session; `globs` (comma-separated) on
  match, with the patterns copied into `applies_to`; a `description` alone on the agent's decision; no fields at
  all only via @-mention. The last two are the same in rigswig's model (`load: on-invoke`) — the difference is
  who decides, the agent either way.
- A plain `.md` file in `.cursor/rules` is ignored (wrong extension, no frontmatter).
- `AGENTS.md` at the repo root loads every session; the docs' nested support puts one in any subdirectory, applied
  when the agent works with files in that directory — listed as `load: on-invoke`, out of the startup total.
- The root `.cursorrules` is deprecated (project rules "replaced" it; an on/off settings toggle keeps it
  (**inferred**)); presence gets a `cursorrules-legacy` warning, and the file is counted as a startup instruction.
- Rule bodies' `@file.md` mentions are **not** inlined into the prompt — the docs are explicit that the agent
  reads the file with its tools when it needs it — so the mention stays part of the rule's characters and follows
  no import trail.

## Skills and commands

- `SKILL.md` folders under `.cursor/skills/**` (project, walked recursively so category subfolders are
  organisation, not identity — the name is the folder holding `SKILL.md`), `~/.cursor/skills/` (user),
  `.agents/skills/` and `~/.agents/skills/`, and the compat `.claude/skills` / `.codex/skills` at both levels
  ([skills](https://cursor.com/docs/context/commands)). Every skill is `load: on-invoke` (typed `/name` or
  attached `@name`): the listing (name + description) is measured at startup, the body on invoke.
- The legacy slash-command `.md` files under `.cursor/commands/` are listed as skills named for their file stem;
  Cursor 2.4+ migrates them with `/migrate-to-skills`.

## Agents (subagents)

`<name>.md` under `.cursor/agents/` (project and user) and the compat `.claude/agents/` / `.codex/agents/`. One
item per file with its frontmatter: `model` (the `inherit` default means no own model), `tools` — there are none;
a subagent takes the parent's ([subagents](https://cursor.com/docs/subagents)). `readonly` and `is_background` are
not modelled (permissions and scheduling, not context).

## Hooks

Sources, each event's hooks additive — every matching hook from every source runs, and Cursor merges the
responses rather than shadowing ([hooks](https://cursor.com/docs/hooks)):

- `.cursor/hooks.json` (project; runs from the project root) and `~/.cursor/hooks.json` (user; runs from
  `~/.cursor/`): the file's `hooks` object maps an event to hook definitions with their `command` and `matcher`.
- Claude Code's hooks, for the compat import: `.claude/settings.json` + `settings.local.json` in the repo and
  `~/.claude/settings.json` (the docs' Third-Party Hooks note names Claude Code; the exact file set is
  (**inferred**) the same three the Claude adapter reads).

Cursor's camelCase event names map to the shared neutral names (`preToolUse` → `before-tool`, `sessionStart` →
`session-start`, `beforeSubmitPrompt` → `prompt-submit`, `preCompact` → `before-compact`, ...), so a hook counts
toward the same stage it would in Claude Code. All of a file's events are listed, including the Tab and
`workspaceOpen` ones. Scripts appear by file name only.

## MCP servers

`mcpServers` from `.cursor/mcp.json` (project) and `~/.cursor/mcp.json` (user) ([mcp](https://cursor.com/docs/context/mcp)).
A `url` server is `http` with its origin only; a `command` server is `stdio` with the command's file name only —
`args`, `env` and `headers` (with their `${env:...}` interpolations) are never read. Both files' servers
coexist name-by-name (the docs give no same-name merge across the two locations; neither shadows the other,
(**inferred**)).

## Env vars read

None. The adapter reads only files under the repo and the two config folders. No `.env`-style file, no app
database, no network.
