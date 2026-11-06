//! The Cursor adapter on invented setups, one rule per test (docs/adapters/cursor.md). Every output keeps the
//! contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Cursor;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const U: &str = "/home/dev/.cursor";
const P: &str = "/home/dev/work/shop/app/.cursor";

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
    let h = Cursor.resolve(&ctx(env, fs));
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
    Cursor.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new().dir(REPO)), 0);
    let home = MemFs::new().dir(REPO).dir(U);
    assert_eq!(detect(&home), 45, "an installed Cursor alone is home-folder evidence");
    let rules = MemFs::new().dir(REPO).file(format!("{REPO}/.cursorrules"), "old rules");
    assert_eq!(detect(&rules), 60, "the legacy file is still repo evidence");
    let repo = MemFs::new().dir(REPO).file(format!("{P}/mcp.json"), "{}");
    assert_eq!(detect(&repo), 100, "the repo's own .cursor/ is repo evidence");
}

#[test]
fn config_layers_follow_the_documented_rank() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/mcp.json"), "{}")
        .file(format!("{U}/hooks.json"), "{}")
        .file(format!("{REPO}/.claude/settings.json"), "{}");
    let h = run(&fs);
    for id in ["user", "project", "imports_project", "imports_user"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
}

#[test]
fn rules_follow_their_frontmatter_through_the_four_types() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/rules/always.mdc"), "---\ndescription:\nglobs:\nalwaysApply: true\n---\nAlways applied.")
        .file(format!("{P}/rules/glob.mdc"), "---\nglobs: src/**/*.tsx\nalwaysApply: false\n---\nReact rules.")
        .file(
            format!("{P}/rules/smart.mdc"),
            "---\ndescription: Backend conventions for services.\n---\nService rules.",
        )
        .file(format!("{P}/rules/manual.mdc"), "---\nalwaysApply: false\n---\nOnly on @-mention.")
        .file(format!("{P}/rules/plain.md"), "Ignored: the wrong extension.");
    let h = run(&fs);
    let always = item(&h, "cursor:instruction:project:.cursor/rules/always.mdc");
    assert_eq!(always.load.when, LoadWhen::Startup);
    let globed = item(&h, "cursor:instruction:project:.cursor/rules/glob.mdc");
    assert_eq!(globed.load.when, LoadWhen::OnMatch);
    match &globed.detail {
        ItemDetail::Instruction(a) => assert_eq!(a.applies_to, vec!["src/**/*.tsx".to_string()]),
        other => panic!("not an instruction: {other:?}"),
    }
    let smart = item(&h, "cursor:instruction:project:.cursor/rules/smart.mdc");
    assert_eq!(smart.load.when, LoadWhen::OnInvoke, "the agent decides from the description");
    item(&h, "cursor:instruction:project:.cursor/rules/manual.mdc");
    assert!(ids(&h, "instruction").len() == 4, "the plain .md file is ignored; have {:#?}", ids(&h, "instruction"));
}

#[test]
fn nested_agents_md_is_lazy_and_the_root_is_every_session() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/AGENTS.md"), "Root instructions.")
        .file(format!("{REPO}/frontend/AGENTS.md"), "Frontend instructions.");
    let h = run(&fs);
    assert_eq!(item(&h, "cursor:instruction:project:AGENTS.md").load.when, LoadWhen::Startup);
    let nested = item(&h, "cursor:instruction:project:frontend/AGENTS.md");
    assert_eq!(nested.load.when, LoadWhen::OnInvoke, "loads when the agent works in that directory");
}

#[test]
fn the_legacy_cursorrules_file_warns_and_loads() {
    let fs = MemFs::new().dir(REPO).file(format!("{REPO}/.cursorrules"), "Old-style rules.");
    let h = run(&fs);
    assert!(h.warnings.iter().any(|w| w.code == "cursorrules-legacy"), "the deprecation is said aloud");
    item(&h, "cursor:instruction:project:.cursorrules");
}

#[test]
fn skills_come_from_every_family_and_the_nested_folder_is_not_part_of_the_name() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/skills/review/SKILL.md"), "---\nname: review\ndescription: Review code\n---\nSteps here.")
        .file(format!("{P}/skills/shipping/deploy/SKILL.md"), "---\ndescription: Ship it\n---\nDeploy steps.")
        .file(format!("{HOME}/.cursor/skills/audit/SKILL.md"), "Audit the repo.")
        .file(format!("{REPO}/.agents/skills/plan/SKILL.md"), "Plan the work.");
    let h = run(&fs);
    item(&h, "cursor:skill:project:review");
    let deploy = item(&h, "cursor:skill:project:deploy");
    assert_eq!(deploy.origin.layer, "project", "the category folder is not part of the name");
    item(&h, "cursor:skill:user:audit");
    item(&h, "cursor:skill:imports_project:plan");
}

#[test]
fn legacy_commands_load_as_skills_by_their_file_name() {
    let fs = MemFs::new().dir(REPO).file(format!("{P}/commands/deploy.md"), "Deploy the app now.");
    let h = run(&fs);
    let c = item(&h, "cursor:skill:project:deploy.md");
    match &c.detail {
        ItemDetail::Skill(s) => assert!(s.body_chars.is_some(), "the prompt body is measured for the invoke"),
        other => panic!("not a skill: {other:?}"),
    }
}

