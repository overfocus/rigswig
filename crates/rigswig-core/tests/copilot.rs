//! The GitHub Copilot CLI adapter on invented setups, one rule per test (docs/adapters/copilot.md). Every
//! output keeps the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Copilot;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const GH: &str = "/home/dev/work/shop/app/.github";
const U: &str = "/home/dev/.copilot";

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
    let h = Copilot.resolve(&ctx(env, fs));
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
    Copilot.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new().dir(REPO)), 0);
    let home = MemFs::new().dir(REPO).dir(U);
    assert_eq!(detect(&home), 45, "an installed CLI alone is home-folder evidence");
    let agents = MemFs::new().dir(REPO).dir(format!("{GH}/agents"));
    assert_eq!(detect(&agents), 80);
    let repo = MemFs::new().dir(REPO).file(format!("{GH}/copilot-instructions.md"), "rules");
    assert_eq!(detect(&repo), 100, "the repo's own customizations are repo evidence");
}

#[test]
fn the_copilot_home_moves_with_the_documented_variable() {
    let fs = MemFs::new().dir(REPO).file("/opt/company/copilot/copilot-instructions.md", "Company rules.");
    let h = run_env(&[("COPILOT_HOME", "/opt/company/copilot")], &fs);
    assert!(h.items.iter().any(|i| i.name == "/opt/company/copilot/copilot-instructions.md"
        && i.effective
        && i.load.when == LoadWhen::Startup));
}

#[test]
fn config_layers_follow_the_documented_rank() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/settings.json"), "{}")
        .file(format!("{GH}/copilot/settings.json"), "{}");
    let h = run(&fs);
    for id in ["user", "project", "imports_project", "imports_user"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
}

#[test]
fn settings_carry_their_trail_user_over_repository_over_local() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/settings.json"), r#"{"model": "auto"}"#)
        .file(format!("{GH}/copilot/settings.json"), r#"{"model": "gpt-5"}"#)
        // .jsonc: the settings file supports comments
        .file(format!("{GH}/copilot/settings.local.json"), "// personal\n{\n  \"effortLevel\": \"high\"\n}");
    let h = run(&fs);
    assert_eq!(h.settings["model"].value, "gpt-5", "the repository settings file overrides the user's");
    let trail: Vec<&str> = h.settings["model"].trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(trail, ["project", "user"]);
    assert_eq!(h.settings["effort"].value, "high");
    assert_eq!(h.settings["effort"].source.as_deref(), Some("local"));
}

#[test]
fn instructions_from_every_documented_chain_load_additively() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/copilot-instructions.md"), "Personal instructions.")
        .file(format!("{U}/instructions/code-style.instructions.md"), "Personal topic file.")
        .file(format!("{GH}/copilot-instructions.md"), "Repository-wide instructions.")
        .file(
            format!("{GH}/instructions/api.instructions.md"),
            "---\napplyTo:\n  - \"src/api/**\"\n  - \"**/*.ts\"\n---\nAPI rules.",
        );
    let h = run(&fs);
    item(&h, "copilot:instruction:user:~/.copilot/copilot-instructions.md");
    item(&h, "copilot:instruction:user:~/.copilot/instructions/code-style.instructions.md");
    item(&h, "copilot:instruction:project:.github/copilot-instructions.md");
    let api = item(&h, "copilot:instruction:project:.github/instructions/api.instructions.md");
    assert_eq!(api.load.when, LoadWhen::OnMatch, "applyTo scopes the file to its patterns");
    match &api.detail {
        ItemDetail::Instruction(a) => {
            assert_eq!(a.applies_to, vec!["src/api/**".to_string(), "**/*.ts".to_string()]);
        }
        other => panic!("not an instruction: {other:?}"),
    }
}

#[test]
fn the_agent_instruction_family_loads_at_the_root() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/AGENTS.md"), "Standard rules.")
        .file(format!("{REPO}/CLAUDE.md"), "Claude's own.")
        .file(format!("{REPO}/GEMINI.md"), "Gemini's own.");
    let h = run(&fs);
    item(&h, "copilot:instruction:project:AGENTS.md");
    item(&h, "copilot:instruction:project:CLAUDE.md");
    item(&h, "copilot:instruction:project:GEMINI.md");
    assert!(h.items.iter().filter(|i| i.detail.kind() == "instruction").all(|i| i.load.when == LoadWhen::Startup));
}

#[test]
fn skills_from_five_roots_and_project_beats_personal() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{GH}/skills/review/SKILL.md"), "Project review steps.")
        .file(format!("{U}/skills/review/SKILL.md"), "Personal review twin.")
        .file(format!("{REPO}/.claude/skills/claude-only/SKILL.md"), "Claude's skill.")
        .file(format!("{U}/skills/audit/SKILL.md"), "Audit steps.");
    let h = run(&fs);
    let review = item(&h, "copilot:skill:project:review");
    assert!(review.effective, "the project's skill wins the name clash");
    item(&h, "copilot:skill:imports_project:claude-only");
    item(&h, "copilot:skill:user:audit");
    let effective = h.items.iter().filter(|i| i.detail.kind() == "skill" && i.effective).count();
    assert_eq!(effective, 3, "the clashing twin is listed but not effective");
}

