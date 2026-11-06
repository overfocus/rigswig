# The Gemini CLI adapter: the rules it models

Every rule below is from Gemini CLI's documentation at geminicli.com/docs — `cli/gemini-md`, `reference/memport`,
`reference/configuration`, `hooks`, `core/subagents`, `extensions/reference`, `reference/policy-engine` and
`cli/custom-commands` — read on 2026-11-06, for the installed CLI (the nested settings format, v0.3+). Where the
docs are silent and the adapter has to choose, the rule is marked **inferred** and listed in the output's
`coverage.notModelled`. When the docs change, change this file and the adapter together.

One standing fact: the docs record that Gemini CLI was folded into Antigravity CLI on June 18, 2026 (for the
unpaid tier and Google One users). The installed CLI's files are this adapter's subject; nothing here claims what
Antigravity reads. Gemini CLI is also not installed on a machine unless its `~/.gemini/` exists.

Gemini's settings precedence is the opposite of most tools': the four settings files override in order *system
settings file over project over user over system defaults file*, and then environment variables over all of them.
Its hooks (unlike its settings) are additive: every matching hook from every layer runs.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `system` | 0 | the system settings file: `/etc/gemini-cli/settings.json` (macOS: `/Library/Application Support/GeminiCli/settings.json`; Windows: `C:\ProgramData\gemini-cli\settings.json`), or `GEMINI_CLI_SYSTEM_SETTINGS_PATH` |
| `project` | 1 | the repo's `.gemini/` (`settings.json`, `commands/`, `agents/`) and the repo root's context files |
| `user` | 2 | `~/.gemini/`: `settings.json`, the global context file, `commands/`, `agents/` (**inferred**), `policies/`, `extensions/` |
| `extensions` | 3 | `~/.gemini/extensions/<name>/`: the manifest's `mcpServers`, its context file, `commands/`, `agents/`, `hooks/hooks.json`, `policies/` |
| `system_defaults` | 4 | the system defaults file: `/etc/gemini-cli/system-defaults.json` (same OS variants), or `GEMINI_CLI_SYSTEM_DEFAULTS_PATH` |

A same-named item wins at the higher-ranked layer; a settings file's MCP server beats an extension's (pinned) and
a workspace config beats an extension's (pinned), so the extensions layer ranks under both (the user-over-
extension order is **inferred**).

## Settings

From the JSONC-tolerant settings files, with the full trail, highest precedence first: `model`
(`model.name`), `sandbox` (`tools.sandbox`), `maxSessionTurns` (`model.maxSessionTurns`),
`contextFileName` (`context.fileName`, a string or an array as configured), `discoveryMaxDirs`
(`context.discoveryMaxDirs`) and `includeDirectories` (`context.includeDirectories`).

## Instructions (context files)

- The context file names in effect: `context.fileName` (a string or an array) from the project and user settings,
  `GEMINI.md` by default. Only configured names are read at all.
- The user's global file (`~/.gemini/<name>`) and the repo root's load every session. A file in a subdirectory is
  the docs' *just-in-time* context: discovered when the agent touches its directory, so it is listed
  `load: on-invoke` and stays out of the startup total. The walk probes at most `context.discoveryMaxDirs`
  directories (200 by default) and skips heavy and dot directories.
- `@path` imports in a context file are followed the way the memory import processor does: relative or absolute
  paths, nested imports, at most five hops deep (the processor's documented default), never twice the same file.
  Every imported file is its own item with `imported_by`, measured by its own characters. Imports only follow into
  the repo and `~/.gemini` (+ the extension's own directory for its context file): anything else gets an
  `import-outside` warning, and a missing file is skipped silently (the processor "fails gracefully").
- GEMINI.md files in directories **above** the repo root are not read (the CLI searches ancestor directories; see
  `coverage.notModelled`).

## Skills (custom commands)

A custom command is a TOML file with a `prompt`, the text sent when it runs, so commands are skills here. Files
under `.gemini/commands/` and `~/.gemini/commands/` are named by their path with subdirectories as `:` namespaces
(`git/commit.toml` → `git:commit`); the listing (stem + description) is measured at startup, the prompt body on
invoke. An extension's commands rank under the user's and the project's (the docs: extension commands conflict to
a prefixed name).

## Agents (subagents)

`<name>.md` under `.gemini/agents/` (**inferred** for the user level) and in an extension's `agents/` directory.
Frontmatter: `name` (the file stem by default), `description`, `tools` (the `tools` list → Only those, else
everything), `model`. `temperature`, `max_turns` and `kind` are not modelled. Same names: the project profile wins
over the user's and an extension's (**inferred**, on the workspace-beats-extension rule).

## Hooks

The `hooks` key in every settings file (system, project, user) plus each extension's `hooks/hooks.json`, all
**additive** — the docs merge across layers and every matching hook runs [hooks](https://geminicli.com/docs/hooks/).
The shape is Claude's (`{ Event: [ { matcher, hooks: [ ... ] } ] }`), and the events map onto the shared neutral
names: `BeforeTool` → `before-tool`, `SessionStart` → `session-start`, `PreCompress` → `before-compact`,
`BeforeAgent` → `before-agent`, and the rest from the lifecycle list. Commands appear by file name only; the CLI
fingerprints project hooks, which is a runtime trust mechanism, not load.

## MCP servers

`mcpServers` from the system, project and user `settings.json` and the system defaults file, plus each extension
manifest's `mcpServers` — same names shadow across layers with the settings files winning over extensions
(pinned). A `url` server is `http` with its origin only; a `command` server is `stdio` with the command's file
name only — `args`, `env` and `excludeTools` never surface.

## Permissions (the policy engine)

Policy TOML rules at the user tier (`~/.gemini/policies/*.toml`, tier 4) and in an extension's `policies/`
directory (tier 2) become permission items: `decision` maps to the effect (`allow`/`deny`/`ask_user`), `toolName`
(string or array — one item per tool, wildcards kept as written) names the tool, and the pattern is the rule's
`commandPrefix` or else `argsPattern`, printable only. Priority numbers, `subagent`-scoped rules and the policy
engine's runtime argument matching are not modelled, and the workspace tier is listed in `coverage.notModelled`
(the docs mark it currently disabled).

## Env vars read

`GEMINI_CLI_SYSTEM_SETTINGS_PATH` and `GEMINI_CLI_SYSTEM_DEFAULTS_PATH` (the two path overrides the configuration
docs name). Authentication and model env vars are never read.
