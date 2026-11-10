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

/// The environment variables every adapter reads, removed so a test child sees only the sandbox.
fn hermetic(mut cmd: std::process::Command) -> std::process::Command {
    for v in [
        "CLAUDE_CONFIG_DIR",
        "ANTHROPIC_MODEL",
        "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
        "CLAUDE_CODE_PROJECT_DIR_NAME",
        "CODEX_HOME",
        "HERMES_HOME",
        "HERMES_PROFILE",
        "HERMES_MANAGED_DIR",
        "HERMES_INFERENCE_MODEL",
        "HERMES_MODEL",
        "LOCALAPPDATA",
        "OLLAMA_MODELS",
        "OLLAMA_HOST",
        "OLLAMA_CONTEXT_LENGTH",
        "OLLAMA_KEEP_ALIVE",
        "OLLAMA_MAX_LOADED_MODELS",
        "OLLAMA_NUM_PARALLEL",
        "OLLAMA_NO_CLOUD",
    ] {
        cmd.env_remove(v);
    }
    cmd
}

/// A throwaway home and repo on the real file system, removed when dropped.
struct Sandbox(std::path::PathBuf);

impl Sandbox {
    fn new(tag: &str) -> Self {
        let s = Self::bare(tag);
        std::fs::create_dir_all(s.0.join("home/.claude")).unwrap();
        std::fs::create_dir_all(s.0.join("repo/.claude")).unwrap();
        s
    }

