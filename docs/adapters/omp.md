# The oh-my-pi adapter: the rules it models

oh-my-pi (`omp`, github.com/can1357/oh-my-pi, npm `@oh-my-pi/pi-coding-agent`) is a fork of Pi with its own folders
(`~/.omp`, `.omp/`). Every rule below is from its source and docs, read on 2026-11-07 at release 18.18.0 (commit
`e95a8e7`). Paths in parentheses are under `packages/coding-agent/src/` unless they start with `docs/`. Where the code
and the docs disagree, the adapter follows the code and this file says so. OMP releases almost daily and its changelog
lists breaking changes: when it changes, change this file and the adapter together.

## What makes OMP different: it reads other harnesses' files

OMP's discovery providers (`discovery/*.ts`) read other tools' files as well as its own. In a repo, their project files
load **by default**: `.claude/`, `.cursor/rules`, `.github/copilot-instructions.md`, `.mcp.json` and so on. Their home
folders (`~/.claude`, `~/.codex`, `~/.gemini`, `~/.cursor`, OpenCode, Windsurf, `~/.copilot`) load only when
`enabledProviders` names them (or `*`/`all`; Claude also when `CLAUDE_CONFIG_DIR` is set). `.agent`/`.agents` and
OMP's own folders are on by default at both scopes (`capability/index.ts`, `isUserSourceEnabled`). `disabledProviders`
turns a provider off at both scopes.

So each provider's files get a layer of their own: `project` and `user` are OMP's own, and `claude:project`,
`cursor:user`, `agents:project` and so on are what OMP imports. An item in `claude:project` is a Claude Code file as
OMP reads it, not something counted twice. Layers are ranked by provider priority (native 100, claude 80, agents and
codex 70, gemini 60, opencode 55, cursor and windsurf 50, cline 40, github 30, vscode 20, standalone `AGENTS.md` and
`CLAUDE.md` 10, `contextFiles.extra` 9, root `mcp.json` and auto-learned skills 5), ties in registration order.

Within a capability, items are gathered in provider order and, inside a provider, in the order it emits them; the first
of a key wins (`capability/index.ts`).

## Paths

- Agent directory: `~/<cfg>/agent`, `<cfg>` being `.omp` or `$PI_CONFIG_DIR`; a named profile (`OMP_PROFILE`, else
  `PI_PROFILE`; empty or `default` is none) uses `~/<cfg>/profiles/<name>/agent`; without a profile,
  `$PI_CODING_AGENT_DIR` moves it (`utils/src/dirs.ts`). Task agents and the user's `APPEND_SYSTEM.md` ignore
  `PI_CODING_AGENT_DIR`.
- Project folders are always `.omp`. OMP's own project files are read from the repo's `.omp/` only when it is
  non-empty and the repo is not the home.
- Claude's home is `$CLAUDE_CONFIG_DIR` or `~/.claude` (`config/claude-paths.ts`); Copilot's is `$COPILOT_HOME` or
  `~/.copilot`. `CODEX_HOME` and `XDG_CONFIG_HOME` are not read by OMP.
- The repo root is the nearest folder holding `.git` (`capability/fs.ts`).

## Settings

The global layer is `<agent>/config.yml` (else `config.yaml`). The project layer merges, in this order, `.omp/settings.json`,
`.omp/config.yml`, then every foreign project settings file (`.claude/settings.json`, `.codex/config.toml`,
`.gemini/settings.json`, `opencode.json[c]`, `.opencode/opencode.json[c]`, `.cursor/settings.json`), **the later file
winning a key**, with `.omp/config.yml`'s `modelRoles` put back on top (`config/settings.ts`). The docs say the
higher-priority provider wins; the code does the opposite, and rigswig follows the code. Objects merge key by key;
arrays and scalars are replaced whole, so a project `disabledProviders` replaces the global list.

Settings shown: `model` (`modelRoles.default`), `approvalMode` (`tools.approvalMode`, default `yolo`),
`enabledProviders`, `disabledProviders`, `contextFiles.extra`, `skills.enabled`, `mcp.enableProjectConfig`.

## Context files

`docs/context-files.md`, `capability/context-file.ts`, `discovery/helpers.ts` (`loadStandaloneContextFiles`).

- User candidates: `<agent>/AGENTS.md`; `~/.claude/CLAUDE.md`; `~/.agent/AGENTS.md` and `~/.agents/AGENTS.md`;
  `~/.codex/AGENTS.md`; `~/.gemini/GEMINI.md`; `~/.config/opencode/AGENTS.md`; `~/.copilot/copilot-instructions.md`.
  **One user file** is kept across all of them, the highest priority winning.
