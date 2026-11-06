//! The Devin adapter on invented setups, one rule per test (docs/adapters/devin.md). Every output keeps the
//! contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Devin;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const U: &str = "/home/dev/.config/devin";
const P: &str = "/home/dev/work/shop/app/.devin";

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
    let h = Devin.resolve(&ctx(env, fs));
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

fn ids(h: &Harness, kind: &str) -> Vec<String> {
    h.items.iter().filter(|i| i.detail.kind() == kind).map(|i| i.id.clone()).collect()
}

fn detect(fs: &MemFs) -> u8 {
    Devin.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new().dir(REPO)), 0);
    let home = MemFs::new().dir(REPO).dir(U);
    assert_eq!(detect(&home), 45, "an installed CLI alone is home-folder evidence");
    let home_rules = MemFs::new().dir(REPO).dir(format!("{HOME}/.devin/rules"));
    assert_eq!(detect(&home_rules), 40);
    let repo = MemFs::new().dir(REPO).dir(P).file(format!("{P}/config.json"), "{}");
    assert_eq!(detect(&repo), 100, "the repo's own .devin/ is repo evidence");
}

#[test]
fn config_layers_follow_the_documented_rank() {
    let fs = MemFs::new()
        .dir(REPO)
        .dir(P)
        .file(format!("{U}/config.json"), "{}")
        .file(format!("{P}/config.json"), "{}")
        .file(format!("{P}/config.local.json"), "{}");
    let h = run(&fs);
    for id in ["managed", "user", "home_rules", "project", "local", "imports_project", "imports_user"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
}

#[test]
fn the_user_global_agents_md_loads_into_every_session() {
    let fs = MemFs::new().dir(REPO).file(format!("{U}/AGENTS.md"), "Global standards.\n".repeat(3));
    let h = run(&fs);
    let g = item(&h, "devin:instruction:user:~/.config/devin/AGENTS.md");
    assert_eq!(g.load.when, LoadWhen::Startup);
    assert_eq!(g.load.chars.value, 53, "\"Global standards.\\n\" x 3, with the trailing newline trimmed off");
    assert_eq!(h.context.chars.value, 53, "the same one item, summed");

    let singular = MemFs::new().dir(REPO).file(format!("{U}/AGENT.md"), "Singular rules.");
    let h2 = run(&singular);
    assert!(h2.items.iter().any(|i| i.name == "~/.config/devin/AGENT.md"));
}

#[test]
fn home_rules_load_from_the_second_user_location() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{HOME}/.devin/global_rules.md"), "Home always-on.")
        .file(format!("{HOME}/.devin/rules/deps.md"), "Dependency rules.");
    let h = run(&fs);
    item(&h, "devin:instruction:home_rules:~/.devin/global_rules.md");
    item(&h, "devin:instruction:home_rules:~/.devin/rules/deps.md");
}

#[test]
fn the_standard_project_family_loads_additively_and_claude_joins_by_import() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/AGENTS.md"), "Standard file.")
        .file(format!("{REPO}/AGENT.md"), "Singular twin.")
        .file(format!("{REPO}/CLAUDE.md"), "Claude's own file.");
    let h = run(&fs);
    // All three are read: nothing replaces anything, and each is counted once.
    assert!(h.items.iter().any(|i| i.name == "AGENTS.md" && i.effective && i.load.when == LoadWhen::Startup));
    assert!(h.items.iter().any(|i| i.name == "AGENT.md" && i.effective));
    let claude =
        h.items.iter().find(|i| i.name == "CLAUDE.md" && i.effective).expect("CLAUDE.md imported, on by default");
    assert_eq!(claude.origin.layer, "imports_project");
}

#[test]
fn turning_a_source_off_removes_what_it_would_have_imported() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{U}/config.json"),
            "{\n  // personal choice\n  \"read_config_from\": {\"agents_standard\": false, \"claude\": false}\n}",
        )
        .file(format!("{REPO}/AGENTS.md"), "Standard file.")
        .file(format!("{REPO}/CLAUDE.md"), "Claude's own file.");
    let h = run(&fs);
    assert!(ids(&h, "instruction").is_empty(), "both standard and claude gone");
    assert_eq!(h.settings["readConfigFrom.agents_standard"].value, Value::Bool(false));
    assert_eq!(h.settings["readConfigFrom.claude"].value, Value::Bool(false));
    assert_eq!(h.settings["readConfigFrom.claude"].source.as_deref(), Some("user"));
}

