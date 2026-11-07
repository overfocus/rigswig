# The Hugging Face CLI adapter

Sources (read 2026-11-06, against huggingface_hub 2.4.0, the installed CLI):

- the installed CLI's own code, quoted as the contract: `cli/_skills.py` (the staleness rule), `cli/skills.py`
  (the install and symlink logic), `huggingface_hub/constants.py` (the paths)
- huggingface.co/docs/hub: "CLI for AI Agents" (agents-cli) and "Skills" (agents-skills)
- huggingface.co/blog/hf-cli-for-agents (the generated skill's design; its context cost amortizes over a session)

Hugging Face does not ship its own coding agent (ml-intern is archived and moved to HuggingChat, which is a
hosted service). What the CLI governs on disk is a skill manager, so this adapter models the provider side of
the skills the other adapters read.

## The skills it models

| Layer | Path | Contents |
|---|---|---|
| project | `.agents/skills` | the store `hf skills add` writes without `--global` (a constant, no env override) |
| user | `~/.agents/skills` | the store `hf skills add -g` writes |
| imports | `.claude/skills`, or `CLAUDE_CONFIG_DIR/skills` | the mirror the CLI exposes to Claude Code |
| — | `--dest` roots | outside the modelled pairs |

The Agent-Skills layout: `<name>/SKILL.md` with `name` (else the folder's) and `description` frontmatter. Skill
items measure the listing; the body loads on invocation, so the listing is what shows at startup
(`hf-cli`'s body is the CLI's whole command reference, regenerated every release).

## The mirror rule (pinned from `cli/skills.py`)

Install writes the central copy into an agents root, then exposes it to Claude Code with
`link_path.symlink_to(os.path.relpath(central, agent_skills_dir))` — a relative symlink, correct for however
far `CLAUDE_CONFIG_DIR` sits from the store. On Windows the symlink may be refused and the CLI falls back to
`shutil.copytree`, and `hf skills update` refreshes the copy along with the store. The adapter pairs a mirror
entry when its realpath stays inside an agents root (link) or when its SKILL.md content hashes equal a store
skill's (copy); the store copy is effective and the mirror stays listed. Anything else inside the mirror
belongs to the Claude Code adapter, not here.

## The staleness audit (pinned from `cli/_skills.py`)

`_GENERATED_VERSION_RE = re.compile(r"Generated with `huggingface_hub v([^`]+)`")`; a skill whose stamp is
missing or differs from the installed version is stale — the CLI hints at install (when none exists) or update,
at most once per 24h, global before project. The adapter warns `hf-cli-skill-stale` per stale copy instead of
hinting, naming the scope. The installed version is read from disk: `~/.hf-cli/venv/lib/python*/site-packages`
`huggingface_hub-*.dist-info` (the standalone installer), then pipx (`~/.local/pipx/venvs/*`) and uv-tool
(`~/.local/share/uv/tools/*`) venvs. When no dist-info is found the adapter says so rather than claiming a
mismatch it cannot check.

## Not modelled

Common coverage plus, machine-checked:

- the update-check stamps (`HF_HOME`-located `.check_for_update_done`, `.check_for_skill_update_done`): mtime
  files gating a 24-hour reminder; `HF_HOME` moves them but nothing the adapter reads, so it is not declared
- the cached harness registry (`~/.cache/huggingface/.agent_harnesses.json`): the CLI's runtime detection of
  which agent runs it, mirrored from the Hub — telemetry, not configuration
- `hf skills update` / `hf skills list` reaching the Hub (marketplaces, statuses like `source_unreachable`):
  this read-only tool does no network
- the agentsmd `AGENTS.md` fallback bundles, marketplace skill archives, and `--dest` installs
- HF tokens and auth state: credentials, always outside the model
