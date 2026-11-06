//! The Gemini CLI adapter on invented setups, one rule per test (docs/adapters/gemini.md). Every output keeps
//! the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Gemini;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const U: &str = "/home/dev/.gemini";
const P: &str = "/home/dev/work/shop/app/.gemini";
const SYS: &str = "/etc/gemini-cli/settings.json";

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
    let h = Gemini.resolve(&ctx(env, fs));
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
    Gemini.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new().dir(REPO)), 0);
    let home = MemFs::new().dir(REPO).dir(U);
    assert_eq!(detect(&home), 45, "an installed CLI alone is home-folder evidence");
    let root_md = MemFs::new().dir(REPO).file(format!("{REPO}/GEMINI.md"), "Root context.");
    assert_eq!(detect(&root_md), 60);
    let repo = MemFs::new().dir(REPO).file(format!("{P}/settings.json"), "{}");
    assert_eq!(detect(&repo), 100, "the repo's own .gemini/ is repo evidence");
}

#[test]
fn config_layers_follow_the_documented_rank() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(SYS, "{}")
        .file(format!("{P}/settings.json"), "{}")
        .file(format!("{U}/settings.json"), "{}")
        .file("/etc/gemini-cli/system-defaults.json", "{}")
        .dir(format!("{U}/extensions/demo"));
    let h = run(&fs);
    for id in ["system", "project", "user", "system_defaults", "extensions"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
}

#[test]
fn settings_carry_the_trail_and_the_system_file_overrides() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(SYS, r#"{"model": {"name": "team-model"}}"#)
        .file(format!("{P}/settings.json"), r#"{"model": {"name": "project-model"}}"#)
        .file(format!("{U}/settings.json"), r#"{"model": {"name": "user-model"}}"#);
    let h = run(&fs);
    assert_eq!(h.settings["model"].value, "team-model", "the system settings file overrides all");
    let trail: Vec<&str> = h.settings["model"].trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(trail, ["system", "project", "user"], "every layer that had a say, highest first");

    let fs2 = MemFs::new().dir(REPO).file(format!("{U}/settings.json"), r#"{"tools": {"sandbox": "docker"}}"#);
    let h2 = run(&fs2);
    assert_eq!(h2.settings["sandbox"].value, "docker");
    assert_eq!(h2.settings["sandbox"].source.as_deref(), Some("user"));

    let fs3 = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/settings.json"), r#"{"context": {"fileName": ["AGENTS.md", "GEMINI.md"]}}"#);
    let h3 = run(&fs3);
    assert_eq!(
        h3.settings["contextFileName"].value,
        Value::Array(vec!["AGENTS.md".into(), "GEMINI.md".into()]),
        "the array survives as the decision's value"
    );
}

#[test]
fn context_files_load_hierarchically_and_subdirectories_are_just_in_time() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/GEMINI.md"), "Global instructions.")
        .file(format!("{REPO}/GEMINI.md"), "Project instructions.")
        .file(format!("{REPO}/packages/api/GEMINI.md"), "API instructions.");
    let h = run(&fs);
    assert_eq!(item(&h, "gemini:instruction:user:~/.gemini/GEMINI.md").load.when, LoadWhen::Startup);
    assert_eq!(item(&h, "gemini:instruction:project:GEMINI.md").load.when, LoadWhen::Startup);
    let jit = item(&h, "gemini:instruction:project:packages/api/GEMINI.md");
    assert_eq!(jit.load.when, LoadWhen::OnInvoke, "discovered just in time, out of the startup total");
}

#[test]
fn the_context_file_name_setting_is_honored() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/settings.json"), r#"{"context": {"fileName": ["AGENTS.md"]}}"#)
        .file(format!("{REPO}/AGENTS.md"), "Instructions by another name.")
        .file(format!("{REPO}/GEMINI.md"), "Not configured, so not read.");
    let h = run(&fs);
    assert!(h.items.iter().any(|i| i.name == "AGENTS.md" && i.effective));
    assert!(!h.items.iter().any(|i| i.name == "GEMINI.md"), "only the configured names are read");
}

#[test]
fn gemini_at_imports_follow_their_trail_into_the_repo_and_the_user_folder() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/GEMINI.md"), "Main.\n@./docs/style.md\n@../shared/missing.md")
        .file(format!("{REPO}/docs/style.md"), "Style rules.\n@./deeper.md")
        .file(format!("{REPO}/docs/deeper.md"), "Deeper rules.");
    let h = run(&fs);
    let root = item(&h, "gemini:instruction:project:GEMINI.md");
    match &root.detail {
        ItemDetail::Instruction(a) => assert!(a.imported_by.is_none(), "the root file imports, it is not imported"),
        other => panic!("not an instruction: {other:?}"),
    }
    let style = item(&h, "gemini:instruction:project:docs/style.md");
    match &style.detail {
        ItemDetail::Instruction(a) => {
            assert_eq!(a.imported_by.as_deref(), Some("gemini:instruction:project:GEMINI.md"))
        }
        other => panic!("not an instruction: {other:?}"),
    }
    item(&h, "gemini:instruction:project:docs/deeper.md");
    // A missing import is not an item and not a crash.
    assert!(!h.items.iter().any(|i| i.name.contains("missing")));
}

