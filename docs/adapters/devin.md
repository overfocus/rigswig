# The Devin adapter: the rules it models

Every rule below is from Cognition's Devin CLI documentation at docs.devin.ai — `cli/enterprise/system-config.md`,
`cli/extensibility/rules.md`, `cli/extensibility/hooks/lifecycle-hooks.md`, `cli/subagents.md` and
`cli/extensibility/plugins/overview.md` — read on 2026-11-06, against the release the docs describe (v3000.3,
"Local 3.6"). Where the docs are silent and the adapter has to choose, the rule is marked **inferred** and listed in
the output's `coverage.notModelled`. When the docs change, change this file and the adapter together.

The Devin CLI is a local agent that reads its policy from the machine: one administrator file, one user config
directory, a second home-folder rules location, the repo's `.devin/` directory and gitignored personal overrides.
Its configs are JSON with `//` and `/* */` comments, which the adapter parses as JSONC. The CLI also imports the
config of sibling tools in the same checkout — the AGENTS.md family, Claude Code, Cursor, Windsurf, Copilot,
OpenCode and Zed — through `read_config_from`, which is **all on by default**; the imported files are modelled as
their own layers, so a checkout's totals show what Devin would actually load.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `managed` | 0 | the system policy: `/etc/devin/system.json` on Linux (macOS: `/Library/Application Support/Devin/system.json`; Windows: `C:\ProgramData\Devin\system.json`) |
| `user` | 1 | the user config directory: `~/.config/devin/` on Linux (macOS: `~/.devin/`; Windows: `%APPDATA%\Devin\`) — `config.json`, `mcp_config.json`, `AGENTS.md`/`AGENT.md`, `skills/`, `agents/` |
| `home_rules` | 2 | `~/.devin/` on every OS: `global_rules.md`, `rules/*.md`, the AGENTS.md family |
| `project` | 3 | the repo's `.devin/`: `config.json`, `mcp_config.json`, `hooks.v1.json`, `global_rules.md`, `rules/*.md`, `skills/`, `agents/` |
| `local` | 4 | `.devin/config.local.json` and `.devin/mcp_config.local.json` (gitignored personal overrides) |
| `imports_project` | 5 | the repo's other tools' files the import reads: `CLAUDE.md`, `.claude/`, `.windsurf/`, `.cursor/`, `.agents/skills/`, `opencode.json`, `.zed/`, `.mcp.json` |
| `imports_user` | 6 | the home's other tools' files: `~/.claude.json`, `~/.claude/CLAUDE.md`, `~/.agents/skills/`, `~/.copilot/skills/`, `~/.codeium/<channel>/`, `~/.config/opencode/opencode.json`, `~/.config/zed/settings.json` |

Ranks follow the docs' precedence for settings (org policy above session grants above local above project above
user — [system config](https://docs.devin.ai/cli/enterprise/system-config.md)); the in-memory session grants are
rank 2 in those docs but never live on disk, so they are named in `coverage.notModelled`. The two `imports_`
layers rank below every Devin-native one (**inferred**: the docs give MCP merges "higher-priority source wins"
among Devin's own layers and say imported files join the same mechanisms; rigswig ranks them last so a native file
always outranks its imported twin, and lists this among the not-modelled notes).

## Settings

From the JSONC configs, flattened with dot names and carrying their full trail:

- `model`, `subagent` (`agent.model`, `agent.subagent_model`), `permissionMode` (`agent.permission_mode`),
  `theme` (`agent.theme_mode`), `subagentsEnabled` (`agent.subagents_enabled`), and the sandbox block:
  `sandbox` (`agent.sandbox`: `network_mode`, `allowed_domains` — the domain list is printables, not URLs).
- `readConfigFrom.<key>` for each `read_config_from` switch (`agents_standard`, `claude`, `cursor`, `windsurf`,
  `copilot`, `opencode`, `zed`, `cloud`): a boolean carrying its trail, so a checkout shows who turned a source
  off. Unset means on.
- `policy`: the managed policy's `enterprise_host` reduced to its host (credentials, scheme and path dropped) when
  a system policy exists; the trail carries every top-level system key with its strings redacted (URLs keep their
  origin only). The system `proxy` object survives with its URLs origin-only and its port dropped.
- Project config keys the CLI ignores there (`agent`, `permission_mode`, `theme_mode`, `sandbox`,
  `subagents_enabled`, `attribution`, `proxy`) get a `project-key-ignored` warning.

## Instructions

- `AGENTS.md` and `AGENT.md` at a level load additively as startup instructions; both are read when both exist
  (**inferred**: the docs list both spellings everywhere without a precedence).
- The project standard family is `AGENTS.md`, `AGENTS.local.md`, `AGENT.md` and `.windsurfrules` at the repo root,
  joined by `CLAUDE.md`, plus the Windsurf and Cursor rules folders imported by switch
  ([rules](https://docs.devin.ai/cli/extensibility/rules.md)); all startup.
- Home-folder rules (`~/.devin/global_rules.md`, `~/.devin/rules/*.md`) are startup; so is `.devin/rules/*.md` and
  `.devin/global_rules.md` at the repo root.
- Below the workspace root, everything is lazy: `AGENTS.md`-family and `CLAUDE.md` files join only when the agent
  first touches their directory. rigswig lists them as `load: on-invoke` and keeps them out of the startup total.
- `.devin/rules/*.md` below the root are listed `on-invoke` for the same reason; a rule's `trigger` frontmatter
  (`always_on`, `manual`, `model_decision`, `agent`, `glob`) matters only at the root, where `always_on` and
  `manual` are startup and the rest (with an unknown trigger) are on-invoke.
- `.windsurf/global_rules.md` gives way to a same-place `.devin/global_rules.md`
  ([rules](https://docs.devin.ai/cli/extensibility/rules.md)); when the `.devin` file exists the `.windsurf` file
  is not read and a `windsurf-global-rules-shadowed` warning is emitted.
- Cursor `.md`/`.mdc` rules map their frontmatter: `alwaysApply: true` to startup, `globs` to on-match with the
  pattern copied into `applies_to`, a description alone to on-invoked
  (**inferred**: the docs import Cursor rules wholesale; the mapping follows the Claude adapter's).

## Skills

`<dir>/<name>/SKILL.md` under `.devin/skills/` (project), `~/.config/devin/skills/` (user), `.claude/skills/**`
(imported, on by default), `.agents/skills/`, `.windsurf/skills/` and `~/.copilot/skills/`. Each is one skill item
with a character count (name + description measured at startup, body on invoke) and a content hash. Claude's
`<lang>.md` command files under `.claude/commands/` import as skills named for their file stem. Same names keep
their own items — the id carries the layer, so two `review` skills from Devin and Claude stay distinct.

## Agents (subagent profiles)

`<dir>/<name>.md` under `.devin/agents/` and `~/.config/devin/agents/`, one per name. Frontmatter: `name`,
`description`, `model`, `allowed-tools` (mapped to `ToolAccess` — an `allow` list for the deny list, otherwise
Only these). Same-named profiles: the project one is effective and the user one is listed as shadowed
(**inferred**: the docs rank project above user for the same-named merge in general, and say nothing specific
about subagent profiles).

## Hooks

Sources, each event's list loaded additively ([lifecycle hooks](https://docs.devin.ai/cli/extensibility/hooks/lifecycle-hooks.md)):

- `.devin/hooks.v1.json` — the project hook file.
- the `hooks` key in `config.json` (user) and `config.local.json` (local).
- Claude Code's `.claude/settings.json` and `.claude/settings.local.json` (project and `~/.claude/`), plus
  `~/.claude.json`, when the Claude import is on — each hook there becomes an imported item.

Events keep the CLI's names, mapped to rigswig's neutral event names: `PreToolUse`, `PostToolUse`,
`UserPromptSubmit`, `Stop`, `SessionStart`, `SessionEnd` as-is; `PermissionRequest` becomes `permission-request`
and `PostCompaction` becomes `post-compaction`. Matcher text is printable; commands are paths or bare commands,
not URLs, so they pass through the redactor unchanged. Hooks that ask an LLM to decide (PermissionRequest
decisions) or rewrite a tool's input are listed, never evaluated.

## MCP servers

Sources: `.devin/mcp_config.json`, `.devin/mcp_config.local.json`, `~/.config/devin/mcp_config.json`, the
configs' `mcpServers` keys, and the imported tools' MCP shapes (`.mcp.json`, `opencode.json`'s `mcp`, `.cursor/`,
`.zed/`, and Claude's servers from `.mcp.json`/`~/.claude.json` when the import is on). Same-named servers merge
with the higher-priority layer winning; the losers are listed as shadowed, never deleted
([system config](https://docs.devin.ai/cli/enterprise/system-config.md)). Every server is one item with its own
id (`devin:mcp:<layer>:<name>`); a server's `disabled: true` or an importer's `enabled: false` (OpenCode's flag is
inverted) becomes `state: disabled`. URLs are redacted to their origin; command-shaped servers keep their argv.

## Permissions

The `permissions` key (`allow` and `deny` lists of `Tool(pattern)` strings) merges **across** layers — a rule from
any layer stays effective, deny and allow alike; there is no shadowing between layers
([system config](https://docs.devin.ai/cli/enterprise/system-config.md)). Each entry is one item whose id carries
its effect, tool and pattern.

## Env vars read

None. The adapter reads only files: the system policy, the user config directory, `~/.devin/`, the repo's
`.devin/`, and the imported tools' files. No network, no subprocesses, nothing outside the file system it is
handed.
