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
            .env("USERPROFILE", s.0.join("home"))
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
            .env("USERPROFILE", s.0.join("home"))
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
        .env("USERPROFILE", s.0.join("home"))
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

impl Sandbox {
    fn cmd(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_rigswig"))
            .args(args)
            .env("HOME", self.0.join("home"))
            .env("USERPROFILE", self.0.join("home"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("ANTHROPIC_MODEL")
            .env_remove("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
            .env_remove("CLAUDE_CODE_PROJECT_DIR_NAME")
            .output()
            .unwrap()
    }
    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }
}

#[test]
fn audit_prints_ranked_findings_and_json() {
    let s = Sandbox::new("audit");
    s.write("repo/.claude/settings.local.json", r#"{"permissions":{"allow":["Bash(git push *)"]}}"#);
    s.write("repo/CLAUDE.md", "See `docs/missing.md`.\n");
    s.write("repo/docs/here.md", "x");
    let repo = s.path("repo");
    let out = s.cmd(&["audit", "--repo", &repo]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1. HIGH"), "{text}");
    assert!(text.contains("git push"), "{text}");
    assert!(text.contains("docs/missing.md"), "{text}");

    let json = s.cmd(&["audit", "--repo", &repo, "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(v["findings"][0]["code"], "permission-broad");
    assert_eq!(v["findings"][0]["severity"], "high");
}

#[test]
fn audit_fleet_takes_a_glob_and_skips_folders_without_a_harness() {
    let s = Sandbox::new("fleet");
    s.write("code/a/CLAUDE.md", "# a\n");
    s.write("code/b/CLAUDE.md", "# b\nAsk the helper-bot agent.\n");
    s.write("code/c/CLAUDE.md", "# c\n");
    std::fs::create_dir_all(s.0.join("code/plain")).unwrap();
    s.write("home/.claude/agents/helper-bot.md", "---\nname: helper-bot\ndescription: helps\ntools: Read\n---\n");
    let glob = s.path("code/*");
    let out = s.cmd(&["audit", "--repos", &glob]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("3 repos"), "{text}");
    assert!(text.contains("move to project scope in that one  -> b"), "{text}");
    assert!(text.contains("Skipped (no harness found): plain"), "{text}");
}

#[test]
fn private_folders_are_refused_and_skipped() {
    let s = Sandbox::new("private");
    s.write("code/open/CLAUDE.md", "# open\n");
    s.write("code/secret/CLAUDE.md", "# secret\n");
    s.write("home/.rigswig/private.json", &format!(r#"{{"private":[{}]}}"#, serde_json::json!(s.path("code/secret"))));
    let secret = s.path("code/secret");
    for cmd in ["status", "audit", "check"] {
        let out = s.cmd(&[cmd, "--repo", &secret]);
        assert_eq!(out.status.code(), Some(2), "{cmd} refuses a private folder");
        assert!(String::from_utf8_lossy(&out.stderr).contains("private.json"));
    }
    let out = s.cmd(&["audit", "--repos", &s.path("code/*")]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("1 repos") || text.contains("rigswig audit: 1 repo"), "{text}");
    assert!(text.contains("Skipped 1 private folder"), "{text}");
    assert!(!text.contains("secret"), "{text}");
}

/// A repo archive as GitHub serves it (`<repo>-<sha>/...`), with a symlinked CLAUDE.md pointing at `outside`.
fn archive_with_link(dir: &std::path::Path, outside: &std::path::Path) -> std::path::PathBuf {
    let mut b = tar::Builder::new(Vec::new());
    let mut add = |name: &str, data: &[u8]| {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, name, data).unwrap();
    };
    add("shop-abc123/AGENTS.md", b"# Shop\nSee `docs/gone.md`.\n");
    add("shop-abc123/docs/here.md", b"x");
    add("shop-abc123/.claude/settings.json", br#"{"permissions":{"allow":["Bash(git push *)"]},"model":"m"}"#);
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Symlink);
    h.set_size(0);
    h.set_mode(0o777);
    b.append_link(&mut h, "shop-abc123/CLAUDE.md", outside).unwrap();
    let tar = b.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut gz, &tar).unwrap();
    let path = dir.join("shop.tar.gz");
    std::fs::write(&path, gz.finish().unwrap()).unwrap();
    path
}

#[test]
fn an_archive_is_audited_in_memory_without_the_readers_setup_or_files() {
    let s = Sandbox::new("archive");
    // The reader's own setup and a private file the archive's link points at: neither may appear.
    s.write("home/.claude/agents/mine.md", "---\nname: CANARY-MY-AGENT\ndescription: mine\n---\n");
    s.write("home/.claude/CLAUDE.md", "CANARY-MY-INSTRUCTIONS\n");
    s.write("secret.md", "CANARY-SECRET see `docs/CANARY-path.md`\n");
    let archive = archive_with_link(&s.0, &s.0.join("secret.md"));
    let archive = archive.to_str().unwrap();

    let out = s.cmd(&["audit", "--archive", archive, "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("CANARY"), "{text}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["repo"], "shop.tar.gz");
    let codes: Vec<&str> = v["findings"].as_array().unwrap().iter().map(|f| f["code"].as_str().unwrap()).collect();
    assert!(codes.contains(&"permission-broad"), "{codes:?}");
    assert!(codes.contains(&"dead-reference"), "{codes:?}");
    assert!(!codes.contains(&"heavy-user-scope"), "the reader's agents are not read");
    assert!(v["notes"][0].as_str().unwrap().contains("1 links and special files dropped"), "{}", v["notes"]);

    let status = s.cmd(&["status", "--archive", archive, "--json"]);
    let h: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let names: Vec<&str> = h["items"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"AGENTS.md"), "{names:?}");
    assert!(!names.contains(&"CLAUDE.md"), "the link was dropped, so AGENTS.md is read instead: {names:?}");
    assert!(!String::from_utf8_lossy(&status.stdout).contains("CANARY"));
}

#[test]
fn github_input_is_checked_before_any_request_and_never_mixes_with_a_local_repo() {
    let s = Sandbox::new("github-input");
    for bad in ["https://evil.example/o/r", "file:///etc/passwd", "o/r@../x", "o"] {
        let out = s.cmd(&["audit", "--github", bad]);
        assert_eq!(out.status.code(), Some(2), "{bad}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.contains("downloading"), "refused before any request: {err}");
    }
    let repo = s.path("repo");
    let both = s.cmd(&["audit", "--github", "o/r", "--repo", &repo]);
    assert_eq!(both.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&both.stderr).contains("downloading"));
    // The commands people run every day never reach the downloader.
    for args in [vec!["status", "--repo", &repo], vec!["audit", "--repo", &repo], vec!["check", "--repo", &repo]] {
        let out = s.cmd(&args);
        assert!(!String::from_utf8_lossy(&out.stderr).contains("downloading"), "{args:?}");
    }
}

#[cfg(not(feature = "github"))]
#[test]
fn a_build_without_the_github_feature_cannot_fetch() {
    let s = Sandbox::new("no-network");
    let out = s.cmd(&["audit", "--github", "o/r"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("built without the github feature"));
    let page = s.cmd(&["ui", "--allow-github", "--idle", "1"]);
    assert_eq!(page.status.code(), Some(2), "the page cannot be started with downloads on");
    assert!(String::from_utf8_lossy(&page.stderr).contains("built without the github feature"));
}
