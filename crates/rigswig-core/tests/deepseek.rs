//! The DeepSeek-CLI adapter on invented setups, one rule per test (docs/adapters/deepseek-cli.md). Every
//! output keeps the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::DeepSeekCli;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const U: &str = "/home/dev/.deepseek-cli";

fn ctx<'a>(env: &[(&str, &str)], fs: &'a MemFs) -> Ctx<'a> {
    Ctx {
        fs,
        env: env.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect::<BTreeMap<_, _>>(),
        home: PathBuf::from(HOME),
        repo: PathBuf::from(REPO),
        os: Os::Linux,
    }
}

fn run_env(env: &[(&str, &str)], fs: &MemFs) -> Harness {
    let h = DeepSeekCli.resolve(&ctx(env, fs));
    assert_eq!(contract_errors(&h), Vec::<String>::new(), "contract");
    h
}

fn run(fs: &MemFs) -> Harness {
    run_env(&[], fs)
}

fn item<'h>(h: &'h Harness, id: &str) -> &'h Item {
    h.items
        .iter()
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}; have {:#?}", h.items.iter().map(|i| &i.id).collect::<Vec<_>>()))
}

fn detect(fs: &MemFs) -> u8 {
    DeepSeekCli.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new().dir(REPO)), 0);
    let home = MemFs::new().dir(REPO).dir(U);
    assert_eq!(detect(&home), 45, "an installed CLI alone is home-folder evidence");
    let md = MemFs::new().dir(REPO).file(format!("{REPO}/deepseek.md"), "Project rules.");
    assert_eq!(detect(&md), 60);
    let repo = MemFs::new().dir(REPO).file(format!("{REPO}/.deepseek/config.json"), "{}");
    assert_eq!(detect(&repo), 100, "the repo's own .deepseek/ is repo evidence");
}

#[test]
fn config_layers_follow_the_documented_rank() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/config.json"), "{}")
        .file(format!("{REPO}/.deepseek/config.json"), "{}")
        .file(format!("{REPO}/.claude/skills/x.md"), "x");
    let h = run(&fs);
    for id in ["user", "project", "imports_project", "imports_user"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
}

#[test]
fn the_project_instruction_file_is_the_first_of_the_documented_list() {
    let fs = MemFs::new().dir(REPO).file(format!("{REPO}/AGENTS.md"), "Standard rules.");
    let h = run(&fs);
    item(&h, "deepseek:instruction:project:AGENTS.md");
    assert!(h.items.iter().filter(|i| i.detail.kind() == "instruction").count() == 1, "one file wins");
    assert_eq!(
        h.items.iter().find(|i| i.detail.kind() == "instruction").map(|i| &i.load.when),
        Some(&LoadWhen::Startup)
    );

    // deepseek.md on its own wins when AGENTS.md is absent, ahead of CLAUDE.md and the rest.
    let fs2 = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/deepseek.md"), "DeepSeek rules.")
        .file(format!("{REPO}/CLAUDE.md"), "Claude.");
    let h2 = run(&fs2);
    assert_eq!(
        h2.items.iter().find(|i| i.detail.kind() == "instruction").map(|i| &i.name),
        Some(&"deepseek.md".to_string())
    );
}

#[test]
fn settings_resolve_env_over_project_over_user() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/config.json"), r#"{"defaultModel": "deepseek-chat", "approvalMode": "auto"}"#)
        .file(format!("{REPO}/.deepseek/config.json"), r#"{"defaultModel": "deepseek-reasoner"}"#);
    let h = run(&fs);
    assert_eq!(h.settings["model"].value, "deepseek-reasoner", "the project config wins");
    let trail: Vec<&str> = h.settings["model"].trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(trail, ["project", "user"]);

    let h2 = run_env(&[("DEEPSEEK_MODEL", "deepseek-chat-v4")], &fs);
    assert_eq!(h2.settings["model"].value, "deepseek-chat-v4", "the env var outranks both files");
    assert_eq!(h2.settings["model"].source.as_deref(), Some("env"));
    assert_eq!(h2.settings["approvalMode"].value, "auto");
}