#[test]
fn windsurf_rules_join_and_the_global_file_defers_to_the_devin_one() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/global_rules.md"), "Devin always-on.")
        .file(format!("{REPO}/.windsurf/global_rules.md"), "Windsurf's twin.")
        .file(format!("{REPO}/.windsurf/rules/api.md"), "---\ntrigger: model_decision\n---\nAPI rules.");
    let h = run(&fs);
    assert!(h.items.iter().any(|i| i.name == ".windsurf/rules/api.md"), "the folder's rules do load");
    assert!(
        !h.items.iter().any(|i| i.name == ".windsurf/global_rules.md"),
        "the .devin file exists, so the .windsurf one is not read"
    );
    assert!(h.warnings.iter().any(|w| w.code == "windsurf-global-rules-shadowed"), "the double-up is said aloud");
    assert!(
        h.items.iter().any(|i| i.name == ".devin/global_rules.md" && i.load.when == LoadWhen::Startup),
        "the .devin one is always-on"
    );
}

#[test]
fn devin_rule_files_follow_their_trigger_and_the_distant_ones_are_lazy() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/rules/always.md"), "One rule.")
        .file(format!("{P}/rules/manual.md"), "---\ntrigger: manual\n---\nInvoked by hand.")
        .file(format!("{REPO}/docs/.devin/rules/deep.md"), "A distant rule.");
    let h = run(&fs);
    let always = item(&h, "devin:instruction:project:.devin/rules/always.md");
    assert_eq!(always.load.when, LoadWhen::Startup);
    let invoked = item(&h, "devin:instruction:project:.devin/rules/manual.md");
    assert_eq!(invoked.load.when, LoadWhen::Startup, "manual rules at the root are still always loaded");
    let deep = item(&h, "devin:instruction:project:docs/.devin/rules/deep.md");
    assert_eq!(deep.load.when, LoadWhen::OnInvoke, "below the root, rules load on access");
    assert_eq!(deep.load.chars.value, 15, "measured even when lazy");
}

#[test]
fn cursor_rules_map_their_frontmatter_to_how_they_load() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/.cursor/rules/always.mdc"), "---\nalwaysApply: true\n---\nAlways.")
        .file(format!("{REPO}/.cursor/rules/glob.md"), "---\nglobs: \"src/**/*.tsx\"\n---\nReact rules.")
        .file(format!("{REPO}/.cursor/rules/suggested.md"), "Only a description.");
    let h = run(&fs);
    let startup = h.items.iter().find(|i| i.name.ends_with("always.mdc")).expect("always.mdc");
    assert_eq!(startup.load.when, LoadWhen::Startup);
    let globed = h.items.iter().find(|i| i.name.ends_with("rules/glob.md")).expect("glob.md");
    assert_eq!(globed.load.when, LoadWhen::OnMatch);
    match &globed.detail {
        ItemDetail::Instruction(a) => assert_eq!(a.applies_to, vec!["src/**/*.tsx".to_string()]),
        other => panic!("not an instruction: {other:?}"),
    }
    let suggested = h.items.iter().find(|i| i.name.ends_with("suggested.md")).expect("suggested.md");
    assert_eq!(suggested.load.when, LoadWhen::OnInvoke, "a description alone is the agent's call");
}