- Project candidates: OMP's `.omp/AGENTS.md` from the **nearest non-empty `.omp/`** walking up to the repo root (a
  nearer `.omp/` without the file blocks a farther one); `.claude/CLAUDE.md`, `.gemini/GEMINI.md` and
  `.github/copilot-instructions.md` in the repo only; `.agent/AGENTS.md` and `.agents/AGENTS.md` up the walk; standalone
  `AGENTS.md` and `CLAUDE.md` from the repo up to the git root and, for a repo under the home, on up to (not including)
  the home. Folders whose names start with `.` hold no standalone file.
- **One project file per depth** (the repo is depth 0, its parent 1, ...; a config folder counts as its parent's
  depth), the higher-priority provider winning; standalone `AGENTS.md` beats `CLAUDE.md` at a tie. Files at different
  depths all load.
- Empty files contribute nothing and claim no slot.
- `contextFiles.extra` names (such as `AGENTS.local.md`) load beside the slot winner on the same walk, plus the agent
  directory when the list is not the project's.
- `disabledExtensions: context-file:<level>:<basename>` drops a file before the slots are decided, so the file it
  shadowed loads instead.
- A farther file whose paragraphs a closer file holds as one contiguous run is dropped (`context-contained`).
- Order: the user file, then farthest to closest. `@path` imports expand inline, up to five hops; rigswig lists each as
  an item imported by its file and follows them only into the repo and the home.
- `RULES.md` in the agent directory and the nearest non-empty `.omp/` is an always-apply rule: its whole body rides in
  every request.

## System prompt files

`docs/system-prompt-customization.md`, `capability/system-prompt.ts`, `system-prompt.ts`. One `SYSTEM.md` (custom
instruction block) or `SYSTEM_TEMPLATE.md` (whole template) is used, picked as project text, project template, user text,
user template, each from the highest-priority provider that has it (native's nearest `.omp/` and agent directory, then
`.claude`, `.agent(s)`, `.codex`, `.gemini`, Gemini's text file being `system.md`). `APPEND_SYSTEM.md`: the first that
exists of the repo's `.omp`, `.claude`, `.codex`, `.gemini`, then the user's OMP folder and opted-in homes (`main.ts`).

## Rules

`capability/rule-buckets.ts`. Rule files: `.omp/rules/*.{md,mdc}` and the agent directory's; `.agent(s)/rules` up the
walk and in the home; `~/.cursor/rules` (opt-in, and read **before** the repo's, so the user's wins a name) and
`.cursor/rules`; Windsurf's `global_rules.md` (opt-in) and `.windsurf/rules/*.md`; the first `.clinerules` walking up to
the filesystem root (a file or a folder of `.md`); `.github/instructions/**/*.instructions.md`. The first of a name wins.

- `alwaysApply: true`: the whole body, every request (startup).
- A `description`: a rulebook line (name, globs, description) at startup; the body is read on demand.
- A TTSR `condition`, `astCondition` or `question` (with `ttsr.enabled`): fires mid-session (`on-match`).
- None of these: **never in the prompt**. rigswig lists it as not effective and warns `rule-dropped`. A plain
  `.clinerules` or Windsurf rule file is such a rule.
- GitHub `applyTo` of `*`, `**` or `**/*` is always-apply; other globs make a rulebook line described as "GitHub
  Copilot instructions for <globs>".
- `.cursorrules` and `.windsurfrules` are **not** read, despite the provider descriptions saying so.

## Skills

`extensibility/skills.ts`, `docs/skills.md`. `<root>/<name>/SKILL.md`, **one level**, in this order:
`skills.customDirectories`; `.omp/skills` up the walk and `<agent>/skills` (description required); `~/.claude/skills`
(opt-in, or `skills.enableClaudeUser`) then `.claude/skills` up the walk; `.agent(s)/skills` up the walk then in the
home; `~/.codex/skills` (opt-in or `skills.enableCodexUser`) and `.codex/skills`; OpenCode's; `.github/skills`
(description required); then auto-learned skills (`<agent>/managed-skills`), which never win.

A second skill of a name is **not shadowed** when it differs: it loads as `<namespace>/<name>` (the folder before
`skills/`, or the provider id for a dot-folder: `.claude/skills/x` becomes `claude/x`), with `~2`... on a further
clash, and rigswig warns `skill-name-collision`. An identical copy is dropped. `enabled: false`, `disabledExtensions`
`skill:<name>`, `skills.ignoredSkills` and `skills.includeSkills` (globs) and the `skills.enable*` toggles apply.
`hide: true`, `disable-model-invocation: true` and `skills.manualSkills` keep a skill out of the listing.

The listing at startup is each skill's name and its description cut to a 100-character preview
(`extensibility/skill-descriptions.ts`).

## Commands and prompt templates

`extensibility/slash-commands.ts`: `.omp/commands/*.md` then the agent directory's; `~/.claude/commands/**` (opt-in or
`commands.enableClaudeUser`) then `.claude/commands/**` (recursive); `.agent(s)/commands`; Codex's and OpenCode's. The
first of a name wins. Prompt templates OMP expands are `<agent>/prompts/**/*.md` then `.omp/prompts/**/*.md`, so the
user's wins (`config/prompt-templates.ts`); `.github/prompts` and the other providers' prompt folders feed only a UI
listing. Nothing of either reaches the model until one is typed.

