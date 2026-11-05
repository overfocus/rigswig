# Roadmap

## v0.1

1. **Repository** (done): name, licence, fresh history, CLI skeleton, tests.
2. **Data model and schema.** A harness-neutral model under a versioned JSON Schema:
   - layers: managed, org, user, project, local, package;
   - items (instructions, scoped instructions, agents, skills, commands, hooks, MCP servers, permissions, model), each
     with its origin, `effective`, `shadowedBy` and a content hash;
   - a decision trail for every resolved setting ("which scope decided this");
   - a required `coverage.notModelled` list, so the output says what it could not see;
   - every figure labelled native, calculated, estimated or unknown.
   Adapters are pure functions (`detect`, `layers`, `read`, `resolve`) over an injected file system.
3. **Claude Code adapter.** Settings precedence, plugins and marketplaces, agents, skills, commands, hooks
   (honouring `disableAllHooks`), MCP servers and approval, permissions, managed settings on macOS, Linux and Windows,
   and overridden items detected rather than counted twice.
4. **Codex adapter.** `AGENTS.md`, `~/.codex/config.toml`, MCP servers, managed requirements.
5. **The page.** The headline is a context budget: one bar on the model's context-window scale, split by kind and
   then by source, the unmeasured part marked, with a ranked list underneath. Compare is two bars on one scale.

Also in v0.1: `rigswig check --max-tokens <n>` for CI, and a `--without <item>` what-if.

## Not in scope

Session monitoring and transcripts, writing or syncing configuration, testing MCP servers, any network call.