#[test]
fn the_long_term_memory_loads_at_session_start() {
    // The loader hashes the working directory with sha256 and keeps the first 12 hex characters.
    let hash = &rigswig_core::read::redact::sha256_hex(REPO)[..12];
    let fs =
        MemFs::new().dir(REPO).file(format!("{U}/memories/{hash}/MEMORY.md"), "Conventions learned in past sessions.");
    let h = run(&fs);
    let m = item(&h, &format!("deepseek:instruction:user:memories/{hash}/MEMORY.md"));
    assert_eq!(m.load.when, LoadWhen::Startup);
    // A different project's memory directory stays unread.
    assert!(h.items.iter().filter(|i| i.detail.kind() == "instruction" && i.effective).count() == 1);
}

#[test]
fn skills_scan_in_the_documented_priority_and_the_directory_layout_works() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/skills/deploy.md"), "Global deploy steps.")
        .file(format!("{REPO}/.deepseek/skills/deploy.md"), "Project deploy twin.")
        .file(format!("{REPO}/.claude/skills/review/SKILL.md"), "Review steps.")
        .file(format!("{REPO}/.codex/skills/greet.md"), "Greet.");
    let h = run(&fs);
    let deploy = item(&h, "deepseek:skill:user:deploy");
    assert!(deploy.effective, "the deepseek global dir wins the name clash");
    let twin =
        h.items.iter().find(|i| i.id.starts_with("deepseek:skill:project:deploy")).expect("the project twin listed");
    assert!(!twin.effective);
    item(&h, "deepseek:skill:imports_project:review");
    item(&h, "deepseek:skill:imports_project:greet");
}

#[test]
fn hooks_read_the_two_flat_files_and_the_project_runs_first() {
    let fs = MemFs::new().dir(REPO)
        .file(
            format!("{REPO}/.deepseek/hooks.json"),
            "[{\"event\": \"PreToolUse\", \"matcher\": \"write_file\", \"command\": \"./guard.sh\"}]",
        )
        .file(
            format!("{U}/hooks.json"),
            "[{\"event\": \"PostToolUse\", \"command\": \"./audit.sh\"}, {\"event\": \"PreCompact\", \"command\": \"./save.sh\"}]",
        );
    let h = run(&fs);
    let guard = item(&h, "deepseek:hook:project:PreToolUse-1");
    match &guard.detail {
        ItemDetail::Hook(a) => {
            assert_eq!(a.event, "before-tool");
            assert_eq!(a.native_event, "PreToolUse");
            assert_eq!(a.matcher.as_deref(), Some("write_file"));
            assert_eq!(a.script.as_deref(), Some("guard.sh"));
        }
        other => panic!("not a hook: {other:?}"),
    }
    item(&h, "deepseek:hook:user:PostToolUse-1");
    let compact = item(&h, "deepseek:hook:user:PreCompact-1");
    match &compact.detail {
        ItemDetail::Hook(a) => assert_eq!(a.event, "before-compact"),
        other => panic!("not a hook: {other:?}"),
    }
}

#[test]
fn mcp_servers_read_both_files_with_the_targets_redacted() {
    let fs = MemFs::new().dir(REPO)
        .file(
            format!("{U}/mcp.json"),
            "{\"mcpServers\": {\"fs\": {\"command\": \"npx\", \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/abs\"], \"env\": {\"TOKEN\": \"secret\"}}}}",
        )
        .file(
            format!("{REPO}/.mcp.json"),
            "{\"mcpServers\": {\"docs\": {\"url\": \"https://docs.example.com/mcp\"}}}",
        );
    let h = run(&fs);
    let fserver = item(&h, "deepseek:mcp:user:fs");
    match &fserver.detail {
        ItemDetail::Mcp(m) => {
            assert_eq!(m.transport, McpTransport::Stdio);
            assert_eq!(m.target.as_deref(), Some("npx"), "the file name only");
        }
        other => panic!("not mcp: {other:?}"),
    }
    let docs = item(&h, "deepseek:mcp:project:docs");
    match &docs.detail {
        ItemDetail::Mcp(m) => assert_eq!(m.target.as_deref(), Some("https://docs.example.com")),
        other => panic!("not mcp: {other:?}"),
    }
    let all = serde_json::to_string(&h).unwrap_or_default();
    assert!(!all.contains("secret"), "no env value ever prints");
    assert!(!all.contains("/abs"), "no args ever print");
}