#[test]
fn skills_come_from_every_source_family_and_keep_their_names_distinct() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/skills/review/SKILL.md"), "---\nname: review\ndescription: Own review\n---\nBody.")
        .file(
            format!("{REPO}/.claude/skills/review/SKILL.md"),
            "---\nname: review\ndescription: Claude's twin\n---\nBody.",
        )
        .file(format!("{REPO}/.claude/commands/deploy.md"), "---\ndescription: Ship it\n---\nDeploy steps.")
        .file(format!("{HOME}/.agents/skills/audit/SKILL.md"), "Audit the repo.");
    let h = run(&fs);
    item(&h, "devin:skill:project:review");
    item(&h, "devin:skill:imports_project:review");
    let list: Vec<&Item> = h.items.iter().filter(|i| i.detail.kind() == "skill").collect();
    assert_eq!(
        list.len(),
        4,
        "all sources load; same names just get longer ids; have {:?}",
        list.iter().map(|i| (&i.id, &i.name)).collect::<Vec<_>>()
    );
    item(&h, "devin:skill:imports_project:deploy");
    item(&h, "devin:skill:user:audit");
}

#[test]
fn agents_compete_on_the_name_and_carry_their_tools() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/agents/researcher.md"),
            "---\nname: researcher\ndescription: Studies code\nallowed-tools:\n  - read\n  - grep\nmodel: sonnet\n---\nYou research.",
        )
        .file(
            format!("{HOME}/.config/devin/agents/researcher.md"),
            "---\nname: researcher\ndescription: User's twin\n---\nYou research too.",
        );
    let h = run(&fs);
    let proj = item(&h, "devin:agent:project:researcher");
    assert!(proj.effective, "the project profile is assumed to win");
    let user =
        h.items.iter().find(|i| i.id.starts_with("devin:agent:user:") && i.name == "researcher").expect("listed");
    assert!(!user.effective, "same name at the user level is shadowed by the project one");
    assert!(proj.shadowed_by.is_none());
    match &proj.detail {
        ItemDetail::Agent(a) => {
            assert_eq!(a.tools, ToolAccess::Only(vec!["read".into(), "grep".into()]));
            assert_eq!(a.model.as_deref(), Some("sonnet"));
        }
        other => panic!("not an agent: {other:?}"),
    }
}

#[test]
fn hooks_load_from_the_json_file_the_config_key_and_the_claude_import() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{P}/hooks.v1.json"),
            "{\n  \"PreToolUse\": [{\"matcher\": \"exec\", \"hooks\": [{\"type\": \"command\", \"command\": \"./guard.sh\"}]}]\n}",
        )
        .file(
            format!("{U}/config.json"),
            "{\"hooks\": {\"SessionStart\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"./start.sh\"}]}]}}",
        )
        .file(
            format!("{REPO}/.claude/settings.json"),
            "{\"hooks\": {\"PermissionRequest\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"./ask.sh\"}]}]}}",
        );
    let h = run(&fs);
    item(&h, "devin:hook:project:PreToolUse-1");
    item(&h, "devin:hook:user:SessionStart-1");
    let imported = item(&h, "devin:hook:imports_project:PermissionRequest-1");
    match &imported.detail {
        ItemDetail::Hook(a) => assert_eq!(a.event, "permission-request", "mapped to the neutral event name"),
        other => panic!("not a hook: {other:?}"),
    }
}

#[test]
fn mcp_servers_merge_by_name_with_the_higher_layer_winning() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{P}/mcp_config.json"), "{\"mcpServers\": {\"db\": {\"command\": \"pg-mcp\"}}}")
        .file(
            format!("{P}/mcp_config.local.json"),
            "{\"mcpServers\": {\"db\": {\"command\": \"pg-mcp-personal\"}, \"docs\": {\"url\": \"https://docs.internal/api\", \"transport\": \"sse\"}}}",
        )
        .file(
            format!("{REPO}/opencode.json"),
            "{\"mcp\": {\"oc\": {\"type\": \"remote\", \"url\": \"https://oc.example.com/mcp\", \"enabled\": false}}}",
        )
        .file(format!("{HOME}/.claude.json"), "{\"mcpServers\": {\"claude-db\": {\"command\": \"claude-db-mcp\"}}}");
    let h = run(&fs);
    let db = item(&h, "devin:mcp:local:db");
    assert!(db.effective, "the local file outranks the project file");
    let shadowed =
        h.items.iter().find(|i| i.id.starts_with("devin:mcp:project:db")).expect("the project twin is listed");
    assert!(!shadowed.effective, "the same-named server loses to the local one");
    let oc = item(&h, "devin:mcp:imports_project:oc");
    match &oc.detail {
        ItemDetail::Mcp(m) => {
            assert_eq!(m.state, McpState::Disabled, "opencode's inverted enabled flag is converted");
            assert_eq!(m.transport, McpTransport::Http);
            assert_eq!(m.target.as_deref(), Some("https://oc.example.com"), "the URL keeps its origin only");
        }
        other => panic!("not mcp: {other:?}"),
    }
    let sse = item(&h, "devin:mcp:local:docs");
    match &sse.detail {
        ItemDetail::Mcp(m) => assert_eq!(m.transport, McpTransport::Sse),
        other => panic!("not mcp: {other:?}"),
    }
    let cb = item(&h, "devin:mcp:imports_user:claude-db");
    assert!(cb.effective);
}

