# Roadmap

## Product direction from the 2026-11-10 review

rigswig's durable advantage is its compiler-like core: harness-specific readers resolve into one provenance-rich
model, then audits, comparisons, simulations and output formats consume it. The product direction is **static
analysis and regression prevention for coding-agent configuration** — the equivalent of a linter and a dry-run plan
for agent harnesses — not runtime watching, general configuration synchronisation or automatic instruction rewriting.

The next releases optimise for reproducibility, measurement confidence and a repeatable CI workflow rather than the
number of adapters or findings. Token estimates remain explicitly secondary to semantic resolution and provenance.

### 0.2.x: correctness and trust

- [x] Make automation deterministic: `check`, `audit`, `compare` and MCP use repository evidence, never live-session
  activity. Interactive `status` and `ui` may follow a fresh **repo-scoped** session; machine-wide activity is shown
  as evidence but never overrides the repo's own configuration.
- [x] Harden write mode before expanding it: preserve settings-file permissions across atomic replacement, create
  private backup directories and files, and use an OS-backed CSPRNG which fails closed when secure randomness is not
  available.
- [x] Clarify that the headline estimate covers known file-provided startup context; characters remain the exact
  comparison unit and tokens are advisory.
- [x] Keep the required formatting, test and Clippy gates green on stable Rust.
- [x] Design and implement strict automation semantics for unreadable required files and incomplete adapter coverage.
  `check --strict` fails on machine-readable `incomplete-*` warnings, not on `coverage.notModelled` itself; adapters
  opt into the contract and third-party adapters fail closed by default ([STRICT.md](STRICT.md)).

### 0.3: regression prevention and confidence

- [x] Model Ollama Launch integrations and Hermes setups launched to local or cloud models. Preserve the selected
  models, infer execution from Ollama's cloud naming rather than the local endpoint, and include cloud models which
  have no local manifest ([Ollama](adapters/ollama.md), [Hermes](adapters/hermes.md)).
- Version every public JSON surface (`status`, `audit`, `compare`, `fleet`, `usage` and `check`) with generated
  schemas, compatibility tests and consistent adapter/coverage provenance. Expose structured MCP results and schemas.
- [x] Add versioned check baselines, `check --baseline`, and a no-new-debt ratchet which carries existing warnings
  and failures, rejects new debt, reports fixes, and refuses to drop a previously failing check
  ([BASELINES.md](BASELINES.md)).
- Add repository policy with explained suppressions and SARIF output. The key question is "did this change make the
  harness worse?", not only "is it under a fixed budget?".
- Separate finding impact, confidence, effort and remediation safety. Distinguish referenced, invoked, manually
  invoked but unobservable, unknown because scanning was incomplete, and supported negative evidence. Truncation or
  unsupported invocation types must produce `unknown`, never proof that an item is unnecessary.
- [x] Reuse `OverlayFs` and full adapter re-resolution for file-backed `--without` selections across local/remote
  status, UI, check and MCP. Shadowed winners return and warnings are recomputed; shared config files are refused
  rather than deleting unselected siblings.
- Extend exact re-resolution to structured config-item removal, audit savings and multi-change simulations. The
  model-only `whatif::without` compatibility helper documents that it does not restore shadowed items.
- [x] Make MCP summary-first, with kind/item drill-down and typed request validation before resolution. Full harness
  JSON requires `view: "full"`; advertised schemas reject unknown arguments and match the runtime types.
- Introduce adapter capability tiers and a compatibility matrix with last-verified harness versions, a contributor
  conformance kit and differential fixtures from real harness versions. Depth and confidence outrank adapter count.
- Promote native harness-reported measurements and public tokenizers, calibrated by content type. Let `check` select
  the estimate or a conservative bound explicitly.

### Candidate spike: behavioral harness comparison with agent-neuron