## Task agents

`task/discovery.ts`: the nearest `.omp/agents` walking up to the filesystem root, then the user's `agents/`, then
plugins and the bundled agents (`scout`, `reviewer`, `security-reviewer`, `seance`, `task`, `sonic`). The first of a
name wins; `name` and `description` are required; `main` and `sub` are reserved. The task tool lists each agent's name
and description up front. `.claude/agents`, `.codex/agents` and `.gemini/agents` are **not** read.
`task.disabledAgents` turns agents off.

## MCP servers

`mcp/config.ts`, `capability/mcp.ts`. Read in provider order: `.omp/mcp.json`, `.omp/.mcp.json` and the agent
directory's two; Claude's first of `.claude/.mcp.json` / `.claude/mcp.json` with servers, then the first of
`~/.claude.json` (top-level `mcpServers` only) / `~/.claude/mcp.json` (opt-in); `.codex/config.toml` and the home's;
`.gemini/settings.json` and the home's; OpenCode's `mcp` maps; `.cursor/mcp.json` and the home's; Windsurf's;
`.vscode/mcp.json`; and the repo's root `mcp.json` and `.mcp.json` last.

The **first definition of a name wins, and a disabled one still claims it**. `mcp.enableProjectConfig: false` drops
every project-level server. `disabledServers` and `enabledServers` are top-level lists in the agent directory's
`mcp.json`, not settings keys. OMP reads `.vscode/mcp.json` from `mcp.servers` (with `transport`), so a standard VS
Code file with top-level `servers` loads nothing: rigswig warns `vscode-mcp-not-read`.

## Hooks

`hooks/pre/*` and `hooks/post/*` in `.omp/` and the agent directory, in `.claude/` and the opted-in Claude home, and
Codex's `hooks/(pre|post)-<tool>.(ts|js)`. The tool is the file name without its extension. **Only `.ts` and `.js`
files run** (as extension modules; `extensibility/extensions/loader.ts`); others are listed as not effective
(`hook-not-run`). OMP does not run Claude Code's `settings.json` hooks; rigswig warns `claude-hooks-ignored` when the
repo has some.

## Permissions

`tools.approvalMode` (`always-ask`, `always-ask-cached`, `write`, `yolo`; default **yolo**), `tools.approval.<tool>`
(`allow`, `deny`, `prompt`; merged per tool) and `tools.restrictedFolders` (every call into one prompts, even in yolo),
from `tools/settings.ts`.

## Env vars read

`PI_CODING_AGENT_DIR`, `PI_CONFIG_DIR`, `OMP_PROFILE`, `PI_PROFILE`, `CLAUDE_CONFIG_DIR`, `COPILOT_HOME`. `.env` files,
which OMP also reads for these, are never opened: they hold secrets.

## What is not modelled

Listed in the output's `coverage.notModelled`: plugins and packages (omp-plugins, the marketplace, Agent Plugins,
Skillshare, Claude Code's own plugins) and everything they bring; extensions and custom tools; config overlays, runtime
overrides, settings environment variables, `.env` files and flags; path-scoped provider lists; equivalent-connection MCP
shadowing and the servers OMP drops; session-dependent rules and skills and the model-compressed skill descriptions;
gitignore filtering, temporary-folder ceilings, WSL and `COPILOT_CUSTOM_INSTRUCTIONS_DIRS`; and containment counted
before import expansion.

Session logs (`<agent>/sessions/`, JSONL) are not read.
