# The Hermes Agent adapter: the rules it models

Every rule below is from Hermes Agent's documentation at hermes-agent.nousresearch.com/docs, read on 2026-11-06,
against the installed release 0.22.5. Where the docs are silent and the adapter has to choose, the rule is marked
**inferred** and listed in the output's `coverage.notModelled`. When the docs change, change this file and the
adapter together.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `managed` (enforce) | 0 | `config.yaml` in the managed directory: `/etc/hermes` on POSIX, or `$HERMES_MANAGED_DIR` |
| `session` | 1 | command-line flags (`--profile/-p`, `--model`, `--ignore-rules`) are not read; the environment variables below are |
| `project` | 2 | `.hermes.md`/`HERMES.md`, `AGENTS.md`, `CLAUDE.md`, `.cursorrules`/`.cursor/rules/*.mdc`, and a trusted repo's `.hermes/skills/`, `.agents/skills/` |
| `user` | 3 | the Hermes home: `config.yaml`, `SOUL.md`, `memories/`, `skills/` |
| `create-dir`, `external:<n>` | 4+ | `skills.create_dir` and `skills.external_dirs` |
| `plugin:<name>` | after | an enabled folder plugin, `<home>/plugins/<name>/` |

The Hermes home is `~/.hermes` on POSIX (`%LOCALAPPDATA%\hermes` on Windows), or `$HERMES_HOME` when set
([environment variables](https://hermes-agent.nousresearch.com/docs/reference/environment-variables)). When
`$HERMES_PROFILE` names an existing profile under `profiles/<name>/`, **inferred** (the docs document the `-p`
flag, and a profile is a home of its own
([profiles](https://hermes-agent.nousresearch.com/docs/user-guide/profiles))), that directory is the home instead.
A named profile whose folder is missing warns and falls back to the main home.

Precedence ([managed scope](https://hermes-agent.nousresearch.com/docs/user-guide/managed-scope)): for each key the
managed layer names, it wins over everything including the shell environment; merging is leaf-level. Outside those
keys, an environment variable overrides `config.yaml` (the usual rule, stated on the managed-scope page as the one
inversion). rigswig models that inversion only where it read the variable: `HERMES_INFERENCE_MODEL` and
`HERMES_MODEL` for the model (**inferred**: which of the two wins when both are set is not documented; the
inference variable is taken first, from the model-configuration pages).

## Settings

`model` (`model.default`), `provider` (`model.provider`), `modelBaseUrl` (`model.base_url`, shown as its scheme and
host), `contextLength` (`model.context_length`), `approvalMode` (`approvals.mode`), `contextFileMaxChars`
(`context_file_max_chars`), and the memory limits (`memory.memory_char_limit`, `memory.user_char_limit`), each with
the trail of layers that set it. Only non-secret keys: secrets live in `.env`, which this adapter never opens, and
`model.api_key` is not read.

## Context files

([Context Files](https://hermes-agent.nousresearch.com/docs/user-guide/features/context-files),
[Which File Does What](https://hermes-agent.nousresearch.com/docs/user-guide/which-file-does-what))

- **One project context type loads**, first match wins: `.hermes.md`/`HERMES.md` → `AGENTS.md` → `CLAUDE.md` →
  `.cursorrules`/`.cursor/rules/*.mdc`. `SOUL.md` is independent of that chain.
- `.hermes.md` discovery walks parents up to the git root; when several copies sit in that walk the nearest wins and
  the rest are shadowed (**inferred**: the docs describe the walk and the per-package overrides, not the multi-copy
  tie).
- `AGENTS.md` at the working folder loads at startup; nested copies below it are modelled with `load.when:
  on-match`, `appliesTo: ["<subfolder>/**"]`, the subfolder search skipping dot folders, dependencies and build
  output (**inferred** list, carried over from the other adapters), going 12 folders deep and stopping after 20,000
  folders with a warning. When `AGENTS.md` is not the winning type its nested copies are not modelled (they load
  only when their type wins).
- `CLAUDE.md` and `.cursorrules` load from the working folder only; `.cursor/rules/*.mdc` files load at startup
  (**inferred**: their per-rule frontmatter globs are not modelled).
- Every file is capped at `context_file_max_chars` characters — the pinned value, or 20,000 as the floor of the
  dynamic cap when nothing pins it; the file that crosses it is head+tail truncated with the counted characters =
  the kept ones, and a warning.
- An empty file (or a read that fails) is treated as absent, like Hermes falling back and skipping it.

## SOUL.md and memory

([Persistent Memory](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory))

- `SOUL.md` in the Hermes home is always loaded at startup when present (system-prompt slot #1); capped like every
  context file. Empty means the built-in identity, which rigswig reports as unmeasured.
- `MEMORY.md` and `USER.md` in `<home>/memories/` are injected as a frozen snapshot at startup; their character
  limits are reported as settings, and a file over its limit (a hand-edited one — the `memory` tool refuses writes
  past it) warns. `memory_enabled: false` drops `MEMORY.md`, `user_profile_enabled: false` drops `USER.md`; when
  `memory.provider` names an external provider the on-disk files are not the injected store, so neither is counted
  (**inferred**: the built-in files are the default provider's store).

## Skills

([Skills System](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills))

- Tiers, highest precedence first: project → local (`<home>/skills/`) → `skills.create_dir` → `skills.external_dirs`.
  The same name in two tiers: the higher one wins, the hidden copy is marked shadowed with a warning. Bundled and
  hub-installed skills live in the local dir like user skills (seeded on install; the `.bundled_manifest` marker is
  not modelled).
- Two same-named skills **in one tier** are never guessed between: both are listed under their exact relative path
  (`a/one`, `b/two`), with a warning, and the bare name is ambiguous in Hermes.
- At startup only the listing loads: the skill's `name` and `description` (measured); the body loads when the skill
  is invoked (`SkillAttrs.bodyChars`). `skills.auto_load` pins skills whose full body loads every session; a name
  it lists that no effective skill provides warns (as Hermes warns and skips; a name that is hidden or shadowed
  warns too).
- `platforms` in the front matter hides a skill on other operating systems (**inferred**: the OS matching is the
  plain word comparison the docs describe).
- Project skills load only from a repo in `skills.trusted_project_dirs` (the trust list, `~` expanded to the home);
  `skills.project_discovery: false` turns the feature off. Conditional activation (`requires_toolsets`,
  `fallback_for_tools`), Skills Hub quarantine and curator locks: not modelled.
- `skills.external_dirs` entries with `${VAR}` are skipped with a warning: rigswig reads only Hermes's own variable
  list.

## Plugins

([Plugins](https://hermes-agent.nousresearch.com/docs/user-guide/features/plugins))

- Folder plugins in `<home>/plugins/<name>/` with a `plugin.yaml`. A dropped-in plugin runs unless
  `plugins.disabled` names it; when `plugins.enabled` exists (the allow-list the CLI maintains) and names it
  neither way, the plugin is still counted active **with a warning** that the allow-list suggests it is off. Its
  package records what `user` scope says.
- `provides_hooks` entries are counted as hook items (a Python callback working outside the context; `pre_tool_call`
  → before-tool, `post_tool_call` → after-tool, subagent events likewise; anything else keeps its name kebab-cased).
- Built-in plugins that ship inside the install, Nix-declared plugins, and the tools, slash commands and skills they
  register in Python: not modelled (their schema text is named as an unmeasured cost).

## MCP servers

([MCP](https://hermes-agent.nousresearch.com/docs/user-guide/configuring-models) and the
[MCP Config Reference](https://hermes-agent.nousresearch.com/docs/reference/mcp-config-reference))

- `mcp_servers` in `config.yaml`; the managed layer may also carry servers, and on a name clash the managed server
  wins (**inferred**: pinning is per key, and a whole server entry is one of them).
- `command` → stdio (the command's file name only); `url` → Streamable HTTP, or SSE when `transport: sse`
  (scheme and host only). `enabled: false` → `Disabled` and unmeasured; a server with neither command nor url warns.
- Tool inclusion/exclusion, sampling, elicitation, events, spending limits, OAuth token files (`mcp-tokens/`) and
  trust tiers are not modelled. Like every harness, each effective server's tool listing is named among the
  unmeasured costs.

## Env vars read

`HERMES_HOME`, `HERMES_PROFILE`, `HERMES_MANAGED_DIR`, `HERMES_INFERENCE_MODEL`, `HERMES_MODEL` (and
`LOCALAPPDATA` on Windows for the default home). Nothing else, and never the contents of `.env`.
