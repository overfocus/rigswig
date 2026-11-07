//! `rigswig mcp`: the same five read-only surfaces as MCP tools, over stdio (docs/MCP.md).
//!
//! One client, one subprocess: newline-delimited JSON-RPC 2.0 in, JSON-RPC out. Nothing is written to stdout
//! that is not a message, stderr carries the logs, and stdin closing is the only shutdown signal. Hand-rolled
//! on `serde_json` and std: no network, no threads, no new dependencies.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use rigswig_core::read::redact::printable;
use serde_json::{Map, Value, json};

/// The protocol revisions this server can speak (the handshake model; the stateless model is not implemented).
const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
/// What an unsupported request gets instead; the client may disconnect if it cannot use it.
const FALLBACK_VERSION: &str = "2025-06-18";

pub fn serve(args: &[String]) -> std::process::ExitCode {
    if let Some(a) = args.first() {
        return crate::fail(&format!("unknown option: {a} (rigswig mcp takes no options)"));
    }
    for line in std::io::stdin().lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(t) {
            Err(e) => send_err(None, -32700, format!("parse error: {e}")),
            Ok(msg) => handle(msg),
        }
    }
    std::process::ExitCode::SUCCESS
}

/// One inbound message: a request gets a response, a notification gets (at most) silence.
fn handle(msg: Value) {
    let Some(obj) = msg.as_object() else {
        // The spec's 2025-03-26 revision allowed batches; later ones dropped them. Say so once per message.
        return send_err(None, -32600, "only single JSON-RPC messages are accepted".into());
    };
    let id = obj.get("id").filter(|i| !i.is_null()).cloned();
    let method = obj.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = obj.get("params").and_then(Value::as_object).cloned().unwrap_or_default();
    let Some(id) = id else { return }; // a notification answers nothing, known or not

    match method {
        "initialize" => send(
            id,
            json!({
                "protocolVersion": version(&params),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "rigswig", "version": rigswig_core::VERSION },
                "instructions": "Five read-only tools over coding-agent harness setups, one repo at a time. Nothing is written and no network request is made.",
            }),
        ),
        "ping" => send(id, json!({})),
        "tools/list" => send(id, json!({ "tools": tools_list() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            match call(name, &args) {
                Ok(out) => {
                    send(id, json!({ "content": [{ "type": "text", "text": out.to_string() }], "isError": false }))
                }
                Err(e) => {
                    send(id, json!([{ "content": [{ "type": "text", "text": printable(&e) }], "isError": true }]))
                }
            }
        }
        other => send_err(Some(id), -32601, format!("method not found: {other}")),
    }
}

fn version(params: &Map<String, Value>) -> String {
    match params.get("protocolVersion").and_then(Value::as_str) {
        Some(v) if SUPPORTED_VERSIONS.contains(&v) => v.to_string(),
        _ => FALLBACK_VERSION.into(),
    }
}

fn send(id: Value, mut result: Value) {
    // A tool envelope arrives here as a bare content object's pieces; the spec wants the object itself.
    if result.is_array() {
        result = result[0].clone();
    }
    let out = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let mut out_io = std::io::stdout().lock();
    let _ = writeln!(out_io, "{out}");
    let _ = out_io.flush(); // the client's next read waits on this byte, not on the process's exit
}

fn send_err(id: Option<Value>, code: i32, message: String) {
    let out = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } });
    let mut out_io = std::io::stdout().lock();
    let _ = writeln!(out_io, "{out}");
    let _ = out_io.flush();
}

// The five tools. Each takes its arguments and returns the JSON the model reads, or a plain message.

/// `status`: the full harness JSON for a repo (its tool, or the one with the most evidence).
fn status(args: &Map<String, Value>) -> Result<Value, String> {
    let mut h = crate::resolve(&arg_repo(args)?, arg_tool(args).as_deref())?;
    let without = strs(args, "without");
    if !without.is_empty() {
        h = rigswig_core::whatif::without(&h, &without).0;
    }
    Ok(serde_json::to_value(&h).expect("the model serialises"))
}

/// `check`: the CI limits as one object, with `passed` for the whole row set.
fn check(args: &Map<String, Value>) -> Result<Value, String> {
    let mut h = crate::resolve(&arg_repo(args)?, arg_tool(args).as_deref())?;
    let without = strs(args, "without");
    if !without.is_empty() {
        h = rigswig_core::whatif::without(&h, &without).0;
    }
    let max_tokens = args.get("maxTokens").and_then(Value::as_u64);
    let max_item = args.get("maxItemTokens").and_then(Value::as_u64);
    let no_warnings = args.get("noWarnings").and_then(Value::as_bool).unwrap_or(false);
    Ok(crate::check_json(&h, max_tokens, max_item, no_warnings))
}

