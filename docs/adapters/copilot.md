# The GitHub Copilot CLI adapter

Sources (GitHub's own docs, read 2026-11-06, against the v0.0.36x prerelease line):

- docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference (`~/.copilot` and its file
  table, `COPILOT_HOME`, the settings-file merge list)
- .../how-tos/copilot-cli/customize-copilot/give-copilot-instructions-and-repo-customizations (settings files,
  precedence)
- .../how-tos/copilot-cli/customize-copilot/add-mcp-servers (MCP files, transports, the workspace-over-user rule)
- .../how-tos/copilot-cli/customize-copilot/use-skills (skill folders, project over personal)
- .../how-tos/copilot-cli/customize-copilot/create-custom-agents-for-cli (`.agent.md`, IDs, tools)
- .../copilot/reference/custom-agents-configuration (frontmatter schema, personal-over-project precedence)
- .../copilot/how-tos/copilot-cli/customize-copilot/use-hooks (hook files, events, the settings `hooks` key)
- .../copilot/reference/custom-instructions-support (which instruction files the CLI reads)

Where the docs left a gap this adapter marks it **inferred**, listed at the end.

The config root is `~/.copilot` (`COPILOT_HOME` moves it). `config.json`, `permissions-config.json` and
`providers.json` are written by the CLI or hold credentials; the adapter reads none of them.

## Files it reads

| Layer | Path | Contents |
|---|---|---|
| project | `.github/copilot-instructions.md`, `.github/instructions/**/*.instructions.md` | the repository's instructions |
| project | `AGENTS.md`, `CLAUDE.md`, `GEMINI.md` at the repo root | the agent instruction family, additive |
| project | `.github/agents/**` | custom agents (`.agent.md`, subdirectory IDs joined `--`) |
| project | `.github/hooks/*.json` | hooks, flat event map |
| project | `.github/skills` | skills, `<name>/SKILL.md` |
| project | `.github/copilot/settings.json` | user-selectable settings |
| project | `.github/copilot/settings.local.json` | the same, personal |
| project | `.mcp.json`, `.github/mcp.json` | MCP servers (`mcpServers`) |
| imports | `.claude/settings.json`, `.claude/settings.local.json` | the cross-tool subset: hooks |
| user | `~/.copilot/copilot-instructions.md`, `~/.copilot/instructions/**/*.instructions.md` | personal instructions |
| user | `~/.copilot/agents/**` | personal agents |
| user | `~/.copilot/hooks/` | hook files, same shape |
| user | `~/.copilot/skills`, `~/.agents/skills` **inferred (second personal root's default path)** | skills |
| user | `~/.copilot/settings.json` | user settings (JSONC) |
| user | `~/.copilot/mcp-config.json` | user MCP servers |

Settings precedence (the merge list in the config-dir reference): defaults < MDM < user < repository <
local < environment < CLI flags; the modelled files cover the middle three. The adapter's settings trail runs
strongest-first. The `model` key is either a name or the object's `model.name`. Only `model`, `effortLevel`,
`allowedUrls` and `disabledSkills` are surfaced; they are the keys whose effect the settings reference describes.

MCP: the `local`/`stdio` type keeps the command's file name; `http`/`sse` keep the URL's origin. Args, env and
headers never surface. A name in a workspace file (`.mcp.json`, `.github/mcp.json`) outranks the user's
mcp-config.json (pinned in the MCP how-to). Loading priority beyond these files —
`--additional-mcp-config`, plugins, folder trust — is not modelled.

Skills: five roots, project first — `.github/skills`, repo `.claude/skills`, repo `.agents/skills`, then
`~/.copilot/skills`, `~/.agents/skills`; project beats personal on a shared name. Each skill is a folder with a
`SKILL.md`; the listing (frontmatter name + description) is measured; the body loads on invocation. The folder
is the layout; the CLI additionally accepts `.zip` archives, which are not modelled.

Agents: the ID is the file name without `.agent.md`/`.md` (a subdirectory's name joins with `--`), not the
`name` frontmatter. A personal agent beats a project agent on the same ID — inverted against most of the model,
pinned in the reference. `tools` without frontmatter or with an empty list means everything; the list keeps the
tools. Only `read`, `grep`-style aliases are recorded as seen. `disable-model-invocation` and `user-invocable`
are not modelled (invocation is not context size).

Hooks: `.github/hooks/*.json` and `~/.copilot/hooks/` files hold `{version, hooks: {event: [entry]}}`; each
entry is `{type: "command", bash, powershell, cwd, env, timeoutSec}` and only the script's file name surfaces.
The settings files' `hooks` key uses the same flat shape. Claude's settings files are read for the cross-tool
subset, their hooks in Claude's shape. Events map: `sessionStart`, `sessionEnd`, `preToolUse`, `postToolUse`,
`userPromptSubmitted`, `errorOccurred`; hooks are additive and run for every matching event.

Instructions: the personal `copilot-instructions.md` and the repository's `.github/copilot-instructions.md` load
every session; `.github/instructions/**.instructions.md` load for files matching their `applyTo` globs (string
or list); `~/.copilot/instructions/*.instructions.md` topic files load every session when they carry no
`applyTo` (their scoping is inferred from the same mechanism).

## Not modelled

The `not_modelled` block of a run (the machine's list) adds to anything common above: the CLI's built-in
components (system prompt, tools, model, the GitHub MCP server it ships); MDM managed settings; the
approval record `permissions-config.json`; BYOK `providers.json` (never read — credentials in plain text);
plugins and extensions; LSP config (`lsp-config.json`, `.github/lsp.json`); disabledSkills and folder-trust
filtering; GitHub.com-side (cloud) instructions, repository MCP and enterprise agents, which follow the repo
online, not the machine.
