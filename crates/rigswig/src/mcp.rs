//! `rigswig mcp`: the same five read-only surfaces as MCP tools, over stdio (docs/MCP.md).
//!
//! One client, one subprocess: newline-delimited JSON-RPC 2.0 in, JSON-RPC out. Nothing is written to stdout
//! that is not a message, stderr carries the logs, and stdin closing is the only shutdown signal. Hand-rolled
//! on `serde_json` and std: no network, no threads, no new dependencies.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use rigswig_core::model::SessionScope;
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

/// A small default response: the totals and where to drill down, without serialising every item into model context.
fn status_summary(h: &rigswig_core::model::Harness) -> Value {
    let mut kinds = std::collections::BTreeMap::<&str, (usize, u64)>::new();
    for item in h.items.iter().filter(|i| i.effective) {
        let entry = kinds.entry(item.detail.kind()).or_default();
        entry.0 += 1;
        if item.load.when == rigswig_core::model::LoadWhen::Startup {
            entry.1 += item.load.chars.value;
        }
    }
    let kinds: Vec<Value> = kinds
        .into_iter()
        .map(|(kind, (items, startup_chars))| json!({ "kind": kind, "items": items, "startupChars": startup_chars }))
        .collect();
    json!({
        "view": "summary",
        "tool": h.tool,
        "repo": h.target.repo,
        "context": h.context,
        "effectiveItems": h.items.iter().filter(|i| i.effective).count(),
        "kinds": kinds,
        "warningCount": h.warnings.len(),
        "incompleteReadCount": h.warnings.iter().filter(|w| w.is_incomplete()).count(),
        "coverage": h.coverage,
        "drillDown": {
            "kind": "call status with view=kind and kind=<kind>",
            "item": "call status with view=item and item=<exact item id>",
            "full": "call status with view=full for the complete harness JSON",
        },
    })
}

/// `status`: summary-first harness JSON for a repo, with explicit drill-down or `view: full` for the whole model.
fn status(args: &Map<String, Value>) -> Result<Value, String> {
    only(args, &["repo", "tool", "without", "view", "kind", "item"])?;
    let view = text(args, "view")?.unwrap_or("summary");
    let kind = text(args, "kind")?;
    let item_id = text(args, "item")?;
    if kind.is_some() && view != "kind" {
        return Err("kind needs view=kind".into());
    }
    if item_id.is_some() && view != "item" {
        return Err("item needs view=item".into());
    }
    if !matches!(view, "summary" | "kind" | "item" | "full") {
        return Err(format!("unknown status view: {view}"));
    }
    let without = strs(args, "without")?;
    let repo = arg_repo(args)?;
    let tool = arg_tool(args)?;
    let h = crate::resolve_automation(&repo, tool.as_deref(), false, &without)?;
    match view {
        "summary" => Ok(status_summary(&h)),
        "kind" => {
            let kind = kind.ok_or("view=kind needs kind")?;
            let items: Vec<_> = h.items.iter().filter(|item| item.detail.kind() == kind).collect();
            if items.is_empty() {
                return Err(format!("no items of kind {kind}"));
            }
            Ok(json!({ "view": "kind", "tool": h.tool, "repo": h.target.repo, "kind": kind, "items": items }))
        }
        "item" => {
            let id = item_id.ok_or("view=item needs item")?;
            let item = h.items.iter().find(|item| item.id == id).ok_or_else(|| format!("no item with id {id}"))?;
            Ok(json!({ "view": "item", "tool": h.tool, "repo": h.target.repo, "item": item }))
        }
        "full" => Ok(serde_json::to_value(&h).expect("the model serialises")),
        _ => unreachable!("validated above"),
    }
}

/// `check`: the CI limits as one object, with `passed` for the whole row set.
fn check(args: &Map<String, Value>) -> Result<Value, String> {
    only(args, &["repo", "tool", "maxTokens", "maxItemTokens", "noWarnings", "strict", "without"])?;
    let strict = boolean(args, "strict")?;
    let no_warnings = boolean(args, "noWarnings")?;
    let max_tokens = integer(args, "maxTokens")?;
    let max_item = integer(args, "maxItemTokens")?;
    let without = strs(args, "without")?;
    let repo = arg_repo(args)?;
    let tool = arg_tool(args)?;
    let h = crate::resolve_automation(&repo, tool.as_deref(), strict, &without)?;
    Ok(crate::check_json(&h, max_tokens, max_item, no_warnings, strict))
}

/// `audit`: the ranked findings for one repo.
fn audit(args: &Map<String, Value>) -> Result<Value, String> {
    only(args, &["repo", "tool"])?;
    let repo = arg_repo(args)?;
    let tool = arg_tool(args)?;
    let quiet = &mut |_: rigswig_core::audit::Progress| {};
    let (a, _) = crate::audits::repo(&repo, tool.as_deref(), false, None, quiet)?;
    Ok(serde_json::to_value(&a).expect("serialises"))
}

