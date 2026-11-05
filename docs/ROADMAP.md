# Roadmap

## v0.1

1. **Repository** (done): name, licence, fresh history, a Rust workspace (`rigswig-core` library, `rigswig` binary),
   CLI skeleton, tests.
2. **Data model and schema.** A harness-neutral model under a versioned JSON Schema:
   - layers: managed, org, user, project, local, package;
   - items (instructions, scoped instructions, agents, skills, commands, hooks, MCP servers, permissions, model), each
     with its origin, `effective`, `shadowedBy` and a content hash;
   - a decision trail for every resolved setting ("which scope decided this");
   - a required `coverage.notModelled` list, so the output says what it could not see;
   - every figure labelled native, calculated, estimated or unknown.
   Adapters implement one trait (`detect`, `layers`, `read`, `resolve`) over an injected file system. The schema is
   generated from the Rust types (schemars) and checked in, so other tools can read the JSON without Rust.
3. **Claude Code adapter.** Settings precedence, plugins and marketplaces, agents, skills, commands, hooks
   (honouring `disableAllHooks`), MCP servers and approval, permissions, managed settings on macOS, Linux and Windows,
   and overridden items detected rather than counted twice.
4. **Codex adapter.** `AGENTS.md`, `~/.codex/config.toml`, MCP servers, managed requirements.
5. **The page.** The headline is a context budget: one bar on the model's context-window scale, split by kind and
   then by source, the unmeasured part marked, with a ranked list underneath. Compare is two bars on one scale.

**Reference behaviour.** The agent-kit cockpit (JavaScript) already resolves Claude Code correctly in the cases its 83
tests cover. Its `status --json` on invented fixture homes becomes golden output that the Rust Claude Code adapter must
match, field for field, except where the review found it wrong (overridden items, `disableAllHooks`, managed settings
off macOS).

**Distribution.** One static binary per platform (macOS arm64 and x64, Linux x64 and arm64 musl, Windows x64), built in
CI. Installed with `cargo install rigswig`, Homebrew, or `npx rigswig`, which picks the right prebuilt binary from
per-platform npm packages (as esbuild and Biome do). The agent-kit plugin becomes a thin wrapper that calls the binary.

Also in v0.1: `rigswig check --max-tokens <n>` for CI, and a `--without <item>` what-if.

## Later

- A WebAssembly build of `rigswig-core`, for a VS Code extension or a page that reads an uploaded `--json`.
- Real tokenizer counts where a harness's tokenizer is public, labelled as such.

## Not in scope

Session monitoring and transcripts, writing or syncing configuration, testing MCP servers, any network call.