#[test]
fn permission_rules_merge_across_layers_without_shadowing() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{U}/config.json"), "{\"permissions\": {\"allow\": [\"Read(src/**)\"]}}")
        .file(
            format!("{P}/config.json"),
            "{\"permissions\": {\"deny\": [\"Exec(sudo)\"], \"allow\": [\"Read(src/**)\"]}}",
        );
    let h = run(&fs);
    item(&h, "devin:permission:user:allow Read(src/**)");
    item(&h, "devin:permission:project:allow Read(src/**)");
    let deny = item(&h, "devin:permission:project:deny Exec(sudo)");
    match &deny.detail {
        ItemDetail::Permission(p) => {
            assert_eq!(p.effect, PermissionEffect::Deny);
            assert_eq!(p.tool, "Exec");
            assert_eq!(p.pattern.as_deref(), Some("sudo"));
        }
        other => panic!("not a permission: {other:?}"),
    }
}

#[test]
fn settings_carry_their_trail_and_the_managed_policy_is_redacted() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(
            format!("{U}/config.json"),
            "{\"agent\": {\"model\": \"opus\"}, \"sandbox\": {\"allowed_domains\": [\"**.npmjs.org\"], \"network_mode\": \"limited\"}}",
        )
        .file(format!("{P}/config.json"), "{\"read_config_from\": {\"cursor\": false}}");
    let h = run(&fs);
    assert_eq!(h.settings["model"].value, "opus");
    assert_eq!(h.settings["model"].source.as_deref(), Some("user"));
    assert_eq!(h.settings["sandbox"].source.as_deref(), Some("user"));
    assert_eq!(h.settings["readConfigFrom.cursor"].value, Value::Bool(false));
    assert_eq!(h.settings["readConfigFrom.cursor"].source.as_deref(), Some("project"));
    assert!(h.settings["readConfigFrom.cursor"].trail.len() == 1, "only the project layer said it");
}

#[test]
fn the_managed_system_policy_is_read_and_redacted() {
    let fs = MemFs::new().dir(REPO).file(
        "/etc/devin/system.json",
        "{\"enterprise_host\": \"https://user@acme.devinenterprise.com/secrets\", \"proxy\": {\"mode\": \"manual\", \"url\": \"http://proxy.corp.example.com:8080/x\"}}",
    );
    let h = run(&fs);
    assert_eq!(h.settings["policy"].value, "acme.devinenterprise.com", "scheme, credentials and path are dropped");
    let trail: Vec<&TrailStep> = h.settings["policy"].trail.iter().collect();
    let notes: Vec<&str> = trail.iter().map(|t| t.note.as_deref().unwrap_or("")).collect();
    assert_eq!(notes, ["enterprise_host", "proxy"], "one redacted step per system key, in order");
    // The proxy URL survives only as its origin, in the trail.
    let proxy = trail.iter().find(|t| t.note.as_deref() == Some("proxy")).expect("the proxy step is in the trail");
    let shown = proxy.value.to_string();
    assert!(shown.contains("proxy.corp.example.com"), "the host stays");
    assert!(!shown.contains(":8080"), "the port is dropped");
    assert!(!shown.contains("/x"), "the path is dropped");
    assert!(shown.contains("http://"), "the scheme is kept");
    assert!(ids(&h, "instruction").is_empty(), "the policy alone carries no items");
}