/// `compare`: two repos with one harness, or two harnesses on one repo.
fn compare(args: &Map<String, Value>) -> Result<Value, String> {
    only(args, &["repo", "repoB", "tool", "tools"])?;
    let tools = strs(args, "tools")?;
    if args.contains_key("tools") && tools.len() != 2 {
        return Err("tools must contain exactly two harness ids".into());
    }
    if !tools.is_empty() && args.contains_key("repoB") {
        return Err("tools and repoB are alternative comparison modes".into());
    }
    let repo_b = text(args, "repoB")?;
    let tool = arg_tool(args)?;
    let (x, y) = (tools.first().cloned(), tools.get(1).cloned());
    let (a, b) = match (x, y) {
        (Some(x), Some(y)) => {
            let repo = arg_repo(args)?;
            (crate::resolve(&repo, Some(x.as_str()), false)?, crate::resolve(&repo, Some(y.as_str()), false)?)
        }
        _ => {
            let Some(rb) = repo_b else {
                return Err("compare needs two tools on one repo, or both repos".into());
            };
            (
                crate::resolve(&arg_repo(args)?, tool.as_deref(), false)?,
                crate::resolve(&PathBuf::from(rb), tool.as_deref(), false)?,
            )
        }
    };
    Ok(serde_json::to_value(rigswig_core::compare::compare(&a, &b)).expect("serialises"))
}

/// `tools`: the harnesses rigswig knows and the evidence for each in the repo.
fn tools(args: &Map<String, Value>) -> Result<Value, String> {
    only(args, &["repo"])?;
    let list = crate::detections(&arg_repo(args)?)?;
    let rows: Vec<Value> = list
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "score": row.score,
                "evidence": row.evidence.iter().map(|e| printable(e)).collect::<Vec<_>>(),
                "session": row.session.as_ref().map(|c| json!({
                    "scope": match c.scope { SessionScope::Repo => "repo", SessionScope::Machine => "machine" },
                    "evidence": c.evidence.iter().map(|e| printable(e)).collect::<Vec<_>>(),
                })),
            })
        })
        .collect();
    Ok(json!({ "tools": rows }))
}

fn only(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    for key in args.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown argument: {key}"));
        }
    }
    Ok(())
}

fn text<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value)),
        Some(Value::String(_)) => Err(format!("{key} must not be empty")),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn boolean(args: &Map<String, Value>, key: &str) -> Result<bool, String> {
    match args.get(key) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

fn integer(args: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match args.get(key) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

fn arg_repo(args: &Map<String, Value>) -> Result<PathBuf, String> {
    match text(args, "repo")? {
        Some(r) => Ok(PathBuf::from(r)),
        None => Ok(std::env::current_dir().unwrap_or_default()),
    }
}

fn arg_tool(args: &Map<String, Value>) -> Result<Option<String>, String> {
    Ok(text(args, "tool")?.map(str::to_string))
}

fn strs(args: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match args.get(key) {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| value.as_str().map(str::to_string).ok_or_else(|| format!("{key} must be an array of strings")))
            .collect(),
        Some(_) => Err(format!("{key} must be an array of strings")),
    }
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
      "description": "Summary-first view of what a coding-agent session loads: context totals and per-kind counts by default; drill down with view=kind/item, or explicitly request the full harness with view=full.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" },
          "tool": { "type": "string", "description": "The harness id, e.g. claude-code, codex, cursor, gemini, hermes, ollama, devin, dsh, deepseek, pi, omp" },
          "view": { "type": "string", "enum": ["summary", "kind", "item", "full"], "description": "summary (default), items of one kind, one exact item, or the full harness" },
          "kind": { "type": "string", "description": "The item kind; requires view=kind" },
          "item": { "type": "string", "description": "The exact item id; requires view=item" },
          "without": { "type": "array", "items": { "type": "string" }, "description": "What to take out before totalling: a plugin (name@marketplace), a layer (layer:user), a kind and name (skill:deploy) or an item id" }
      }, "required": [], "additionalProperties": false } },
    { "name": "check",
      "description": "CI-style limits for a checkout: stays within a total token budget and an item budget, and is warning-free. `passed` is the verdict.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" },
          "tool": { "type": "string", "description": "The harness id" },
          "maxTokens": { "type": "integer", "minimum": 0, "description": "The maximum total startup tokens" },
          "maxItemTokens": { "type": "integer", "minimum": 0, "description": "The maximum tokens for any single startup item" },
          "noWarnings": { "type": "boolean", "description": "Also fail when the resolution emits warnings" },
          "strict": { "type": "boolean", "description": "Fail on incomplete reads or ambiguous resolution; refuse an adapter without a completeness contract" },
          "without": { "type": "array", "items": { "type": "string" }, "description": "What to take out before totalling, as in status" }
      }, "required": [], "additionalProperties": false } },
    { "name": "audit",
      "description": "Ranked findings on a repo's harness setup: what costs context and nothing here references, plugins the repo's files do not call for, instruction files over budget, repeated text, dead references, piled-up permission rules; each with a saving and a fix.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" },
          "tool": { "type": "string", "description": "The harness id" }
      }, "required": [], "additionalProperties": false } },
    { "name": "compare",
      "description": "What differs: two repos with one harness, or two harnesses on one repo. Give `tools` as two harness ids for the latter.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder, used with `tools` (default: the process's working directory)" },
          "repoB": { "type": "string", "description": "The second repo folder (then `tools` must be absent, and `tool` covers both)" },
          "tool": { "type": "string", "description": "The harness id, for both repos" },
          "tools": { "type": "array", "items": { "type": "string" }, "description": "Two harness ids on one repo, e.g. [\"claude-code\", \"codex\"]" }
      }, "required": [], "additionalProperties": false } },
    { "name": "tools",
      "description": "The harnesses rigswig knows, and for each the detection score, the evidence found in the repo, and any fresh live-session claim.",
      "inputSchema": { "type": "object", "properties": {
          "repo": { "type": "string", "description": "The repo folder (default: the process's working directory)" }
      }, "required": [], "additionalProperties": false } },
    ])
}
