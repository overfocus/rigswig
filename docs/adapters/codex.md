# The Codex adapter: the rules it models

Every rule below is from OpenAI's Codex documentation, read on 2026-11-05. The `developers.openai.com/codex` pages now
redirect to `learn.chatgpt.com/docs`, so links point there. Where the docs were silent or disagreed, the source at tag
`rust-v0.166.0` of [openai/codex](https://github.com/openai/codex) decided, and the rule says so. Where neither says,
the rule is marked **inferred** and listed in the output's `coverage.notModelled`.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `requirements` (enforce) | 0 | `/etc/codex/requirements.toml` (Unix), `%ProgramData%\OpenAI\Codex\requirements.toml` (Windows) |
| `managed` | 1 | `managed_config.toml`: `/etc/codex/` (Unix), the Codex home (Windows) |
| `session` | 2 | command-line flags: not read |
| `project` | 3 | every `.codex/config.toml` from the project root down to the working folder, closest wins |
| `user` | 4 | `<codex home>/config.toml` |
| `system` | 5 | `/etc/codex/config.toml` (Unix) |
| `plugin:<name>@<marketplace>` | 6+ | an installed plugin's folder |

The Codex home is `~/.codex`, or `CODEX_HOME` when set
([environment variables](https://learn.chatgpt.com/docs/config-file/environment-variables)).

Precedence, highest first: command line, project, profile, user, cloud-managed defaults, system, built-in defaults
([config basics](https://learn.chatgpt.com/docs/config-file/config-basic)). `managed_config.toml` overrides the user
config and the command line at startup; `requirements.toml` holds constraints users cannot override
([managed configuration](https://learn.chatgpt.com/docs/enterprise/managed-configuration)). Tables merge key by key;
other values are replaced by the higher layer.

**Profiles** changed in Codex 0.134.0: a profile is a file, `~/.codex/<name>.config.toml`, applied only with
`--profile`. rigswig does not read command-line flags, so profiles are not modelled; the old `[profiles.x]` tables and
`profile =` key are no longer read by Codex either ([advanced config](https://learn.chatgpt.com/docs/config-file/config-advanced)).

## Trust

A project's `.codex/` config, hooks and agents load only when the project is trusted:
`[projects."<path>"] trust_level = "trusted"` in the user config, looked up for the working folder, its real path and
the project root. With no entry, trust is `unknown` and the project's `.codex/` layer is skipped (Codex asks). Project
`AGENTS.md` files are skipped only when the project is marked `untrusted` (source:
`codex-rs/core/src/agents_md.rs`).

A project `.codex/config.toml` cannot set `model_provider`, `model_providers`, `notify`, `profile`, `profiles`,
`openai_base_url`, `chatgpt_base_url`, `apps_mcp_product_sku`, `experimental_realtime_ws_base_url` or `otel`; Codex
ignores them there, and rigswig warns.

## Settings

`model`, `model_provider` (reported as `provider`), `model_reasoning_effort`, `model_context_window` (the page uses it as
the session's window), `approval_policy`, `sandbox_mode` and `project_doc_max_bytes`, each with the trail of layers that set it. When `requirements.toml`'s
`allowed_approval_policies` or `allowed_sandbox_modes` excludes the resolved value, the trail says so and rigswig warns:
Codex falls back to an allowed value. `projectTrust` is reported as a setting too.

## Instructions (AGENTS.md)

([AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md))

- Global: `AGENTS.override.md` in the Codex home, else `AGENTS.md`; the first non-empty one, trimmed. It does not count
  toward the budget (source).
- Project root: the nearest folder above the working folder that holds a root marker (`project_root_markers`, default
  `[".git"]`, from non-project layers); with none, the working folder alone.
- From the root down to the working folder, one file per folder: `AGENTS.override.md`, then `AGENTS.md`, then the
  `project_doc_fallback_filenames`. The first that exists is the folder's pick even when empty (an empty one is then
  skipped) (source).
- The project files share one budget, `project_doc_max_bytes` (32 KiB by default): the file that crosses it is cut to
  what is left, and later files are dropped (source; one docs page says "per file", the code says combined).

## Skills, prompts and agents

- **Skills** ([skills](https://learn.chatgpt.com/docs/build-skills)): `.agents/skills` in every folder from the
  working folder up to the project root, `~/.agents/skills`, `/etc/codex/skills`, and plugins. Also
  `<project root>/.codex/skills` and the deprecated `<codex home>/skills` (source; rigswig warns on the deprecated one).
  Two skills with the same name are both listed. At startup only the name and description load, within 2% of the
  context window (8,000 characters when the window is unknown); rigswig warns above 8,000. `[[skills.config]]` entries
  (by `name` or `path`, with `enabled`) switch skills off; the last match wins. Bundled system skills are not on disk.
- **Custom prompts** ([custom prompts](https://learn.chatgpt.com/docs/custom-prompts)), deprecated: top-level `.md`
  files in `<codex home>/prompts/`, invoked explicitly as `/prompts:<name>`, so they load on invoke.
- **Agents** ([subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents)): one TOML file each in
  `<codex home>/agents/` and, for a trusted project, `.codex/agents/`. `name` is the identity. A project agent beating a
  user agent of the same name is **inferred**. Agents cannot narrow their tools, so no all-tools warning.

## Hooks

([hooks](https://learn.chatgpt.com/docs/hooks)) `<codex home>/hooks.json`, `.codex/hooks.json` (trusted projects),
inline `[[hooks.<Event>]]` tables in every config layer, and plugins' `hooks/hooks.json`. Every source loads; none
replaces another. `[features] hooks = false` (or the deprecated `codex_hooks`) turns them all off. Codex runs a
non-managed hook only after the user reviews its hash: not modelled.

## MCP servers

([MCP](https://learn.chatgpt.com/docs/extend/mcp)) Only in `config.toml` layers (`[mcp_servers.<name>]`); there is no
project `.mcp.json`. `enabled = false` disables a server. A `requirements.toml` `[mcp_servers]` allowlist denies any
server not named in it (its `identity` matching is not modelled). Plugins' `.mcp.json` servers are switched by
`[plugins."<id>".mcp_servers.<name>] enabled`. A server set in two layers is taken whole from the higher one
(**inferred**: Codex may merge its fields).

## Plugins

([plugins](https://developers.openai.com/plugins/build/plugins)) Installed copies in
`<codex home>/plugins/cache/<marketplace>/<plugin>/<version>/` (the last version folder is used). Enabled by
`[plugins."<plugin>@<marketplace>"] enabled`, then `plugins._default.enabled`; an installed plugin with neither is taken
as enabled (**inferred**). `[features] plugins = false` turns every plugin off. Their skills are namespaced
`<plugin>:<skill>` (**inferred**). Manifest paths and apps are not modelled.
