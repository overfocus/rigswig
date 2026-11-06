# Roadmap

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
   then by source, the unmeasured part marked, with a ranked list underneath. Compare (done): `rigswig compare` for two repos or two harnesses, and on the page (`compare with`) two bars on
   one scale with what only one side has and what both have differently.

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
publishing) and on npm (`npx rigswig`, trusted publishing). Left: Homebrew.

Audit (done, [#2](https://github.com/overfocus/rigswig/issues/2); [AUDIT.md](AUDIT.md)): `rigswig audit` with
ranked findings, savings and fixes, and fleet mode (`--repos`). The gaps it found are closed: the auto-memory index is
measured, and SessionStart hooks and each MCP server are named among the unmeasured costs. Left from #2: the three
write-mode actions (archive an agent or skill, move a plugin to project scope, drop permission rules). The page runs
audits (one repo or a fleet) with a progress bar and shows the findings.

GitHub repos (done; [GITHUB.md](GITHUB.md)): `audit` and `status` take `--github owner/repo[@ref]` (opt-in per run,
one host, the archive unpacked into memory, an empty home) or `--archive <file>` (offline). On the page with
`ui --allow-github`. Left: private repos (a token from the environment), and a counts-only output for sharing.

## Later

- Write mode for Codex (`config.toml` with comments kept) and for more kinds (skills, hooks) where the tool
  documents a switch.
- DeepSeek: show DeepSeek setups, as its own harness or as a model provider seen through other harnesses
  ([#1](https://github.com/overfocus/rigswig/issues/1)).
- A WebAssembly build of `rigswig-core`, for a VS Code extension or a page that reads an uploaded `--json`.
- Real tokenizer counts where a harness's tokenizer is public, labelled as such.

## Not in scope

Session monitoring and transcripts, writing or syncing configuration, testing MCP servers, any network call other than
the opt-in `--github` download.