#[test]
fn an_import_outside_the_repo_and_user_folder_does_not_follow() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/GEMINI.md"), "Main.\n@/etc/passwd")
        .file("/etc/passwd", "existing file outside the allowed directories");
    let h = run(&fs);
    assert!(h.warnings.iter().any(|w| w.code == "import-outside"), "the trail stops at the repo and the user folder");
    assert!(h.items.iter().filter(|i| i.detail.kind() == "instruction").count() == 1);
}

#[test]
fn commands_are_toml_skills_namespaced_by_their_directories() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/commands/git/commit.toml"),
            "description = \"Commit helper\"\nprompt = \"\"\"Write a commit message.\"\"\"",
        )
        .file(format!("{U}/commands/deploy.toml"), "description = \"Ship it\"\nprompt = \"Deploy the app.\"");
    let h = run(&fs);
    let c = item(&h, "gemini:skill:project:git:commit");
    match &c.detail {
        ItemDetail::Skill(s) => assert!(s.body_chars.unwrap() > 0, "the prompt is the invoked body"),
        other => panic!("not a skill: {other:?}"),
    }
    item(&h, "gemini:skill:user:deploy");
}

#[test]
fn subagents_carry_their_tools_model_and_the_extension_ones() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/agents/security-auditor.md"),
            "---\nname: security-auditor\ndescription: Finds vulnerabilities\ntools:\n  - read_file\n  - grep_search\nmodel: gemini-3-flash-preview\n---\nYou are a security auditor.",
        )
        .dir(format!("{U}/extensions/demo"))
        .file(format!("{U}/extensions/demo/gemini-extension.json"), "{\"name\": \"demo\"}")
        .file(format!("{U}/extensions/demo/agents/reviewer.md"), "Review the code.");
    let h = run(&fs);
    let a = item(&h, "gemini:agent:project:security-auditor");
    match &a.detail {
        ItemDetail::Agent(x) => {
            assert_eq!(x.tools, ToolAccess::Only(vec!["read_file".into(), "grep_search".into()]));
            assert_eq!(x.model.as_deref(), Some("gemini-3-flash-preview"));
        }
        other => panic!("not an agent: {other:?}"),
    }
    item(&h, "gemini:agent:extensions:reviewer");
}

#[test]
fn hooks_run_from_every_settings_layer_and_extensions_add_too() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/settings.json"),
            r#"{"hooks": {"BeforeTool": [{"matcher": "write_file", "hooks": [{"type": "command", "command": "@./guard.sh"}]}]}}"#,
        )
        .file(
            format!("{U}/settings.json"),
            r#"{"hooks": {"PreCompress": [{"hooks": [{"type": "command", "command": "./save.sh"}]}]}}"#,
        )
        .dir(format!("{U}/extensions/demo"))
        .file(format!("{U}/extensions/demo/gemini-extension.json"), "{\"name\": \"demo\"}")
        .file(format!("{U}/extensions/demo/hooks/hooks.json"), r#"{"SessionStart": [{"hooks": [{"type": "command", "command": "./ext-start.sh"}]}]}"#);
    let h = run(&fs);
    let guard = item(&h, "gemini:hook:project:BeforeTool-1");
    match &guard.detail {
        ItemDetail::Hook(a) => {
            assert_eq!(a.event, "before-tool", "the neutral name matches Claude's PreToolUse");
            assert_eq!(a.matcher.as_deref(), Some("write_file"));
        }
        other => panic!("not a hook: {other:?}"),
    }
    let compact = item(&h, "gemini:hook:user:PreCompress-1");
    match &compact.detail {
        ItemDetail::Hook(a) => assert_eq!(a.event, "before-compact", "Gemini's PreCompress is the same stage"),
        other => panic!("not a hook: {other:?}"),
    }
    // Additive: the project and the user hooks coexist, and the extension's join.
    item(&h, "gemini:hook:extensions:SessionStart-1");
}

