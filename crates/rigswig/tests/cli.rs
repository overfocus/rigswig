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
