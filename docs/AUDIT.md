# rigswig audit

`rigswig audit --repo <path>` reads a harness the way `status` does and returns ranked findings. Each one has its
evidence, an estimated saving where one can be computed, and a proposed change. `--json` gives everything, including
the `--without` patterns behind each saving and the user-scope items fleet mode is built from.

Everything comes from files, offline: no transcripts, no network, no model. So a finding never says an item is
"unused". It says what the files show: "nothing in the repo references this". Something you call by hand, or that a
user-level file mentions, will still show up here.

## Findings

Sorted by severity (high, medium, low), then by saving.

| Code | What it finds | Saving |
|---|---|---|
| `heavy-user-scope` | User-scope agents, skills or commands (one finding per kind) whose name, slug or file name appears nowhere in the repo's text files. Reported when at least 3 of them, worth at least 500 tokens, are unreferenced. | yes |
| `plugin-fit` | A plugin switched on in user settings when the repo has none of the files that show it is needed (no `Cargo.toml` or `.rs` for a Rust language server, no `vercel.json` or `.vercel/` for a Vercel plugin). A plugin turned on by the project's own settings is never reported. | yes |
| `instruction-budget` | A Claude Code instruction file over 200 lines, the aim the [memory docs](https://code.claude.com/docs/en/memory.md) give, with its five largest sections. Not checked for the auto-memory index, which Claude Code cuts itself. | no: rigswig never rewrites instruction files |
| `repeated-text` | Sentences of eight words or more that appear in two instruction files, for example the user `CLAUDE.md` and the project one. | no |
| `dead-reference` | In the repo's own instruction files: a backticked path whose first folder exists but the rest doesn't, an `npm run` / `pnpm run` / `yarn run` / `bun run` script missing from `package.json`, or a `make` target missing from the `Makefile`. Paths that `.gitignore` covers are skipped, and so are lines about another repo: a line that, together with the line before it, names a folder beside the repo or a `../` path. | no |
| `permission-broad` | Allow rules that let an agent do risky things without asking: every shell command, any `rm`, `kill`, `sudo`, interpreter (`node`, `python3`, ...), `curl`, `git push` and the like, or `Read`/`Edit`/`Write` of a credentials path (`.ssh`, `.aws`, `.config/gh`, `.env`, ...). | no |
| `permission-one-off` | Allow rules that are one full command rather than a pattern (a URL, quotes, `&&`, or 60 characters or more), the kind that piles up from "always allow". | no |
| `permission-covered` | Allow rules that a broader rule in any layer already covers (`Bash(npm run:*)` covers `Bash(npm run lint)`). | no |
| `warning:<code>` | The harness's own warnings (`status` shows them too), with a proposed fix. | no |

## Plugin signals

The built-in table is in `audit::builtin_signals`: language servers matched by their project files, and platforms
(Vercel, Supabase, Firebase, Netlify, Cloudflare, Terraform, Prisma) matched by their config files. A plugin the table
doesn't know is listed in `--json` with `needed` absent, and a note tells you how many there were.

You can add to the table, or replace an entry, in `~/.rigswig/signals.json`:

```json
{ "signals": [ { "plugin": ["deploybot*"], "needs": ["deploy.yaml", "infra/", "*.tf"] } ] }
```

`plugin` is a list of globs matched against the plugin's name (the part before `@`). `needs` lists what shows a repo
needs the plugin: a path, a folder (`infra/`), or a file pattern (`*.tf`), which is looked for at the top of the repo
and one folder down.

## Fleet mode

```sh
rigswig audit --repos '~/code/*'            # every folder in ~/code (rigswig expands the *)
rigswig audit --repos ~/code/*              # or let the shell expand it
rigswig audit --repos ~/code/a ~/code/b --json
```

Fleet mode audits every repo and then looks at each user-scope agent, skill, command and plugin across all of them:

- **archive**: nothing in any repo references it;
- **move to project scope**: fewer than half the repos need it, so it should go into those repos' project scope;
- **keep**: half or more need it;
- **unknown**: a plugin with no signal.

A folder counts as a repo if it is a git checkout or has the harness's own files. Folders listed in
`~/.rigswig/private.json` are skipped. Fleet mode assumes every repo shares one home folder, which is what makes a
user-scope item the same item in each repo.

## On the page

`rigswig ui` has an Audit box under the budget. **Audit this repo** runs the audit for the page's repo and harness;
**Audit many repos** runs fleet mode on the folders typed beside it (space-separated, `*` in the last part). Audits
only read, so they work without `--allow-writes`.

The server runs one audit at a time on its own thread (`POST /api/audit/start`, then `POST /api/audit/poll` with the
job's id, both behind the same gate as write mode: the page's Origin, the token in a header, a JSON body). The page
polls every 150 ms and draws the stage and the fraction done: reading the harness, reading the repo's files (with a
count), then each kind of check. In fleet mode it reports which repo it is on.

The results:

- **one repo**: totals (findings, high severity, tokens they could save), filters by severity, kind and text, and each
  finding as a card with its evidence, saving, fix and, for the page's own repo, buttons that open the item in the
  inventory above;
- **fleet**: each repo with its tokens and findings, which open in place, then every user-scope item with its
  recommendation, filterable.

## Not yet

Applying findings, from the page in write mode, with a preview, a backup and undo:

- archive an agent or skill;
- move a plugin to project scope;
- drop chosen permission rules.
