//! `rigswig mcp` on the wire: spawn the binary, speak newline-delimited JSON-RPC, and check every message.
//!
//! One helper run per test keeps the protocol state small: the server answers one request per line, flushes
//! after each, and stays silent for notifications.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

/// The environment variables every adapter reads, removed so a test child sees only the sandbox.
fn hermetic(mut cmd: Command) -> Command {
    for v in [
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
        "CLAUDE_CODE_PROJECT_DIR_NAME",
        "CODEX_HOME",
        "HERMES_HOME",
        "HERMES_PROFILE",
        "HERMES_INFERENCE_MODEL",
        "HERMES_MODEL",
        "OLLAMA_MODELS",
        "OLLAMA_HOST",
        "OLLAMA_CONTEXT_LENGTH",
        "OLLAMA_KEEP_ALIVE",
        "OLLAMA_MAX_LOADED_MODELS",
        "OLLAMA_NUM_PARALLEL",
        "OLLAMA_NO_CLOUD",
        "DEEPSEEK_MODEL",
        "DEEPSEEK_BASE_URL",
    ] {
        cmd.env_remove(v);
    }
    cmd
}

struct Server {
    child: Child,
    stdin: Box<dyn Write + Send>,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn with_repo(repo: &std::path::Path, home: &std::path::Path) -> Server {
        let mut cmd = hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")));
        let mut child = cmd
            .arg("mcp")
            .current_dir(repo)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the binary spawns");
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Server { child, stdin: Box::new(stdin), stdout }
    }

    /// A request in, its response out (the server flushed it, so this read never waits on the process's exit).
    fn ask(&mut self, body: &str) -> Value {
        writeln!(self.stdin, "{body}").expect("the server still reads");
        self.stdin.flush().expect("the pipe takes it");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("a response line arrives");
        serde_json::from_str::<Value>(&line).expect("every stdout line is a JSON-RPC message")
    }

    /// A notification in, nothing out: the next response is whatever comes after it.
    fn notify(&mut self, body: &str) {
        writeln!(self.stdin, "{body}").expect("the server still reads");
        self.stdin.flush().expect("the pipe takes it");
    }

