# MCP server

`rigswig mcp` serves the same five read-only surfaces as **MCP tools** over stdio: any harness that speaks MCP can
register rigswig and then ask, mid-session, what a session in a repo loads, whether a checkout passes its budgets
and what an audit finds. It is the CLI, not a new mode: the same adapters, the same redaction, the same totals —
over a stdio subprocess the client launches and owns.

There is deliberately nothing else on the wire: no network transport (a socket would make rigswig a listener, and
the harnesses' own clients already launch stdio servers), no write tools (the page's write mode never becomes
agent-invokable), no GitHub tool (a tool call that downloads is a network request; use the CLI's `--archive`
for an offline repo you downloaded yourself), and no usage tool (session logs are read only by a run you start
yourself: [usage](USAGE.md)). The server answers nothing it does not know with `method not
found`, exits when stdin closes, and writes its logs to stderr, where a client may ignore them.

## Register it

The command is `rigswig` with the argument `mcp`. The config rows the adapters already read are the same rows:

- **Claude Code** — `.mcp.json` in the repo, or the user-scoped `~/.claude.json`:
  ```json
  { "mcpServers": { "rigswig": { "command": "rigswig", "args": ["mcp"] } } }
  ```
- **Cursor** — `.cursor/mcp.json` (project) or `~/.cursor/mcp.json`, same `mcpServers` shape.
- **Gemini CLI** — `settings.json`:
  ```json
  { "mcpServers": { "rigswig": { "command": "rigswig", "args": ["mcp"] } } }
  ```
- **DeepSeek-CLI** — `~/.deepseek-cli/mcp.json` or the repo's `.mcp.json`, same `mcpServers` shape.
- **VS Code** (Copilot) — the harness reads the portable format directly: the workspace-root `.mcp.json`, or the
  global `$COPILOT_HOME/mcp-config.json` (`~/.copilot/mcp-config.json` by default) — same `mcpServers` shape, and
  the user row is the very file rigswig's copilot adapter shows. The editor's own workspace file
  `.vscode/mcp.json` works too, in VS Code's `servers` shape (forwarded to the session; rigswig needs no
  interactive inputs, so nothing is held back):
  ```json
  { "servers": { "rigswig": { "type": "stdio", "command": "rigswig", "args": ["mcp"] } } }
  ```
  Or the guided flow: **MCP: Add Server**, choose **Command (stdio)**, enter the command `rigswig` with the
  argument `mcp`.
- **anything else that launches stdio servers** — same two words; MCP's stdio transport is standard.

`rigswig` must be on the harness client's PATH (`cargo install rigswig`, or `npx rigswig` is not usable here —
the client needs one process to launch, and `npx` is a wrapper). `rigswig status` on a repo whose configs carry
this row shows the server like any other MCP server, tool listings unmeasured.

## The tools

| Tool | Answers | Arguments |
|---|---|---|
| `status` | what a session in a repo loads: `status --json` (the full harness: items, sizes, origins, trail) | `repo` (or the client's process cwd), `tool`, `without` |
| `check` | whether a checkout passes CI limits; `passed` is the verdict | `repo`, `tool`, `maxTokens`, `maxItemTokens`, `noWarnings`, `without` |
| `audit` | ranked findings, each with a saving and a fix | `repo`, `tool` |
| `compare` | what differs: two repos with one harness (`repo`, `repoB`, `tool`), or two harnesses on one repo (`repo`, `tools`) | |
| `tools` | the harnesses rigswig knows, with detection score, evidence, and any fresh live-session claim | `repo` |

Every result is the tool's JSON, serialised as the content text; a failure inside the tool (a repo that does not
exist, an unknown harness) is `isError: true` with the message, not a protocol error. Protocol errors are the
JSON-RPC ones: `-32700` for a line that is not JSON, `-32600` for a batch (later revisions dropped those),
`-32601` for a method rigswig does not have.

## The protocol surface

The handshake model of the current spec: `initialize` (the server echoes a requested version among
`2024-11-05`, `2025-03-26`, `2025-06-18`, `2025-11-25`, else answers `2025-06-18`), `notifications/initialized`
(silence), `tools/list`, `tools/call`, and `ping`. The stateless model newer revisions added is not implemented;
the handshake one is what today's clients speak. Hand-rolled on `serde_json` and std — the transport is
newline-delimited JSON over the standard streams, the server flushes after every message, and there is no other
output on stdout.