#[test]
fn subagents_come_from_cursor_and_the_compat_folders_and_inherit_by_default() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/agents/verifier.md"),
            "---\nname: verifier\ndescription: Checks work\nmodel: inherit\nreadonly: true\n---\nVerify everything.",
        )
        .file(
            format!("{P}/agents/scout.md"),
            "---\nname: scout\ndescription: Looks around\nmodel: gpt-5.6-sol\n---\nLook around.",
        )
        .file(format!("{HOME}/.claude/agents/helper.md"), "A Claude agent.");
    let h = run(&fs);
    item(&h, "cursor:agent:project:verifier");
    let scout = item(&h, "cursor:agent:project:scout");
    match &scout.detail {
        ItemDetail::Agent(a) => assert_eq!(a.model.as_deref(), Some("gpt-5.6-sol")),
        other => panic!("not an agent: {other:?}"),
    }
    let verifier = item(&h, "cursor:agent:project:verifier");
    match &verifier.detail {
        ItemDetail::Agent(a) => {
            assert_eq!(a.model, None, "inherit means no own model");
            assert_eq!(a.tools, ToolAccess::All, "Cursor subagents take the parent's tools");
        }
        other => panic!("not an agent: {other:?}"),
    }
    let helper = item(&h, "cursor:agent:imports_user:helper");
    assert!(helper.effective);
}

#[test]
fn hooks_read_the_cursor_shape_and_the_claude_settings() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/hooks.json"),
            "{\"version\": 1, \"hooks\": {\"sessionStart\": [{\"command\": \"./init.sh\"}], \"preToolUse\": [{\"matcher\": \"Shell\", \"command\": \"./guard.sh\"}]}}",
        )
        .file(
            format!("{U}/hooks.json"),
            "{\"hooks\": {\"stop\": [{\"command\": \"./done.sh\"}]}}",
        )
        .file(
            format!("{REPO}/.claude/settings.json"),
            "{\"hooks\": {\"PreToolUse\": [{\"matcher\": \"Write\", \"hooks\": [{\"type\": \"command\", \"command\": \"./claude-guard.sh\"}]}]}}",
        );
    let h = run(&fs);
    let start = item(&h, "cursor:hook:project:sessionStart-1");
    match &start.detail {
        ItemDetail::Hook(a) => {
            assert_eq!(a.event, "session-start", "Cursor's camelCase maps to the neutral name");
            assert_eq!(a.native_event, "sessionStart");
            assert_eq!(a.script.as_deref(), Some("init.sh"), "only the file name survives");
        }
        other => panic!("not a hook: {other:?}"),
    }
    let guard = item(&h, "cursor:hook:project:preToolUse-1");
    match &guard.detail {
        ItemDetail::Hook(a) => assert_eq!(a.event, "before-tool", "the same stage as Claude's PreToolUse"),
        other => panic!("not a hook: {other:?}"),
    }
    item(&h, "cursor:hook:user:stop-1");
    let imported = item(&h, "cursor:hook:imports_project:PreToolUse-1");
    match &imported.detail {
        ItemDetail::Hook(a) => assert_eq!(a.matcher.as_deref(), Some("Write")),
        other => panic!("not a hook: {other:?}"),
    }
}

#[test]
fn mcp_servers_from_both_files_redact_everything_but_the_target() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/mcp.json"),
            "{\"mcpServers\": {\"http-one\": {\"url\": \"https://api.example.com/mcp\", \"headers\": {\"Authorization\": \"Bearer ${env:TOKEN}\"}}, \"local-one\": {\"command\": \"npx\", \"args\": [\"${workspaceFolder}/mcp.js\"], \"env\": {\"KEY\": \"secret\"}}}}",
        )
        .file(format!("{U}/mcp.json"), "{\"mcpServers\": {\"user-one\": {\"command\": \"python\", \"args\": [\"-m\", \"mcp\"]}}}");
    let h = run(&fs);
    let http = item(&h, "cursor:mcp:project:http-one");
    match &http.detail {
        ItemDetail::Mcp(m) => {
            assert_eq!(m.transport, McpTransport::Http);
            assert_eq!(m.target.as_deref(), Some("https://api.example.com"), "origin only");
        }
        other => panic!("not mcp: {other:?}"),
    }
    let local = item(&h, "cursor:mcp:project:local-one");
    match &local.detail {
        ItemDetail::Mcp(m) => {
            assert_eq!(m.transport, McpTransport::Stdio);
            assert_eq!(m.target.as_deref(), Some("npx"), "the file name only");
        }
        other => panic!("not mcp: {other:?}"),
    }
    item(&h, "cursor:mcp:user:user-one");
    let all = serde_json::to_string(&h).unwrap_or_default();
    assert!(!all.contains("secret"), "no env value ever prints");
    assert!(!all.contains("Bearer"), "no header value ever prints");
}

fn ids(h: &Harness, kind: &str) -> Vec<String> {
    h.items.iter().filter(|i| i.detail.kind() == kind).map(|i| i.id.clone()).collect()
}