The existing `compare` answers a static question: what differs between two resolved configurations? A promising next
step is an offline behavioral comparison orchestrated by [agent-neuron](https://github.com/overfocus/agent-neuron):
run the same scenario against configurations A and B, then compare both observed behavior and rigswig's static
prediction. This is a spike, not yet an engine dependency or a committed surface.

- Scenarios use invented, disposable fixtures and only offline harness commands or deterministic stand-ins which
  cannot call a model or the network. One `run` neuron handles one scenario/configuration input; the engine supplies
  timeouts, budgets and a replayable trace rather than another hand-built runner.
- Normalize and compare which instructions and tools became available, permission and trust decisions, context costs,
  errors and invocation order. Keep message contents, personal configuration and real session logs out of the trace.
  Show where observed behavior confirms or contradicts the static model, with each fact's provenance intact.
- The spike must preserve every current guarantee: no real model calls, no network, no writes outside its scratch
  fixture, no session-log reads, and no change to default builds. agent-neuron's engine is POSIX-only; any later
  integration stays optional there. Prefer a sidecar for the experiment; if linking ever makes sense, pin a full
  engine commit and reassess publishing first.
- Real model-backed quality evaluation is a different product and consent boundary, not something to introduce
  silently through this feature. It is outside this spike.

### Deliberately later

More adapters for coverage numbers, broader live-session detection, general-purpose write mode, WebAssembly, private
GitHub support, runtime watching, cloud fleet management, model-generated rewrites and MCP testing. Usage readers for
Gemini CLI and Copilot CLI remain valuable where they improve audit confidence without weakening metadata-only
privacy.

## v0.1

1. **Repository** (done): name, licence, fresh history, a Rust workspace (`rigswig-core` library, `rigswig` binary),
   CLI skeleton, tests.
2. **Data model and schema** (done: `rigswig-core/src/model.rs`, `schema/`, the contract check, the read-only `Fs`
   trait with an in-memory tree for tests, the `Adapter` trait; `rigswig schema` prints the schema). A harness-neutral model under a versioned JSON Schema:
   - layers: managed, org, user, project, local, package;
   - items (instructions, scoped instructions, agents, skills, commands, hooks, MCP servers, permissions, model), each
     with its origin, `effective`, `shadowedBy` and a content hash;
   - a decision trail for every resolved setting ("which scope decided this");
   - a required `coverage.notModelled` list, so the output says what it could not see;
   - every figure labelled native, calculated, estimated or unknown.
   Adapters implement one trait (`detect`, `layers`, `read`, `resolve`) over an injected file system. The schema is
   generated from the Rust types (schemars) and checked in, so other tools can read the JSON without Rust.
3. **Claude Code adapter** (done: `rigswig-core/src/adapters/claude_code/`, its rules and their sources in
   [adapters/claude-code.md](adapters/claude-code.md); `rigswig status` runs it). Settings precedence, plugins and marketplaces, agents, skills, commands, hooks
   (honouring `disableAllHooks`), MCP servers and approval, permissions, managed settings on macOS, Linux and Windows,
   and overridden items detected rather than counted twice.
4. **Codex adapter** (done: `rigswig-core/src/adapters/codex.rs`, its rules and sources in
   [adapters/codex.md](adapters/codex.md)). Trust, layered `config.toml`, the shared `AGENTS.md` budget, skills,
   prompts, TOML agents, hooks, MCP servers, requirements, installed plugins.
5. **The page** (done: `rigswig ui`, `crates/rigswig/src/ui.rs` and `assets/ui.html`). The headline is a context budget: one bar on the model's context-window scale, split by kind and
   then by source, the unmeasured part marked, with a ranked list underneath. Compare (done): `rigswig compare` for two repos or two harnesses, and on the page (the A/B harness picker) two bars on
   one scale, each against its own window (from the model each resolves to, or the reader's pick), with what only one side has and what both have
   differently.

**Reference behaviour.** The adapter was checked against the earlier JavaScript explorer it replaces, on a real setup:
the same agents, skills, commands, hook scripts, MCP servers and plugins, name for name, and the same context total.
Where they differ, the adapter follows the docs (overridden items, four import hops, `AGENTS.md` only without a
`CLAUDE.md`, `disableAllHooks`, managed settings on every OS).

**Distribution.** One static binary per platform (macOS arm64 and x64, Linux x64 and arm64 musl, Windows x64), built in
CI. Installed with `cargo install rigswig`, Homebrew, or `npx rigswig`, which picks the right prebuilt binary from
per-platform npm packages (as esbuild and Biome do). A Claude Code plugin wrapping the binary comes with it.

Also in v0.1 (done): `rigswig check` for CI (`--max-tokens`, `--max-item-tokens`, `--no-warnings`; exit 1 on a
failed limit), and `--without` what-ifs on `status` and `check`.

Release builds (done): a version tag builds five platform binaries into a draft GitHub release
([RELEASING.md](RELEASING.md)); v0.1.0 is the first, also on crates.io (`rigswig`, `rigswig-core`, trusted
publishing) and on npm (`npx rigswig`, trusted publishing). Homebrew since v0.2.0: `brew install
overfocus/tap/rigswig`, a formula over the release binaries in [the tap](https://github.com/overfocus/homebrew-tap).

Audit (done, [#2](https://github.com/overfocus/rigswig/issues/2); [AUDIT.md](AUDIT.md)): `rigswig audit` with
ranked findings, savings and fixes, and fleet mode (`--repos`). The gaps it found are closed: the auto-memory index is
measured, and SessionStart hooks and each MCP server are named among the unmeasured costs. Left from #2: the three
write-mode actions (archive an agent or skill, move a plugin to project scope, drop permission rules). The page runs
audits (one repo or a fleet) with a progress bar and shows the findings.

GitHub repos (done; [GITHUB.md](GITHUB.md)): `audit` and `status` take `--github owner/repo[@ref]` (opt-in per run,
one host, the archive unpacked into memory, an empty home) or `--archive <file>` (offline). On the page with
`ui --allow-github`. Left: private repos (a token from the environment), and a counts-only output for sharing.

Usage from session logs (done; [USAGE.md](USAGE.md)): `rigswig usage` and `audit --usage` read Claude Code's session
logs opt-in per run, metadata only, and `not-invoked` names what loads but no session called. Codex and the other
harnesses refuse for now ([next](#next-usage-from-more-harnesses)), and the page and the MCP server stay out of it.

The extension (done: [vscode/](../vscode/)): the status bar carries the harness, the token bill and the audit's
findings with the tokens they could save; a view lists each finding (with its fix and files) and what loads by kind;
findings land in the Problems panel; it re-reads when a harness file changes; and everything stays read-only — the
findings spawn never passes `--usage`, and an untrusted workspace cannot choose the binary.

## Next: usage from more harnesses

`rigswig usage` reads only Claude Code's session logs today. An assessment (2026-11-07, from each harness's source
and docs) asked whether the other harnesses can be read to the same bar as [USAGE.md](USAGE.md) sets:
- logs opened only by a run that asks;
- metadata only, proved by canary tests;
- streamed;
- matched to the repo by its working folder.

| Harness | Verdict | Effort | Where the sessions are | Skills and agents | Caveats |
|---|---|---|---|---|---|
| Gemini CLI | feasible | S–M | JSONL per session under `~/.gemini/tmp/<project>/chats/`; matched by a hash of the project root | `activate_skill`, `invoke_agent` tool calls | MCP names join server and tool with one `_`; sessions are deleted after 30 days by default; the format moved from JSON to JSONL recently |
| GitHub Copilot CLI | feasible | M | JSONL events per session under `~/.copilot/session-state/<id>/`, with `cwd` and git root | `skill.invoked`, `subagent.started` events; MCP server and tool are separate fields | token totals arrive only with a clean `session.shutdown` |
| OpenAI Codex CLI | partial | M | JSONL rollout per thread under `$CODEX_HOME/sessions/YYYY/MM/DD/`, with `cwd` | subagents yes; **skills no**: they appear only inside message text | two token schemas across versions; zstd compression coming; some MCP names hashed |
| Hermes Agent | feasible | M | one SQLite database, `~/.hermes/state.db`, with `cwd` per session | `skill_view`, `delegate_task` tool calls | needs SQLite; internal schema that changes |
| DeepSeek Harness (dsh) | feasible | M | JSONL per session, zstd-compressed by default, under `~/.dsh/sessions/` | `skill` tool; subagents write child logs | pre-release; the format went v0 to v4 in months |
| Devin CLI | partial | M | SQLite `sessions.db` plus a JSON transcript per session | undocumented | undocumented; each session is one large JSON value |
| DeepSeek-CLI (community) | partial | S–M | one JSON file per session, rewritten whole | `task` tool for agents; **skills not logged** | MCP names ambiguous; cumulative tokens only |
| Cursor | partial | L | SQLite (`state.vscdb` for the editor, `store.db` per chat for cursor-agent), undocumented | unknown | two or three formats; very large databases; token counts often zero |
| Ollama | not applicable | – | no sessions (its prompt history is content, never read) | – | – |
| Hugging Face CLI | not applicable | – | no agent loop, no session logs | – | – |

**What it takes.**
- **Two new dependencies:**
  - SQLite, opened read-only and immutable, for Hermes, Devin and Cursor;
  - zstd, for dsh and later Codex.
  Each needs its reason in the commit, as the rules say.
- **One line in memory, per format:**
  - JSONL readers keep the rule as it stands.
  - SQLite readers select only metadata columns, so content never leaves the database.
  - Where a session is one large JSON value, a streaming parser skips the content keys.
  USAGE.md will say per harness which of these applies.
- **A canary test per format.** It covers SQL columns, nested JSON, and JSON-encoded `arguments` strings, where the
  skill or agent name sits next to content.
- **Invented fixtures** written from each harness's source, ideally checked against logs from a sandboxed install.
- **MCP names matched to the configured servers**, where a harness joins server and tool with one `_` or hashes long
  names. Matching errs toward "invoked", as it does for Claude Code.
- **Gaps stated, not guessed.** Where a harness does not log skills (Codex, DeepSeek-CLI), `not-invoked` leaves
  skills out instead of calling every skill idle.

**Order.**
1. Gemini CLI and Copilot CLI.
2. Codex, without skills.
3. Hermes, the first SQLite reader.
4. dsh, once its format settles.
5. Devin, DeepSeek-CLI and Cursor, on request.

Until a harness has its reader, `usage` keeps refusing it. Ollama and the Hugging Face CLI will refuse with a clearer
reason: they keep no sessions.

## Deferred: broader session detection

Interactive `status` and `ui` follow only a fresh claim whose artifacts name the repo. Machine-wide claims remain
`tools` evidence and never select a harness; `check`, `audit`, `compare` and MCP always use repository evidence.
Claude Code can make a repo-scoped claim. Hermes can only report machine-wide activity without opening its database,
so it no longer moves the default pick.

Further repo-scoped probes (for example Gemini CLI and dsh) are deferred behind the 0.2.x/0.3 correctness work.
Machine-wide probes for Codex and Copilot are not selection inputs. A heartbeat file that harness hooks could append
to is one possible future source of exact per-repo evidence, but is not planned for 0.2.x.

## Later

- Write mode for Codex (`config.toml` with comments kept) and for more kinds (skills, hooks) where the tool
  documents a switch.
- DeepSeek: show DeepSeek setups, as its own harness or as a model provider seen through other harnesses
  ([#1](https://github.com/overfocus/rigswig/issues/1)).
- A WebAssembly build of `rigswig-core`, for a page that reads an uploaded `--json` without the CLI installed
  (the extension publishes instead, from `.github/workflows/extension.yml`).
- Native harness-reported context counts and public tokenizers: promoted to the 0.3 measurement work above.

## Not in scope

Watching a running agent (processes, ports, activity streams), reading session content (`usage` reads metadata
only: [USAGE.md](USAGE.md); the one exception is the interactive picker's is-a-session-live look, which reads session
folders' file names, kinds and modification times and never a file — adapter pages carry each harness's artifacts,
and `--no-session` switches off its influence on interactive selection), writing or
syncing configuration, testing MCP servers, any network call other than
the opt-in `--github` download.
