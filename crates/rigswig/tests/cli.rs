use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rigswig")).args(args).output().expect("the binary runs")
}

#[test]
fn version_prints_the_crate_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}\n", rigswig_core::VERSION));
}

#[test]
fn no_command_prints_usage() {
    let out = run(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("rigswig status"));
}

#[test]
fn unknown_command_exits_2_without_control_characters() {
    let out = run(&["no\u{1b}]0;pwned\u{7}pe"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown or unimplemented command: no]0;pwnedpe"));
    assert!(!err.contains('\u{1b}'));
}

/// A throwaway home and repo on the real file system, removed when dropped.
struct Sandbox(std::path::PathBuf);

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rigswig-test-{tag}-{}", std::process::id()));
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
    fn status(&self, extra: &[&str]) -> std::process::Output {
        let repo = self.0.join("repo");
        let mut args = vec!["status", "--repo", repo.to_str().unwrap()];
        args.extend(extra);
        Command::new(env!("CARGO_BIN_EXE_rigswig"))
            .args(&args)
            .env("HOME", self.0.join("home"))
            .env("USERPROFILE", self.0.join("home"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("ANTHROPIC_MODEL")
            .output()
            .unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn status_json_on_a_real_folder_keeps_the_contract() {
    let s = Sandbox::new("json");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    s.write("repo/.claude/settings.json", r#"{"model":"m-1","permissions":{"deny":["Bash(rm:*)"]}}"#);
    s.write("home/.claude/agents/a.md", "---\ndescription: helps\n---\n");
    let out = s.status(&["--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let h: rigswig_core::model::Harness = serde_json::from_slice(&out.stdout).expect("valid harness JSON");
    assert_eq!(rigswig_core::check::contract_errors(&h), Vec::<String>::new());
    assert_eq!(h.tool.id, "claude-code");
    assert_eq!(h.settings["model"].value, "m-1");
    assert_eq!(h.target.repo, "repo");
    assert!(h.items.iter().any(|i| i.id == "claude-code:agent:user:a"));
}

#[test]
fn status_text_and_bad_options() {
    let s = Sandbox::new("text");
    s.write("repo/CLAUDE.md", "x");
    let out = s.status(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("claude-code in repo"));
    assert_eq!(s.status(&["--tool", "nope"]).status.code(), Some(2));
    assert_eq!(s.status(&["--bogus"]).status.code(), Some(2));
}

#[test]
fn every_adapter_renders_with_no_config_at_all() {
    let s = Sandbox::new("empty");
    for tool in ["claude-code", "codex"] {
        for extra in [&[][..], &["--json"][..]] {
            let mut args = vec!["--tool", tool];
            args.extend(extra);
            let out = s.status(&args);
            assert!(out.status.success(), "{tool} {extra:?}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }
}

#[test]
fn compare_two_repos_and_two_tools() {
    let s = Sandbox::new("compare");
    s.write("repo/CLAUDE.md", "one");
    s.write("repo/AGENTS.md", "shared");
    s.write("other/CLAUDE.md", "two, longer");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rigswig"))
            .args(args)
            .env("HOME", s.0.join("home"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap()
    };
    let (repo, other) = (s.0.join("repo"), s.0.join("other"));
    let out = run(&["compare", repo.to_str().unwrap(), other.to_str().unwrap(), "--tool", "claude-code", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let c: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(c["changed"][0]["name"], "CLAUDE.md");

    let out = run(&["compare", repo.to_str().unwrap(), "--tools", "claude-code,codex"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("A  claude-code in repo") && text.contains("B  codex in repo"));

    assert_eq!(run(&["compare", repo.to_str().unwrap()]).status.code(), Some(2), "one repo needs --tools");
}

#[test]
fn check_exit_codes_and_without() {
    let s = Sandbox::new("check");
    s.write("repo/CLAUDE.md", &"x".repeat(4_000)); // ~1,000 tokens
    s.write("home/.claude/agents/a.md", "---\ndescription: helps\n---\n");
    let repo = s.0.join("repo");
    let run = |args: &[&str]| {
        let mut all = vec!["check", "--repo", repo.to_str().unwrap(), "--tool", "claude-code"];
        all.extend(args);
        Command::new(env!("CARGO_BIN_EXE_rigswig"))
            .args(&all)
            .env("HOME", s.0.join("home"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .output()
            .unwrap()
    };
    assert_eq!(run(&["--max-tokens", "2k"]).status.code(), Some(0));
    assert_eq!(run(&["--max-tokens", "900"]).status.code(), Some(1));
    assert_eq!(run(&["--max-item-tokens", "500"]).status.code(), Some(1));
    assert_eq!(run(&["--max-item-tokens", "500", "--without", "instruction:CLAUDE.md"]).status.code(), Some(0));
    assert_eq!(run(&["--no-warnings"]).status.code(), Some(1), "the all-tools agent is a warning");
    assert_eq!(
        run(&["--no-warnings", "--without", "layer:user"]).status.code(),
        Some(1),
        "warnings come from the resolved setup"
    );
    assert_eq!(run(&["--max-tokens", "lots"]).status.code(), Some(2));
    let out = run(&["--max-tokens", "900", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["passed"], false);
    assert_eq!(v["checks"][1]["check"], "max-tokens");
}

/// A tiny HTTP client for the end-to-end test: one request, the status and the body.
fn http(port: u16, method: &str, path: &str, headers: &str, body: &str) -> (u16, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    (status, out.split("\r\n\r\n").nth(1).unwrap_or("").to_string())
}

/// Start `rigswig ui` in the sandbox; returns the child, the port and the token from the line it prints.
fn start_ui(s: &Sandbox, extra: &[&str]) -> (std::process::Child, u16, String) {
    use std::io::BufRead;
    let repo = s.0.join("repo");
    let mut args = vec!["ui", "--repo", repo.to_str().unwrap(), "--tool", "claude-code", "--idle", "1"];
    args.extend(extra);
    let mut child = Command::new(env!("CARGO_BIN_EXE_rigswig"))
        .args(&args)
        .env("HOME", s.0.join("home"))
        .env_remove("CLAUDE_CONFIG_DIR")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line).unwrap();
    let url = line.split_whitespace().last().unwrap().to_string();
    let port = url.split(':').nth(2).unwrap().split('/').next().unwrap().parse().unwrap();
    let token = url.split("t=").nth(1).unwrap().to_string();
    (child, port, token)
}

#[test]
fn write_mode_plans_applies_backs_up_and_undoes() {
    let s = Sandbox::new("writes");
    let mk = s.0.join("home/mk");
    // The path goes into JSON, so it is JSON-escaped (Windows paths have backslashes).
    let mk_json = serde_json::to_string(&mk.to_string_lossy()).unwrap();
    s.write(
        "home/.claude/plugins/known_marketplaces.json",
        &format!(r#"{{"mk":{{"source":{{"source":"directory","path":{mk_json}}}}}}}"#),
    );
    s.write("home/mk/.claude-plugin/marketplace.json", r#"{"plugins":[{"name":"fmt","source":"./fmt"}]}"#);
    s.write("home/mk/fmt/skills/pretty/SKILL.md", &format!("---\ndescription: {}\n---\n", "formats ".repeat(50)));
    let user_settings = "{\n  \"enabledPlugins\": {\n    \"fmt@mk\": true\n  }\n}\n";
    s.write("home/.claude/settings.json", user_settings);

    // Read-only by default: the write routes do not exist.
    let (mut ro, port, token) = start_ui(&s, &[]);
    let hdr = |port: u16, token: &str| {
        format!("Origin: http://127.0.0.1:{port}\r\nX-Rigswig-Token: {token}\r\nContent-Type: application/json\r\n")
    };
    let change = r#"{"kind":"plugin","id":"fmt@mk","scope":"user","on":false}"#;
    assert_eq!(http(port, "POST", "/api/plan", &hdr(port, &token), change).0, 404);
    let _ = ro.kill();

    let (mut rw, port, token) = start_ui(&s, &["--allow-writes"]);
    let (code, plan) = http(port, "POST", "/api/plan", &hdr(port, &token), change);
    assert_eq!(code, 200, "{plan}");
    let plan: serde_json::Value = serde_json::from_str(&plan).unwrap();
    assert_eq!(plan["file"], "~/.claude/settings.json");
    assert_eq!(plan["takesEffect"], true);
    assert!(plan["tokensAfter"].as_u64() < plan["tokensBefore"].as_u64());
    assert!(
        plan.get("after").is_none() && plan.get("path").is_none(),
        "the new contents and the real path stay on the server"
    );

    let apply = format!(r#"{{"change":{change},"beforeHash":{}}}"#, plan["beforeHash"]);
    let (code, done) = http(port, "POST", "/api/apply", &hdr(port, &token), &apply);
    assert_eq!(code, 200, "{done}");
    let written = std::fs::read_to_string(s.0.join("home/.claude/settings.json")).unwrap();
    assert!(written.contains("\"fmt@mk\": false"));
    let backups: Vec<_> = std::fs::read_dir(s.0.join("home/.rigswig/backups")).unwrap().collect();
    assert_eq!(backups.len(), 1);

    assert_eq!(http(port, "POST", "/api/apply", &hdr(port, &token), &apply).0, 409, "a stale plan is refused");

    let (code, _) = http(port, "POST", "/api/undo", &hdr(port, &token), "{}");
    assert_eq!(code, 200);
    assert_eq!(std::fs::read_to_string(s.0.join("home/.claude/settings.json")).unwrap(), user_settings);
    assert_eq!(http(port, "POST", "/api/undo", &hdr(port, &token), "{}").0, 409, "nothing left to undo");
    let _ = rw.kill();
}