    /// The same throwaway tree minus every harness folder: an empty home and an empty repo.
    fn bare(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rigswig-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::create_dir_all(dir.join("repo")).unwrap();
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
        hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")))
            .args(&args)
            .env("HOME", self.0.join("home"))
            .env("USERPROFILE", self.0.join("home"))
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
    assert!(String::from_utf8_lossy(&out.stdout).contains("estimated tokens from startup files"));
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
        hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")))
            .args(args)
            .env("HOME", s.0.join("home"))
            .env("USERPROFILE", s.0.join("home"))
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
        hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")))
            .args(&all)
            .env("HOME", s.0.join("home"))
            .env("USERPROFILE", s.0.join("home"))
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

#[test]
fn strict_check_fails_on_incomplete_reads_not_advisories() {
    let s = Sandbox::new("strict-check");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    s.write("home/.claude/agents/a.md", "---\ndescription: helps\n---\n");
    let repo = s.real_repo();
    let run = |extra: &[&str]| {
        let mut args = vec!["check", "--repo", &repo, "--tool", "claude-code"];
        args.extend(extra);
        s.run(&args)
    };

    let advisory = run(&["--strict", "--json"]);
    assert!(advisory.status.success(), "an all-tools agent is advisory, not an incomplete read");
    let out: serde_json::Value = serde_json::from_slice(&advisory.stdout).unwrap();
    assert!(out["checks"].as_array().unwrap().iter().any(|c| c["check"] == "strict" && c["passed"] == true));

    s.write("repo/.claude/settings.json", "{not json");
    assert!(run(&[]).status.success(), "ordinary checks still permit warnings");
    let incomplete = run(&["--strict", "--json"]);
    assert_eq!(incomplete.status.code(), Some(1));
    let out: serde_json::Value = serde_json::from_slice(&incomplete.stdout).unwrap();
    let row = out["checks"].as_array().unwrap().iter().find(|c| c["check"] == "strict").unwrap();
    assert_eq!(row["passed"], false);
    assert!(row["detail"].as_str().unwrap().contains("incomplete-malformed-json"), "{row}");
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
    let mut child = hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")))
        .args(&args)
        .env("HOME", s.0.join("home"))
        .env("USERPROFILE", s.0.join("home"))
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
        hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")))
            .args(args)
            .env("HOME", self.0.join("home"))
            .env("USERPROFILE", self.0.join("home"))
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
    assert!(v["notes"][0].as_str().unwrap().contains("1 other links and special files dropped"), "{}", v["notes"]);

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

#[test]
fn a_tie_between_harnesses_picks_the_first_in_rigswigs_list() {
    // Both harnesses have their own folder in the repo: evidence 100 each.
    let s = Sandbox::new("tie");
    std::fs::create_dir_all(s.0.join("repo/.codex")).unwrap();
    let repo = s.path("repo");
    let out = s.cmd(&["status", "--repo", &repo]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("claude-code in repo"), "{text}");
    assert!(text.contains("Also configured here: codex"), "{text}");
    let audit = s.cmd(&["audit", "--repo", &repo]);
    assert!(String::from_utf8_lossy(&audit.stdout).starts_with("rigswig audit: claude-code"));
}

#[test]
fn each_crate_ships_the_repos_licence_and_notice_unchanged() {
    // A published crate holds only its own folder, so both crates carry copies; they must not drift.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["LICENSE", "NOTICE"] {
        let want = std::fs::read(root.join(name)).unwrap();
        for krate in ["crates/rigswig", "crates/rigswig-core"] {
            assert_eq!(std::fs::read(root.join(krate).join(name)).unwrap(), want, "{krate}/{name} differs from {name}");
        }
    }
}

#[test]
fn a_bare_machine_lists_the_readers_instead_of_reading_anything() {
    let s = Sandbox::bare("bare-machine");
    let out = s.status(&[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Nothing points at a harness"), "{text}");
    assert!(text.contains("rigswig status --tool <id> reads one directly:"));
    assert!(text.contains("claude-code") && text.contains("hf"), "{text}");
    // No harness is read: the render header (which names a harness) never appears.
    assert!(!text.contains("in repo"), "{text}");
}

#[test]
fn a_machine_evidence_pick_says_so_and_lists_the_rest() {
    // A bare sandbox: only home/.claude (machine evidence) and an unconnected second install; the repo has
    // no harness folders at all, so nothing repo-specific can claim it. The second install is hf, whose
    // ~/.agents/skills root is the same on every OS (devin's is not: %APPDATA% on Windows).
    let s = Sandbox::bare("guess");
    std::fs::create_dir_all(s.0.join("home/.claude")).unwrap();
    s.write(
        "home/.agents/skills/hf-cli/SKILL.md",
        "---\nname: hf-cli\ndescription: The Hub CLI.\n---\n\nGenerated with `huggingface_hub v2.4.0`.",
    );
    let out = s.status(&[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Picked by this machine's evidence"), "{text}");
    assert!(text.contains("claude-code"), "{text}");
    assert!(text.contains("hf") && text.contains("reading this one"), "{text}");
    assert!(text.contains("claude-code in repo"), "the guess still reads aloud\n{text}");
    // The ranked list replaces the quiet pick's footer.
    assert!(!text.contains("Also configured here"), "{text}");
}

#[test]
fn a_repo_setup_keeps_the_quiet_pick() {
    let s = Sandbox::new("quiet");
    s.write("repo/CLAUDE.md", "Use the test suite.");
    // An installed other harness for the footer to name; hf's root is the same on every OS.
    s.write(
        "home/.agents/skills/hf-cli/SKILL.md",
        "---\nname: hf-cli\ndescription: The Hub CLI.\n---\n\nGenerated with `huggingface_hub v2.4.0`.",
    );
    let out = s.status(&[]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("claude-code in repo"));
    // Repo-evidence picks stay exactly as they have been.
    assert!(!text.contains("Picked by this machine's evidence"));
    assert!(text.contains("Also configured here: hf"), "{text}\n");
}

#[test]
fn the_guess_is_silent_in_json_but_the_pick_is_stable() {
    let s = Sandbox::bare("bare-json");
    s.write(
        "home/.agents/skills/hf-cli/SKILL.md",
        "---\nname: hf-cli\ndescription: The Hub CLI.\n---\n\nGenerated with `huggingface_hub v2.4.0`.",
    );
    let out = s.status(&["--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let h: rigswig_core::model::Harness = serde_json::from_slice(&out.stdout).expect("valid harness JSON");
    assert_eq!(rigswig_core::check::contract_errors(&h), Vec::<String>::new());
    assert_eq!(h.tool.id, "hf");
}

impl Sandbox {
    fn run(&self, args: &[&str]) -> std::process::Output {
        hermetic(Command::new(env!("CARGO_BIN_EXE_rigswig")))
            .args(args)
            .env("HOME", self.0.join("home"))
            .env("USERPROFILE", self.0.join("home"))
            .output()
            .unwrap()
    }
    /// The sandbox repo's real path as the binary resolves it (`real_path` in main.rs): on Windows a plain drive
    /// path, `C:\repo`, never the verbatim `\\?\C:\repo` that `canonicalize` returns — Claude Code names its
    /// project folder from the plain one.
    fn real_repo(&self) -> String {
        let real = std::fs::canonicalize(self.0.join("repo")).unwrap().to_string_lossy().into_owned();
        match real.strip_prefix(r"\\?\") {
            Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
            _ => real,
        }
    }
    /// Where Claude Code keeps this sandbox repo's session logs.
    fn logs(&self) -> String {
        let name: String = self.real_repo().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        format!("home/.claude/projects/{name}")
    }
}

/// `rigswig usage` on the real file system: says what it reads before it reads, streams a log larger than the 4 MB
/// a repo file may be, and prints no message text.
#[test]
fn usage_reads_large_logs_as_metadata_only() {
    let s = Sandbox::new("usage");
    let real = s.real_repo();
    let repo = real.as_str();
    // The `cwd` as JSON: a Windows path's backslashes must be escaped.
    let cwd = serde_json::to_string(repo).unwrap();
    let filler = format!(
        r#"{{"type":"user","sessionId":"s1","timestamp":"2026-10-20T09:00:00Z","cwd":{cwd},"message":{{"role":"user","content":"CANARY-CLI {}"}}}}"#,
        "x".repeat(1000)
    );
    let mut log = String::new();
    while log.len() < 5 * 1024 * 1024 {
        log.push_str(&filler);
        log.push('\n');
    }
    log.push_str(&format!(
        r#"{{"type":"assistant","sessionId":"s1","timestamp":"2026-10-20T09:00:01Z","cwd":{cwd},"message":{{"id":"m1","model":"model-a","content":[{{"type":"tool_use","id":"c1","name":"Skill","input":{{"skill":"deploy","args":"CANARY-CLI"}}}}],"usage":{{"input_tokens":1,"output_tokens":2}}}}}}"#
    ));
    s.write(&format!("{}/s1.jsonl", s.logs()), &log);

    let out = s.run(&["usage", "--repo", repo, "--days", "all", "--json"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("metadata only"), "{err}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("CANARY"), "{stdout}");
    let u: rigswig_core::usage::Usage = serde_json::from_str(&stdout).unwrap();
    assert_eq!((u.files, u.files_unread, u.sessions), (1, 0, 1));
    assert_eq!(u.find("skill", "deploy").map(|i| i.calls), Some(1), "the call after 5 MB of lines is counted");

    let text = s.run(&["usage", "--repo", repo, "--days", "all"]);
    assert!(!String::from_utf8_lossy(&text.stdout).contains("CANARY"));
    let audit = s.run(&["audit", "--repo", repo, "--usage", "--days", "all", "--json"]);
    assert!(audit.status.success());
    assert!(!String::from_utf8_lossy(&audit.stdout).contains("CANARY"));
    // The audit's human output is held to the same bar as its JSON.
    let audit_text = s.run(&["audit", "--repo", repo, "--usage", "--days", "all"]);
    assert!(audit_text.status.success());
    assert!(!String::from_utf8_lossy(&audit_text.stdout).contains("CANARY"));
    // A `--days` older than the calendar begins is every log there is, not an overflow.
    let huge = s.run(&["usage", "--repo", repo, "--days", "9223372036854775808", "--json"]);
    assert!(huge.status.success(), "{}", String::from_utf8_lossy(&huge.stderr));
    let hu: rigswig_core::usage::Usage = serde_json::from_str(&String::from_utf8_lossy(&huge.stdout)).unwrap();
    assert_eq!(hu.sessions, 1);
}

#[test]
fn usage_flags_are_refused_where_they_do_not_belong() {
    let s = Sandbox::new("usage-flags");
    let repo = s.0.join("repo");
    let repo = repo.to_str().unwrap();
    assert_eq!(s.run(&["audit", "--repo", repo, "--days", "7"]).status.code(), Some(2), "--days without --usage");
    assert_eq!(s.run(&["audit", "--usage", "--repos", repo]).status.code(), Some(2), "not in fleet mode");
    assert_eq!(s.run(&["audit", "--usage", "--archive", "x.tar.gz"]).status.code(), Some(2), "not for an archive");
    assert_eq!(s.run(&["usage", "--repo", repo, "--days", "0"]).status.code(), Some(2));
    let none = s.run(&["usage", "--repo", repo]);
    assert_eq!(none.status.code(), Some(2), "no logs folder is an error, not an empty report");
    // Without --usage, the audit says nothing about session logs.
    let plain = s.run(&["audit", "--repo", repo]);
    assert!(!String::from_utf8_lossy(&plain.stderr).contains("session logs"));
}

// ---------------------------------------------------------------------------------------------------------------
// The live-session pick: interactive status/ui may follow a fresh repo-scoped claim. Automation always uses the
// repo's evidence, and every probe is metadata only — folder names, kinds and modification times, never file content.

use std::time::{Duration, SystemTime};

/// Stamp an existing file with a modification time `ago` seconds before now (fresh when ago is small).
fn stamp_ago(path: &std::path::Path, ago: u64) {
    let f = std::fs::OpenOptions::new().append(true).open(path).expect("reopen for set_times");
    f.set_times(std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(ago)))
        .expect("set_times");
}

/// One fresh Hermes-home artifact set: the daemon is up and a session is talking to it.
fn hermes_home_active(s: &Sandbox, ago: u64) {
    s.write("home/.hermes/state.db-wal", "");
    s.write("home/.hermes/gateway_state.json", "{}");
    stamp_ago(&s.0.join("home/.hermes/state.db-wal"), ago);
    stamp_ago(&s.0.join("home/.hermes/gateway_state.json"), ago);
}

/// The projects-folder name of a repo path, the way Claude Code writes it: every character that is not an ASCII
/// letter or digit becomes `-` (docs/adapters/claude-code.md, **inferred**).
fn claude_slug(repo: &std::path::Path) -> String {
    repo.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// One fresh Claude Code session log in the repo's own projects folder. The slug comes from the repo path as the
/// CLI sees it: canonicalized, and on Windows with the verbatim `\\?\C:\` prefix stripped (the same rule the
/// binary's `real_path` applies), since that is the cwd shape a working Claude session writes its folder from.
fn claude_session_in_repo(s: &Sandbox, ago: u64) {
    let mut repo = std::fs::canonicalize(s.0.join("repo")).unwrap();
    if cfg!(windows) {
        let text = repo.to_string_lossy().into_owned();
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                repo = std::path::PathBuf::from(rest);
            }
        }
    }
    let slug = claude_slug(&repo);
    let rel = format!("home/.claude/projects/{slug}/live.jsonl");
    s.write(&rel, r#"not a log the probe may ever open: PLANTED-CONTENT-CANARY"#);
    stamp_ago(&s.0.join(&rel), ago);
}

#[test]
fn machine_activity_does_not_move_the_default_pick() {
    let s = Sandbox::new("session");
    s.write("repo/CLAUDE.md", "the repo is set up for claude-code");
    hermes_home_active(&s, 30);
    let out = s.status(&[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("claude-code in repo"), "the repo's own evidence wins: {text}");
    assert!(!text.contains("Session   "), "machine activity is not a claim on this repo: {text}");
    let plain = s.status(&["--no-session"]);
    assert_eq!(plain.stdout, out.stdout, "--no-session is the same deterministic reading");
}

#[test]
fn check_and_audit_ignore_a_repo_scoped_live_session() {
    let s = Sandbox::new("automation-session");
    std::fs::remove_dir(s.0.join("repo/.claude")).unwrap();
    s.write("repo/AGENTS.md", "Codex rules");
    s.write("repo/.codex/config.toml", "");
    claude_session_in_repo(&s, 30);
    let repo = s.real_repo();

    let interactive = s.status(&["--json"]);
    let h: rigswig_core::model::Harness = serde_json::from_slice(&interactive.stdout).unwrap();
    assert_eq!(h.tool.id, "claude-code", "interactive status may follow this repo's live session");

    for command in ["check", "audit"] {
        let out = s.run(&[command, "--repo", &repo, "--json"]);
        assert!(out.status.success(), "{command}: {}", String::from_utf8_lossy(&out.stderr));
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["tool"], "codex", "{command} must use repo evidence, not live activity");
    }
}

#[test]
fn a_stale_session_does_not_move_anything() {
    let s = Sandbox::new("session-stale");
    s.write("repo/CLAUDE.md", "claude-code's");
    hermes_home_active(&s, 16 * 60); // outside the 15-minute window
    let out = s.status(&[]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("claude-code in repo"), "{text}");
    assert!(text.contains("hermes"), "the run still lists what it edged out: {text}");
}

#[test]
fn the_extension_reads_the_same_pick_in_json() {
    let s = Sandbox::new("session-json");
    s.write("repo/CLAUDE.md", "claude-code's own file");
    claude_session_in_repo(&s, 30);
    let out = s.status(&["--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let h: rigswig_core::model::Harness = serde_json::from_slice(&out.stdout).expect("valid harness JSON");
    assert_eq!(rigswig_core::check::contract_errors(&h), Vec::<String>::new());
    assert_eq!(h.tool.id, "claude-code");
    let session = h.tool.session.expect("the JSON carries the repo-scoped claim the pick followed");
    assert_eq!(session.scope, rigswig_core::model::SessionScope::Repo);
    assert!(!session.evidence.is_empty());
    // An explicit --tool never carries a claim.
    let out = s.status(&["--json", "--tool", "claude-code"]);
    let h: rigswig_core::model::Harness = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(h.tool.id, "claude-code");
    assert!(h.tool.session.is_none());
}

#[test]
fn a_session_log_below_the_repo_claims_the_repo() {
    let s = Sandbox::new("session-repo");
    hermes_home_active(&s, 30);
    claude_session_in_repo(&s, 30);
    let out = s.status(&["--json"]);
    let h: rigswig_core::model::Harness = serde_json::from_slice(&out.stdout).expect("valid harness JSON");
    assert_eq!(h.tool.id, "claude-code", "repo-scoped beats machine-scoped");
    let session = h.tool.session.expect("the claim rides along");
    assert_eq!(session.scope, rigswig_core::model::SessionScope::Repo);
    assert!(session.evidence.iter().any(|e| e.ends_with("session file")), "have {:#?}", session.evidence);
    // And the planted content still never reaches anything rigswig prints or keeps.
    let text = s.status(&[]);
    assert!(text.status.success());
    let all = [&text.stdout[..], &text.stderr[..]].concat();
    assert!(!String::from_utf8_lossy(&all).contains("PLANTED-CONTENT-CANARY"), "session-log text never prints");
}
