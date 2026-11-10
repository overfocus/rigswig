# The Pi adapter: the rules it models

Pi is the `pi` coding agent (github.com/earendil-works/pi, formerly badlogic/pi-mono; npm
`@earendil-works/pi-coding-agent`). Every rule below is from its source and docs in `packages/coding-agent`, read on
2026-11-07 at release 1.5.0 (commit `6860f03`): `src/core/resource-loader.ts`, `src/core/package-manager.ts`,
`src/core/skills.ts`, `src/core/trust-manager.ts`, `src/core/settings-manager.ts`, and the pages under `docs/`
(configuration, settings, security, skills, prompt-templates, packages, mcp). Where the source is silent and the adapter
has to choose, the rule is marked **inferred**. Pi releases often and its changelog still lists breaking changes: when
it changes, change this file and the adapter together.

Pi has no subagents (by design), no hook configuration (hooks are extension events), and no permission rules (it does
not ask before tool calls), so this adapter reports no agents, hooks or permissions.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `project` | 0 | the repo's `.pi/` (behind project trust), its `.agents/skills` chain, and the context files from the filesystem root down to the repo |
| `user` | 1 | the agent directory: `~/.pi/agent`, or `$PI_CODING_AGENT_DIR`; and `~/.agents/skills` |
| `plugin:<id>` | 2+ | each package in a `packages` setting, where it is installed |

Project settings deep-merge over user settings (`settings-manager.ts`), and project resources come before user ones,
which come before packages' (`resourcePrecedenceRank` in `package-manager.ts`).

## Project trust

`docs/security.md` and `trust-manager.ts`. Pi asks for trust only when the project has a gated resource: one of
`.pi/settings.json`, `mcp.json`, `extensions`, `skills`, `prompts`, `themes`, `SYSTEM.md`, `APPEND_SYSTEM.md`, or an
`.agents/skills` folder in the repo or any ancestor (not the user's own `~/.agents/skills`). Then:

1. a saved decision in `<agent>/trust.json` for the repo or its nearest ancestor (`true` or `false`) applies;
2. otherwise the user-only `defaultProjectTrust` setting: `always` trusts, `never` does not, and `ask` (the default)
   means Pi asks at startup.

Without trust, the project's settings, MCP servers, skills, prompts, system files and `.agents/skills` do not load:
rigswig lists them as not effective and warns `project-untrusted`. When Pi would ask, rigswig counts them as if trusted
and warns `project-trust-undecided` (**inferred**: the interactive answer is unknown, and counting them is the larger
bill). The decision is the `projectTrust` setting. Context files load whatever the trust state.

## Settings

`model` (`defaultModel`), `provider` (`defaultProvider`), `thinkingLevel` (`defaultThinkingLevel`), `defaultTools`,
`enableSkillCommands`, `defaultProjectTrust` (read from the agent directory only, as the docs require) and
`projectTrust`, each with the layers that set it. `auth.json`, `mcp-auth.json` and `models.json` are never opened.

## Instructions

`loadProjectContextFiles`: in each directory the first of `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`,
`CLAUDE.MD` that is a file loads; the others there are listed as shadowed by it. The agent directory's file comes
first, then one per directory from the filesystem root down to the working directory (the repo). They are joined in
full: there is no size budget.

`SYSTEM.md` replaces Pi's system prompt and `APPEND_SYSTEM.md` is added to it. For each, the project's `.pi/` copy
(behind trust) wins over the agent directory's; they are not combined.

## Skills

The Agent Skills format: `SKILL.md` with `name` and `description` front matter. The listing (name, description and
the file's path) goes into the system prompt at startup; the body loads when the skill is used. A skill without a
description is not loaded; one with `disable-model-invocation: true` is left out of the listing and runs only as
`/skill:<name>`. The name falls back to the folder's name.

Discovery (`collectSkillEntries`): a folder that holds a `SKILL.md` is one skill and is not walked further; otherwise
its subfolders are walked, and loose `.md` files count at the top of a Pi skills folder, or below the top of an Agent
Skills folder (`.agents/skills`). `node_modules` and dot-folders are skipped.

Sources, in precedence order; the first skill of a name wins and Pi warns (`skills.ts`):

1. plain paths in the project's `skills` setting (relative to `.pi/`);
2. `.pi/skills`;
3. `.agents/skills` in the repo and each ancestor up to the git root;
4. plain paths in the user's `skills` setting (relative to the agent directory);
5. `<agent>/skills`;
6. `~/.agents/skills`;
7. packages.

Pi does not read `.claude/skills` or `.codex`.

## Prompt templates

`docs/prompt-templates.md`: the `.md` files directly in `.pi/prompts` (behind trust) and `<agent>/prompts`, plus plain
paths in the `prompts` settings and packages, in the skills' order. The file name is the command; the first of a name
wins. Nothing loads until one is invoked, and then its body does, so they are commands measured on invoke.

## MCP servers

`docs/mcp.md`: `<agent>/mcp.json` and `.pi/mcp.json` (behind trust), `{"mcpServers": {...}}`. A project entry replaces
the user's server of the same name; a project entry with no `command`, `url` or `type` only overrides `enabled`,
`exposure` and `toolExposure`. Names that differ only in `-` and `_` are one server. `enabled: false` turns a server
off; SSE is rejected. The servers run in the built-in `builtin:mcp` extension, so `-builtin:mcp` in an `extensions`
setting turns them all off. A server's `description` is its line in the system prompt and is measured; its tool
listing is not. Targets keep only a URL's scheme and host or a command's file name.

## Packages

`docs/packages.md`: the `packages` setting of each scope lists `npm:`, `git:` (or a URL) and local-path sources. The
same package in both scopes is the project's. rigswig reads an installed package where Pi installs it:
`<scope>/npm/node_modules/<name>` and `<scope>/git/<host>/<path>`, `<scope>` being the agent directory or `.pi`; a
local path is read where it points. A package's skills and prompts come from the plain paths its `package.json` lists
under `pi`, else from its `skills/` and `prompts/` folders. The object form's empty `skills` or `prompts` list turns
that kind off. A package in settings but not installed is warned (`incomplete-package-not-installed`): Pi installs it at startup.

## Env vars read

`PI_CODING_AGENT_DIR` (the agent directory). Nothing else: API keys and provider settings are never read.

## What is not modelled

Listed in the output's `coverage.notModelled`: extensions (executable TypeScript, listed by nobody but Pi itself, whose
tools, commands, event handlers and prompt changes are only known by running them, including the built-in
`builtin:codemode`, `builtin:tool-search` and `builtin:llama.cpp`); trust decided by an extension or the
`--approve`/`--no-approve` flags; the command-line flags; glob and `!`/`+`/`-` patterns in the resource settings and
packages' object-form filters other than an empty list; `.gitignore`-style files inside skill folders; a nested
worktree's shadowing of the main repo's context file; MCP tool exposure and OAuth; packages in the legacy global npm
root and temporary `-e` installs; themes, keybindings and models.

Session logs (`~/.pi/agent/sessions/`, JSONL with the working folder and per-message token usage) are not read yet:
see the roadmap.