#[test]
fn mcp_servers_come_from_every_layer_and_the_settings_beat_the_extensions() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/settings.json"),
            r#"{"mcpServers": {"docs": {"url": "https://docs.example.com/mcp"}}}"#,
        )
        .file(
            format!("{U}/settings.json"),
            r#"{"mcpServers": {"docs": {"command": "docs-mcp", "args": ["--stdio"]}, "local-dev": {"command": "npx", "args": ["dev-mcp"]}}}"#,
        )
        .dir(format!("{U}/extensions/demo"))
        .file(
            format!("{U}/extensions/demo/gemini-extension.json"),
            "{\"name\": \"demo\", \"mcpServers\": {\"docs\": {\"command\": \"extension-docs\"}}}",
        );
    let h = run(&fs);
    let docs = item(&h, "gemini:mcp:project:docs");
    assert!(docs.effective, "the project's settings file is the one in effect");
    let user_docs = h.items.iter().find(|i| i.id.starts_with("gemini:mcp:user:docs")).expect("the user twin is listed");
    assert!(!user_docs.effective, "the same name at the user layer loses to the project layer");
    let ext_docs =
        h.items.iter().find(|i| i.id.starts_with("gemini:mcp:extensions:docs")).expect("the extension twin is listed");
    assert!(!ext_docs.effective, "settings files win the same name over extensions");
    match &docs.detail {
        ItemDetail::Mcp(m) => {
            assert_eq!(m.transport, McpTransport::Http);
            assert_eq!(m.target.as_deref(), Some("https://docs.example.com"), "origin only");
        }
        other => panic!("not mcp: {other:?}"),
    }
    item(&h, "gemini:mcp:user:local-dev");
    let all = serde_json::to_string(&h).unwrap_or_default();
    assert!(!all.contains("--stdio"), "no args ever print");
}

#[test]
fn policies_become_permissions_at_the_user_tier_and_in_extensions() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{U}/policies/shell.toml"),
            "[[rule]]\ntoolName = \"run_shell_command\"\ncommandPrefix = \"git\"\ndecision = \"ask_user\"\npriority = 100\n\n[[rule]]\ntoolName = \"write_file\"\ndecision = \"deny\"\n[policy]\nnothing = true",
        )
        .dir(format!("{U}/extensions/demo"))
        .file(format!("{U}/extensions/demo/gemini-extension.json"), "{\"name\": \"demo\"}")
        .file(format!("{U}/extensions/demo/policies/lockdown.toml"), "[[rule]]\ntoolName = \"write_data\"\ndecision = \"deny\"");
    let h = run(&fs);
    let git = item(&h, "gemini:permission:user:policy:run_shell_command");
    match &git.detail {
        ItemDetail::Permission(p) => {
            assert_eq!(p.effect, PermissionEffect::Ask);
            assert_eq!(p.pattern.as_deref(), Some("git"), "the command prefix is the pattern");
        }
        other => panic!("not a permission: {other:?}"),
    }
    let deny = item(&h, "gemini:permission:user:policy:write_file");
    match &deny.detail {
        ItemDetail::Permission(p) => assert_eq!(p.effect, PermissionEffect::Deny),
        other => panic!("not a permission: {other:?}"),
    }
    item(&h, "gemini:permission:extensions:policy:write_data");
}

#[test]
fn the_system_paths_move_with_the_documented_env_vars() {
    let fs = MemFs::new().dir(REPO).file("/opt/company/gemini/settings.json", r#"{"model": {"name": "corp-model"}}"#);
    let h = run_env(&[("GEMINI_CLI_SYSTEM_SETTINGS_PATH", "/opt/company/gemini/settings.json")], &fs);
    assert_eq!(h.settings["model"].value, "corp-model");
    assert_eq!(h.settings["model"].source.as_deref(), Some("system"));
}