/// `audit`: the ranked findings for one repo.
fn audit(args: &Map<String, Value>) -> Result<Value, String> {
    let quiet = &mut |_: rigswig_core::audit::Progress| {};
    let (a, _) = crate::audits::repo(&arg_repo(args)?, arg_tool(args).as_deref(), None, quiet)?;
    Ok(serde_json::to_value(&a).expect("serialises"))
}

/// `compare`: two repos with one harness, or two harnesses on one repo.
fn compare(args: &Map<String, Value>) -> Result<Value, String> {
    let tools = strs(args, "tools");
    let (x, y) = (tools.first().cloned(), tools.get(1).cloned());
    let (a, b) = match (x, y) {
        (Some(x), Some(y)) => {
            let repo = arg_repo(args)?;
            (crate::resolve(&repo, Some(x.as_str()))?, crate::resolve(&repo, Some(y.as_str()))?)
        }
        _ => {
            let tool = arg_tool(args);
            let Some(rb) = args.get("repoB").and_then(Value::as_str) else {
                return Err("compare needs two tools on one repo, or both repos".into());
            };
            (crate::resolve(&arg_repo(args)?, tool.as_deref())?, crate::resolve(&PathBuf::from(rb), tool.as_deref())?)
        }
    };
    Ok(serde_json::to_value(rigswig_core::compare::compare(&a, &b)).expect("serialises"))
}

/// `tools`: the harnesses rigswig knows and the evidence for each in the repo.
fn tools(args: &Map<String, Value>) -> Result<Value, String> {
    let list = crate::detections(&arg_repo(args)?)?;
    let rows: Vec<Value> = list
        .iter()
        .map(|(id, score, evidence)| {
            json!({ "id": id, "score": score, "evidence": evidence.iter().map(|e| printable(e)).collect::<Vec<_>>() })
        })
        .collect();
    Ok(json!({ "tools": rows }))
}

fn arg_repo(args: &Map<String, Value>) -> Result<PathBuf, String> {
    match args.get("repo").and_then(Value::as_str) {
        Some(r) => Ok(PathBuf::from(r)),
        None => Ok(std::env::current_dir().unwrap_or_default()),
    }
}

fn arg_tool(args: &Map<String, Value>) -> Option<String> {
    args.get("tool").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string)
}

fn strs(args: &Map<String, Value>, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Dispatch one tool call by name.
fn call(name: &str, args: &Map<String, Value>) -> Result<Value, String> {
    match name {
        "status" => status(args),
        "check" => check(args),
        "audit" => audit(args),
        "compare" => compare(args),
        "tools" => tools(args),
        other => Err(format!("unknown tool: {other}")),
    }
}

/// The five tools, in the catalog order the client shows.
fn tools_list() -> Value {
    json!([
    { "name": "status",
      "description": "What a coding-agent session in a repo loads before you type: instructions, skills, agents, hooks and MCP servers with their sizes and origins, for one harness (default: the one with the most evidence here). Full JSON detail.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" },
          "tool": { "type": "string", "description": "The harness id, e.g. claude-code, codex, cursor, gemini, hermes, ollama, devin, dsh, deepseek" },
          "without": { "type": "array", "items": { "type": "string" }, "description": "What to take out before totalling: a plugin (name@marketplace), a layer (layer:user), a kind and name (skill:deploy) or an item id" }
      }, "required": [] } },
    { "name": "check",
      "description": "CI-style limits for a checkout: stays within a total token budget and an item budget, and is warning-free. `passed` is the verdict.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" },
          "tool": { "type": "string", "description": "The harness id" },
          "maxTokens": { "type": "number", "description": "The maximum total startup tokens" },
          "maxItemTokens": { "type": "number", "description": "The maximum tokens for any single startup item" },
          "noWarnings": { "type": "boolean", "description": "Also fail when the resolution emits warnings" },
          "without": { "type": "array", "items": { "type": "string" }, "description": "What to take out before totalling, as in status" }
      }, "required": [] } },
    { "name": "audit",
      "description": "Ranked findings on a repo's harness setup: what costs context and nothing here references, plugins the repo's files do not call for, instruction files over budget, repeated text, dead references, piled-up permission rules; each with a saving and a fix.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" },
          "tool": { "type": "string", "description": "The harness id" }
      }, "required": [] } },
    { "name": "compare",
      "description": "What differs: two repos with one harness, or two harnesses on one repo. Give `tools` as two harness ids for the latter.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder, used with `tools` (default: the process's working directory)" },
          "repoB": { "type": "string", "description": "The second repo folder (then `tools` must be absent, and `tool` covers both)" },
          "tool": { "type": "string", "description": "The harness id, for both repos" },
          "tools": { "type": "array", "items": { "type": "string" }, "description": "Two harness ids on one repo, e.g. [\"claude-code\", \"codex\"]" }
      }, "required": [] } },
    { "name": "tools",
      "description": "The harnesses rigswig knows, and for each the detection score and the evidence found in the repo.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" }
      }, "required": [] } },
    ])
}
