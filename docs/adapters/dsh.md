# The DeepSeek Harness adapter: the rules it models

Every rule below is from the DeepSeek Harness repository (github.com/deepseek-ai/deepseek-harness) and its docs
site (deepseek-harness.github.io/deepseek-harness) — the `agent-instructions` package README, the standard agent
preset, the "Skills" and "System Prompt Assembly" subsystem pages, and `$DSH_HOME` layouts recorded by the
integration guides — read on 2026-11-06, for the developer preview (v0.1.0-rc). Where the docs are silent and the
adapter has to choose, the rule is marked **inferred** and listed in the output's `coverage.notModelled`. When
they change, change this file and the adapter together: the project is pre-release and the docs themselves say to
expect breaking changes.

DeepSeek Harness (`dsh`) is DeepSeek AI's own agent harness: an everything-is-a-plugin composition (Cordis —
profiles, bundles and patches in YAML) around one agent loop. Its instructions, skills and MCP come from plugins
mounted by a composition row, not from a settings file; the loader whose behaviour this adapter follows —
`dsh-agent-instructions` — ships enabled by default in `dsh-base` with a 65,536-byte budget per chain. There is
no settings file to read: the settings that decide anything live in a composed tree the CLI prints with
`dsh --dump-config`, which is not a file contract. So this adapter models the file surfaces the documents pin:
the instruction chain and the local skill roots.

## Layers

| Layer | Rank (lower wins) | Files |
|---|---|---|
| `user` | 0 | the harness home `$DSH_HOME` (default `~/.dsh`): `AGENTS.md` (the one user-global file), `skills/` |
| `project` | 1 | the repo: `.dsh/` (`skills/`), `.agents/skills/`, and every `AGENTS.md`-compatible file in the chain from the root down |

Project files outrank the harness home in a same-name clash (the docs: project entries outrank user entries in
one layer, and the nearest layer wins a duplicate name outright — a project skill's root always ranks above the
user's).

## Instructions

The `agent-instructions` plugin composes two chains into one baseline message:

- the **user-global chain**: `$DSH_HOME/AGENTS.md` alone — it has "no local overlay";
- the **project chain**: every existing candidate file from the project root (`.git`'s nearest ancestor) down to
  the session's working directory, in broad-to-specific order.

Per directory the base candidates are `AGENTS.md` then `CLAUDE.md`, then the local overlays
`AGENTS.local.md` then `CLAUDE.local.md` additively. Within one directory, candidates whose content matches after
trimming leading and trailing whitespace render once — the earlier candidate in the order wins, and the collapse
is what the CLI does silently; rigswig adds an `instruction-duplicate` warning. A candidate group's identical
`CLAUDE.md`-duplicates-`AGENTS.md` pairing is the docs' own example.

At the repo root the chain loads every session; below the root a file joins when a `read`, `write` or `edit`
first reaches its directory (the docs' dynamic discovery), so such files are listed `load: on-invoke` and stay
out of the startup total. A file that disappears or turns into a duplicate produces a removal notice at runtime
(a reload concern, not a load).

The per-chain byte budget (`maxBytes`, one for the user-global chain and one for the project chain; the
dsh-base default is 65,536) makes the rendered message smaller than the files whenever it bites: broader files
are omitted before the most specific file is truncated. rigswig measures the files.

## Skills

The shipped local provider scans roots in the documented rank order, and a duplicate skill name wins outright at
the higher rank:

| Rank | Source | Root |
|---|---|---|
| 100 | `project-dsh` | `<projectRoot>/.dsh/skills` |
| 200 | `project-agents` | `<projectRoot>/.agents/skills` |
| 400 | `user-dsh` | `<dshHome>/skills` (its `.system` child skipped) |
| 500 | `user-agents` | `<agentsHome>/skills` |

`Config.customSkillDirs` (300) and a bundled root (600) are composition-configured, so they are coverage notes,
as are the packaged and remote providers. The layouts are a directory bundle (`<name>/SKILL.md`) and a flat
`<name>.md`, with no recursive `**/SKILL.md` discovery and kebab-case names. A skill is invoked by name, so the
listing (name + description) is measured at startup and the body on invoke. The project root is the nearest
ancestor with `.git` — for rigswig's read it is the repo. `<agentsHome>`'s default location is
**inferred** (`~/.agents`); the docs name the root but not its default.

## Env vars read

`DSH_HOME` (the harness home's location). Everything else — API keys, provider settings, plugin options — lives
in the composition, and the credentials file is never read.

## What is not modelled

The Cordis composition is the whole settings surface: profile → bundles → patches mount the persona, tools, the
mcp-client (MCP), hooks, approvals, permission presets, the sandbox and the web profile. None of that is a file
contract the loader documents, so it is named in `coverage.notModelled` — along with the byte budgets' render
behaviour and which skills a session's scope chain and invocation policies expose. If DeepSeek Harness grows a
documented settings-file contract, the adapter gains a Settings section and this file changes.