#[test]
fn custom_agents_keep_their_id_and_a_personal_agent_wins_the_same_id() {
    /* The docs' inverted precedence, pinned: "If a personal agent and a project-level agent have the same ID
     * ... the personal agent takes precedence." */
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{GH}/agents/security-auditor.agent.md"),
            "---\ndescription: Checks code\nmodel: gpt-5\ntools: [\"read\", \"grep\"]\n---\nYou audit.",
        )
        .file(format!("{U}/agents/security-auditor.agent.md"), "The personal twin wins.")
        .file(format!("{GH}/agents/sub/directory.agent.md"), "---\ndescription: Nested agent\n---\nHello.");
    let h = run(&fs);
    let personal = item(&h, "copilot:agent:user:security-auditor");
    assert!(personal.effective, "the personal agent outranks the project one on the same ID");
    let proj_twin =
        h.items.iter().find(|i| i.id.starts_with("copilot:agent:project:security-auditor")).expect("listed");
    assert!(!proj_twin.effective);
    match &proj_twin.detail {
        ItemDetail::Agent(a) => {
            assert_eq!(a.tools, ToolAccess::Only(vec!["read".into(), "grep".into()]));
            assert_eq!(a.model.as_deref(), Some("gpt-5"));
        }
        other => panic!("not an agent: {other:?}"),
    }
    // A nested file's ID carries its subdirectory joined by `--` (the docs' ID form).
    item(&h, "copilot:agent:project:sub--directory");
}

#[test]
fn hooks_load_from_the_json_files_the_settings_key_and_the_claude_import() {
    let fs = MemFs::new().dir(REPO)
        .file(
            format!("{GH}/hooks/lifecycle.json"),
            "{\"version\": 1, \"hooks\": {\"sessionStart\": [{\"type\": \"command\", \"bash\": \"./start.sh\"}], \"userPromptSubmitted\": [{\"type\": \"command\", \"bash\": \"./log.sh\"}]}}",
        )
        .file(format!("{U}/settings.json"), r#"{"hooks": {"postToolUse": [{ "type": "command", "bash": "./audit.sh" }]}}"#)
        .file(
            format!("{REPO}/.claude/settings.json"),
            r#"{"hooks": {"PreToolUse": [{"matcher": "Write", "hooks": [{"type": "command", "command": "./guard.sh"}]}]}}"#,
        );
    let h = run(&fs);
    let start = item(&h, "copilot:hook:project:sessionStart-1");
    match &start.detail {
        ItemDetail::Hook(a) => {
            assert_eq!(a.event, "session-start");
            assert_eq!(a.script.as_deref(), Some("start.sh"), "the bash branch's script file name only");
        }
        other => panic!("not a hook: {other:?}"),
    }
    let prompt = item(&h, "copilot:hook:project:userPromptSubmitted-1");
    match &prompt.detail {
        ItemDetail::Hook(a) => assert_eq!(a.event, "prompt-submit"),
        other => panic!("not a hook: {other:?}"),
    }
    // The settings key and the Claude import are additive with the files.
    item(&h, "copilot:hook:user:postToolUse-1");
    let imported = item(&h, "copilot:hook:imports_project:PreToolUse-1");
    match &imported.detail {
        ItemDetail::Hook(a) => assert_eq!(a.matcher.as_deref(), Some("Write")),
        other => panic!("not a hook: {other:?}"),
    }
}

#[test]
fn mcp_servers_from_three_files_and_the_workspace_overrides_the_user() {
    let fs = MemFs::new().dir(REPO)
        .file(
            format!("{U}/mcp-config.json"),
            "{\"mcpServers\": {\"docs\": {\"type\": \"http\", \"url\": \"https://docs.example.com/mcp\", \"headers\": {\"KEY\": \"x\"}}, \"local\": {\"type\": \"stdio\", \"command\": \"npx\", \"args\": [\"dev-mcp\"]}}}",
        )
        .file(format!("{REPO}/.mcp.json"), "{\"mcpServers\": {\"docs\": {\"command\": \"docs-mcp\"}}}")
        .file(format!("{GH}/mcp.json"), "{\"mcpServers\": {\"repo-only\": {\"type\": \"local\", \"command\": \"repo-mcp\"}}}");
    let h = run(&fs);
    let docs = item(&h, "copilot:mcp:project:docs");
    assert!(docs.effective, "the workspace file outranks the user's on a name conflict (pinned)");
    let user_docs =
        h.items.iter().find(|i| i.id.starts_with("copilot:mcp:user:docs")).expect("the user twin is listed");
    assert!(!user_docs.effective);
    match &docs.detail {
        ItemDetail::Mcp(m) => assert_eq!(m.transport, McpTransport::Stdio, "the user url twin stays its own item"),
        other => panic!("not mcp: {other:?}"),
    }
    let item_docs = item(&h, "copilot:mcp:project:repo-only");
    match &item_docs.detail {
        ItemDetail::Mcp(m) => assert_eq!(m.target.as_deref(), Some("repo-mcp"), "the file name only"),
        other => panic!("not mcp: {other:?}"),
    }
    let all = serde_json::to_string(&h).unwrap_or_default();
    assert!(!all.contains("dev-mcp"), "no args ever print");
    assert!(!all.contains("\": \"x\"") && !all.contains("\"KEY\""), "no header ever prints");
}