    fn request(&mut self, id: &str, method: &str, params: &str) -> Value {
        self.ask(&format!(r#"{{"jsonrpc":"2.0","id":"{id}","method":"{method}","params":{params}}}"#))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stdin = Box::new(std::io::sink()); // drop the pipe first: closing stdin shuts the server down
        let _ = self.child.wait();
    }
}

/// A throwaway home and repo on the real file system, removed when dropped.
struct Sandbox(std::path::PathBuf);

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rigswig-mcp-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home/.claude")).unwrap();
        std::fs::create_dir_all(dir.join("repo/.claude")).unwrap();
        Self(dir)
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn server(&self) -> Server {
        let repo = self.0.join("repo");
        let home = self.0.join("home");
        Server::with_repo(&repo, &home)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn initialized(s: &mut Server) -> Value {
    s.request(
        "i1",
        "initialize",
        r#"{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}"#,
    )
}

#[test]
fn the_handshake_negotiates_the_version_and_names_itself() {
    let s = Sandbox::new("handshake");
    let mut srv = s.server();
    let r = initialized(&mut srv);
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18", "a supported request is echoed");
    assert_eq!(r["result"]["serverInfo"]["name"], "rigswig");
    assert_eq!(r["result"]["capabilities"]["tools"], serde_json::json!({}));

    let unknown = srv.request(
        "i2",
        "initialize",
        r#"{"protocolVersion":"1999-01-01","capabilities":{},"clientInfo":{"name":"test","version":"0"}}"#,
    );
    assert_eq!(
        unknown["result"]["protocolVersion"], "2025-06-18",
        "an unsupported request gets the server's fallback, not an error"
    );
}

#[test]
fn a_notification_silences_and_the_next_answer_is_the_next_request_s() {
    let s = Sandbox::new("notification");
    let mut srv = s.server();
    let r = initialized(&mut srv);
    assert_eq!(r["id"], "i1");
    srv.notify(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    let ping = srv.request("p1", "ping", "{}");
    // The ping's reply is the very first line after the writes; the notification produced no line at all.
    assert_eq!(ping["id"], "p1");
    assert_eq!(ping["result"], serde_json::json!({}));
}

#[test]
fn the_catalog_lists_exactly_the_five_tools_with_schemas() {
    let s = Sandbox::new("catalog");
    let mut srv = s.server();
    initialized(&mut srv);
    let r = srv.request("l1", "tools/list", "{}");
    let tools = r["result"]["tools"].as_array().expect("the tools");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap_or_default()).collect();
    assert_eq!(names, ["status", "check", "audit", "compare", "tools"]);
    for t in tools {
        assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()), "every tool says what it does");
        assert_eq!(t["inputSchema"]["type"], "object");
    }
}

#[test]
fn status_returns_the_harness_json_and_whatif_takes_items_out() {
    let s = Sandbox::new("status");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    s.write("home/.claude/agents/a.md", "---\ndescription: helps\n---\n");
    let mut srv = s.server();
    initialized(&mut srv);

    let r = srv.request("c1", "tools/call", r#"{"name":"status","arguments":{}}"#);
    assert_eq!(r["result"]["isError"], false);
    let h: Value =
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("the text")).expect("harness JSON");
    assert_eq!(h["tool"]["id"], "claude-code");
    assert!(h["items"].as_array().expect("items").iter().any(|i| i["id"] == "claude-code:agent:user:a"));

    let r2 = srv.request("c2", "tools/call", r#"{"name":"status","arguments":{"without":["layer:user"]}}"#);
    let h2: Value = serde_json::from_str(r2["result"]["content"][0]["text"].as_str().expect("the text")).unwrap();
    // A taken-out item stays in the list, but nothing from the user layer is in effect any more.
    for i in h2["items"].as_array().expect("items") {
        if i["origin"]["layer"] == "user" {
            assert_eq!(i["effective"], false, "{} should be taken out", i["id"]);
        }
    }
}

#[test]
fn reads_use_repo_evidence_not_a_repo_scoped_live_session() {
    let s = Sandbox::new("deterministic");
    std::fs::remove_dir(s.0.join("repo/.claude")).unwrap();
    s.write("repo/AGENTS.md", "Codex rules");
    s.write("repo/.codex/config.toml", "");
    let mut repo = std::fs::canonicalize(s.0.join("repo")).unwrap();
    if cfg!(windows) {
        let text = repo.to_string_lossy().into_owned();
        if let Some(rest) = text.strip_prefix(r"\\\\?\\") {
            if rest.as_bytes().get(1) == Some(&b':') {
                repo = std::path::PathBuf::from(rest);
            }
        }
    }
    let slug: String =
        repo.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.write(&format!("home/.claude/projects/{slug}/live.jsonl"), "metadata only");
    let mut srv = s.server();
    initialized(&mut srv);

    for (id, tool) in [("d1", "status"), ("d2", "check"), ("d3", "audit")] {
        let r = srv.request(id, "tools/call", &format!(r#"{{"name":"{tool}","arguments":{{}}}}"#));
        let out: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        if tool == "status" {
            assert_eq!(out["tool"]["id"], "codex", "status followed live activity instead of repo evidence");
            assert!(out["tool"]["session"].is_null(), "automation carries no live-session claim");
        } else {
            assert_eq!(out["tool"], "codex", "{tool} followed live activity instead of repo evidence");
        }
    }
}

#[test]
fn check_reports_the_verdict_and_the_budget_rows() {
    let s = Sandbox::new("check");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    let mut srv = s.server();
    initialized(&mut srv);

    let r = srv.request("k1", "tools/call", r#"{"name":"check","arguments":{}}"#);
    let out: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("the text")).unwrap();
    assert_eq!(out["passed"], true, "an empty checkout keeps every default limit");
    assert!(out["checks"].as_array().expect("rows").iter().any(|c| c["check"] == "contract"));

    let r2 = srv.request("k2", "tools/call", r#"{"name":"check","arguments":{"maxTokens":1}}"#);
    let out2: Value = serde_json::from_str(r2["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(out2["passed"], false, "a one-token budget does not hold");
    assert!(out2["checks"].as_array().unwrap().iter().any(|c| c["check"] == "max-tokens" && c["passed"] == false));
}

#[test]
fn audit_returns_findings_and_tools_lists_the_harnesses() {
    let s = Sandbox::new("audit");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    let mut srv = s.server();
    initialized(&mut srv);

    let r = srv.request("a1", "tools/call", r#"{"name":"audit","arguments":{}}"#);
    let out: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("the text")).unwrap();
    assert_eq!(r["result"]["isError"], false);
    assert!(out.get("findings").is_some() || out.get("items").is_some(), "an audit carries its findings");

    let r2 = srv.request("a2", "tools/call", r#"{"name":"tools","arguments":{}}"#);
    let list: Vec<Value> = r2["result"]["content"][0]["text"]
        .as_str()
        .map(|t| serde_json::from_str::<Value>(t).expect("tools JSON")["tools"].clone())
        .and_then(|v| v.as_array().cloned())
        .expect("the harness list");
    let ids: Vec<&str> = list.iter().map(|t| t["id"].as_str().unwrap_or_default()).collect();
    assert!(ids.contains(&"claude-code"));
    assert!(ids.contains(&"omp"), "the newest adapter rides along");
}

#[test]
fn strict_check_reports_incomplete_reads() {
    let s = Sandbox::new("strict-check");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    s.write("repo/.claude/settings.json", "{not json");
    let mut srv = s.server();
    initialized(&mut srv);

    let r = srv.request("s1", "tools/call", r#"{"name":"check","arguments":{"strict":true}}"#);
    let out: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(out["passed"], false);
    let row = out["checks"].as_array().unwrap().iter().find(|c| c["check"] == "strict").unwrap();
    assert_eq!(row["passed"], false);
    assert!(row["detail"].as_str().unwrap().contains("incomplete-malformed-json"), "{row}");
}

#[test]
fn compare_answers_both_ways() {
    let s = Sandbox::new("compare");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    let mut srv = s.server();
    initialized(&mut srv);

    let r = srv.request("m1", "tools/call", r#"{"name":"compare","arguments":{"tools":["claude-code","codex"]}}"#);
    assert_eq!(r["result"]["isError"], false);
    let out: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().expect("the text")).unwrap();
    assert!(out.get("rows").is_some() || out.get("kinds").is_some(), "a comparison carries its shape");

    let r2 = srv.request(
        "m2",
        "tools/call",
        r#"{"name":"compare","arguments":{"repo":"no-such-folder-xyz","repoB":".","tool":"claude-code"}}"#,
    );
    assert_eq!(r2["result"]["isError"], true, "a folder that does not exist is a tool error, not a crash");
}

#[test]
fn protocol_errors_are_errors_and_unknown_tools_are_tool_errors() {
    let s = Sandbox::new("errors");
    let mut srv = s.server();
    initialized(&mut srv);

    let unknown = srv.request("u1", "resources/list", "{}");
    assert_eq!(unknown["error"]["code"], -32601, "a method the server does not have");

    let bad = srv.ask("this is not JSON");
    assert_eq!(bad["error"]["code"], -32700, "a line that is not JSON gets the parse error");

    let array = srv.ask(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#);
    assert_eq!(array["error"]["code"], -32600, "a batch is rejected outright");

    let tool = srv.request("u2", "tools/call", r#"{"name":"nope","arguments":{}}"#);
    assert_eq!(tool["result"]["isError"], true, "an unknown tool is a tool error, not a protocol one");
    assert!(tool["result"]["content"][0]["text"].as_str().unwrap_or_default().contains("unknown tool: nope"));
}

#[test]
fn every_stdout_line_is_a_message_with_the_wire_fields() {
    let s = Sandbox::new("purity");
    s.write("repo/CLAUDE.md", "x");
    let mut srv = s.server();
    // Nothing the tools run may print to stdout: each of these answers must be the exact next line.
    initialized(&mut srv);
    srv.notify(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    for (i, call) in [
        r#"{"name":"status","arguments":{}}"#,
        r#"{"name":"check","arguments":{}}"#,
        r#"{"name":"audit","arguments":{}}"#,
        r#"{"name":"tools","arguments":{}}"#,
        r#"{"name":"compare","arguments":{"tools":["claude-code","codex"]}}"#,
    ]
    .into_iter()
    .enumerate()
    {
        let r = srv.request(&format!("w{i}"), "tools/call", call);
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["result"]["isError"], false, "{}", r["result"]["content"][0]["text"]);
        assert_eq!(r["id"], format!("w{i}"), "no tool printed a line into the protocol");
    }
}
