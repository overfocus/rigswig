# The Claude Code adapter: the rules it models

Every rule below is from Claude Code's documentation, read on 2026-11-05 (the subfolder rules on 2026-11-06). Where
the docs are silent and the adapter has to choose, the rule is marked **inferred** and listed in the output's
`coverage.notModelled`. When the docs change, change this file and the adapter together.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `managed` (enforce) | 0 | `managed-settings.json`, then `managed-settings.d/*.json` in alphabetical order, in the system folder |
| `session` | 1 | command-line flags and the shell environment: only `ANTHROPIC_MODEL` is read |
| `local` | 2 | `.claude/settings.local.json`; local MCP servers in `.claude.json` under the project's path |
| `project` | 3 | `.claude/settings.json`, `.mcp.json`, `.claude/` folders, `CLAUDE.md` files |
| `user` | 4 | `<config>/settings.json`, `<config>/` folders, `.claude.json` |
| `plugin:<name>@<marketplace>` | 5+ | an enabled plugin's folder |

The system folder is `/Library/Application Support/ClaudeCode` (macOS), `/etc/claude-code` (Linux and WSL) or
`C:\Program Files\ClaudeCode` (Windows); the legacy `C:\ProgramData` path is not read
([managed settings](https://code.claude.com/docs/en/managed-settings.md)). Drop-in files merge after the main file:
single values replace, lists combine without duplicates, nested blocks merge key by key.

`<config>` is `~/.claude`, or `CLAUDE_CONFIG_DIR` when set; `.claude.json` moves into it too
([.claude directory](https://code.claude.com/docs/en/claude-directory.md)).

Precedence: managed, command line, local, project, user
([settings](https://code.claude.com/docs/en/settings.md)). Lists such as `permissions.allow` combine across files
instead of overriding.

## Settings

- **Model.** `ANTHROPIC_MODEL` is read before the `model` key from any file, a managed one included; `--model` beats
  both ([settings reference](https://code.claude.com/docs/en/settings-reference.md),
  [env vars](https://code.claude.com/docs/en/env-vars.md)). A settings `env.ANTHROPIC_MODEL` is taken to beat the
  shell's value (**inferred**: settings `env` entries are written into the process environment).
- **Permissions.** `allow`, `ask` and `deny` lists combine from every layer; at run time deny is checked first, then
  ask, then allow. Each rule is an item; `defaultMode` is a setting.

## Instructions

- Managed `CLAUDE.md` in the system folder; `<config>/CLAUDE.md`; `CLAUDE.md` and `CLAUDE.local.md` in the working
  directory and every folder above it (root first, `.local` after); `.claude/CLAUDE.md`
  ([memory](https://code.claude.com/docs/en/memory.md)).
- `@path` imports: relative to the importing file, at most four hops, skipping code spans and fenced blocks. rigswig
  follows them only inside the repo, the config folder and the home folder's `~/` form inside those.
- `AGENTS.md` is read only when there is no `CLAUDE.md`, `.claude/CLAUDE.md` or `CLAUDE.local.md` in the working
  directory or above it (Claude Code 2.1.277 or later).
- Rules: every `.md` under `.claude/rules/` and `<config>/rules/`. Without `paths` front matter a rule loads at launch;
  with it, when a matching file is read or edited (`load.when: on-match`).
- `CLAUDE.md` and `CLAUDE.local.md` in subfolders below the working directory load on demand, "once Claude reads,
  writes, or edits another file in that subdirectory" ([memory](https://code.claude.com/docs/en/memory.md)):
  `load.when: on-match`, `appliesTo: ["<subfolder>/**"]`, and what they `@import` loads with them. `AGENTS.md` and
  `.claude/rules` in subfolders: not modelled (the docs do not say when they load).
- **Subfolder search.** One walk of the repo's folders finds the subfolder files and skills. It skips dot folders,
  dependencies and build output (`node_modules`, `target`, `dist`, `build`, `out`, `vendor`, `coverage`,
  `__pycache__`, `venv`, `Pods`, `DerivedData`), never follows a link, goes 12 folders deep, and stops after 20,000
  folders with a note in `coverage.notModelled`.
- **Auto memory** ([memory](https://code.claude.com/docs/en/memory.md), "Auto memory"): the first 200 lines of
  `MEMORY.md`, or its first 25 KB, whichever comes first, load at the start of every conversation; topic files beside
  it are read on demand. The folder is `<config>/projects/<project>/memory/`, where `<project>` comes from the git
  repository, so worktrees share the main checkout's folder; `CLAUDE_CODE_PROJECT_DIR_NAME` names it instead.
  `autoMemoryDirectory` (any settings scope, absolute or `~/`) moves it; `autoMemoryEnabled: false` or
  `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` turns it off. How the path becomes `<project>` is not documented: rigswig turns
  every character that is not a letter or digit into `-`, as Claude Code's own folders show (**inferred**). The
  workspace-trust rule for a repo's `autoMemoryDirectory` is not modelled.

## Agents, skills and commands

- **Agents** ([subagents](https://code.claude.com/docs/en/sub-agents.md)): managed, then `--agents`, then project,
  then `<config>/agents/`, then plugins. On a name clash the higher one wins. An omitted `tools` means every tool.
  - `.claude/agents/` and `<config>/agents/` are scanned recursively, and a subfolder does not change the name, which
    comes only from the `name` field (the file name when there is none). Two files with one name in one tree: one
    loads, "chosen by filesystem read order"; rigswig keeps the first it reads and warns.
  - Project agents come from every `.claude/agents/` between the working directory and the git root; "the definition
    closest to the working directory" wins, and the farther one is marked overridden. `.claude/agents` folders below
    the working directory: not modelled (the docs do not say).
  - Plugin agents are namespaced `plugin:agent`, and a plugin's subfolders add to the name
    (`agents/review/security.md` is `plugin:review:security`).
- **Skills** ([skills](https://code.claude.com/docs/en/skills.md)): enterprise over personal (`<config>/skills/`) over
  project (`.claude/skills/`); plugin skills are namespaced and never clash. At startup only the listing loads: the name
  and the description plus `when_to_use`, cut at 1,536 characters. The body loads when the skill is invoked.
  Skills in a subfolder's `.claude/skills/` (`<subdir>/.claude/skills/<name>/SKILL.md`) "don't load at startup": they
  load "the first time Claude reads or edits a file in that subdirectory" (`load.when: on-match`), named after the
  folder (`apps/web:deploy`), and both they and a root skill of the same name load. Skills nested deeper than
  `skills/<name>/SKILL.md`, and command subfolders: not modelled (the docs do not say).
- **Commands** are skills now: `.claude/commands/x.md` and `.claude/skills/x/SKILL.md` both make `/x`, and on a clash
  the skill wins. Between a user and a project command of the same name, the skills order is used (**inferred**).
- The enterprise skills and agents folders are taken to be `.claude/skills` and `.claude/agents` inside the system
  folder (**inferred**: the docs give the folder's name, not its full path).

## Hooks

([hooks](https://code.claude.com/docs/en/hooks.md))

- From every settings layer and every enabled plugin (`hooks/hooks.json` plus the manifest's `hooks`).
- `disableAllHooks` takes the value left after precedence. It cannot disable managed hooks unless set in managed
  settings.
- `allowManagedHooksOnly` (managed) blocks user, project, local and plugin hooks, except hooks of plugins that managed
  `enabledPlugins` force-enables.
- Handler types: `command`, `http`, `mcp_tool`, `prompt`, `agent`. rigswig shows a command's script by file name and an
  HTTP hook's scheme and host only.
- Hooks in skill or agent front matter: not modelled.
- A SessionStart hook's output joins the context before the first prompt; rigswig never runs hooks, so each effective
  SessionStart hook is named in `context.notMeasured`.

## MCP servers

([MCP](https://code.claude.com/docs/en/mcp.md), [managed MCP](https://code.claude.com/docs/en/managed-mcp.md))

- Local: `.claude.json` under the project's path (looked up by the repo's path and its real path); project: `.mcp.json`;
  user: `.claude.json` top-level `mcpServers`; plugins: `.mcp.json` and the manifest's `mcpServers`.
- On a name clash: local, then project, then user, then plugin. The whole entry from the winner is used.
- A `.mcp.json` server is `disabled` if any settings file (or the project's `.claude.json` entry) lists it in
  `disabledMcpjsonServers`; else `active` if `enableAllProjectMcpServers` (highest layer that sets it) is true or it is
  in any `enabledMcpjsonServers`; else `pending`. An untrusted folder ignores the project's own approvals: folder trust
  is not modelled.
- `managed-mcp.json` in the system folder takes exclusive control: only its servers load.
- `deniedMcpServers` merge from every layer and win; `allowedMcpServers` merge too (unless managed
  `allowManagedMcpServersOnly`); unset allows all, an empty list allows none. Entries match `serverName`, `serverUrl`
  (with `*`) or `serverCommand` (the exact command and arguments).
- Tool listings are not read: MCP context cost is `unknown`, and each effective server is named in
  `context.notMeasured` (a server with hundreds of tools adds a long listing even when its tools load on demand).

## Plugins

([plugin loading](https://code.claude.com/docs/en/plugins/loading.md),
[plugin manifest](https://code.claude.com/docs/en/plugins-reference.md))

- `enabledPlugins["<name>@<marketplace>"]`: the highest layer that mentions the id decides; managed `true`
  force-enables and `false` blocks.
- Folder: a `directory` marketplace's plugin is read from the marketplace folder (a differing install cache is reported
  stale and ignored); otherwise the install cache from `installed_plugins.json`.
- Manifest paths: `agents` and `commands` replace their default folders; `skills` adds to `skills/`; `hooks` loads with
  `hooks/hooks.json`; `mcpServers` with `.mcp.json` (a later name replaces an earlier one). Every path must stay inside
  the plugin folder, symlinks included; an escaping path is dropped with a warning.

## Session logs (`rigswig usage`, `audit --usage`)

Opt-in per run and metadata only. [USAGE.md](../USAGE.md) lists every field read.

- **Where they are:** one JSON Lines file per session under `<config>/projects/<project folder>/`, the file a hook
  receives as `transcript_path` ([hooks](https://code.claude.com/docs/en/hooks.md)), plus each session's
  `subagents/*.jsonl`. The folder name follows the same inferred rule as auto memory (non-alphanumerics become `-`).
  Folders whose name starts with the repo's are read too, and a line counts only when its `cwd` is inside the repo.
- **How long they last:** `cleanupPeriodDays` deletes them, 30 days by default
  ([settings](https://code.claude.com/docs/en/settings.md)).
- **The line format is not documented.** These shapes are inferred from the files Claude Code writes:
  - a response's tokens are repeated on every line of that response, so they are counted once per `message.id`;
  - a resumed session writes calls again, so they are counted once per call id;
  - `Agent` (earlier `Task`) with no `subagent_type` is `general-purpose`;
  - MCP tools are named `mcp__<server>__<tool>`, and a plugin's server arrives as `plugin_<plugin>_<server>`.
- **Not modelled:** skills and commands typed as `/name` (they are in message text, which is not read), hook runs,
  and sessions whose working folder is outside the repo.
