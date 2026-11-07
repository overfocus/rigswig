//! The Codex adapter on invented setups, one rule per test (docs/adapters/codex.md). Every output keeps the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Codex;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const ROOT: &str = "/home/dev/work/shop";
const REPO: &str = "/home/dev/work/shop/app";
const CX: &str = "/home/dev/.codex";

fn run(fs: &MemFs) -> Harness {
    let ctx = Ctx { fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
    let h = Codex.resolve(&ctx);
    assert_eq!(contract_errors(&h), Vec::<String>::new(), "contract");
    h
}

/// A git project with `app/` as the working folder, trusted unless a test says otherwise.
fn base(trust: Option<&str>) -> MemFs {
    let projects = trust.map(|t| format!("[projects.\"{ROOT}\"]\ntrust_level = \"{t}\"\n")).unwrap_or_default();
    MemFs::new().dir(format!("{ROOT}/.git")).dir(REPO).file(format!("{CX}/config.toml"), projects)
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

fn with_user_config(trust: &str, extra: &str) -> MemFs {
    base(None).file(format!("{CX}/config.toml"), format!("{extra}\n[projects.\"{ROOT}\"]\ntrust_level = \"{trust}\"\n"))
}

#[test]
fn settings_follow_layer_precedence_and_project_limits() {
    let fs = with_user_config(
        "trusted",
        "model = \"user-model\"\nmodel_provider = \"local-llm\"\napproval_policy = \"never\"\nmodel_context_window = 64000",
    )
    .file("/etc/codex/config.toml", "model = \"system-model\"\nsandbox_mode = \"read-only\"")
    .file(format!("{ROOT}/.codex/config.toml"), "model = \"root-model\"\nmodel_provider = \"ignored\"")
    .file(format!("{REPO}/.codex/config.toml"), "model = \"closest-model\"")
    .file("/etc/codex/requirements.toml", "allowed_approval_policies = [\"on-request\"]");
    let h = run(&fs);
    assert_eq!(h.settings["model"].value, "closest-model", "the closest project config wins");
    assert_eq!(h.settings["model"].source.as_deref(), Some("project"));
    let trail: Vec<&str> = h.settings["model"].trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(trail, ["project", "user", "system"]);
    assert_eq!(h.settings["provider"].value, "local-llm", "model_provider is ignored in project config");
    assert!(h.warnings.iter().any(|w| w.code == "project-key-ignored" && w.message.starts_with("model_provider")));
    assert_eq!(h.settings["sandbox_mode"].source.as_deref(), Some("system"));
    assert_eq!(h.settings["model_context_window"].value, 64000, "a stated window is reported for the page to use");
    let ap = &h.settings["approval_policy"];
    assert_eq!(ap.trail.last().unwrap().layer, "requirements");
    assert!(h.warnings.iter().any(|w| w.code == "not-allowed"));
    assert_eq!(h.settings["projectTrust"].value, "trusted");
}

#[test]
fn an_untrusted_or_unknown_project_skips_its_config() {
    let project = |fs: MemFs| {
        fs.file(format!("{REPO}/.codex/config.toml"), "model = \"project-model\"")
            .file(format!("{REPO}/AGENTS.md"), "docs")
    };
    let unknown = run(&project(base(None)));
    assert_eq!(unknown.settings.get("model"), None);
    assert_eq!(unknown.settings["projectTrust"].value, "unknown");
    assert!(unknown.warnings.iter().any(|w| w.code == "project-untrusted"));
    assert_eq!(
        ids(&unknown, "instruction"),
        ["codex:instruction:project:AGENTS.md"],
        "AGENTS.md is skipped only when untrusted"
    );

    let untrusted = run(&project(base(Some("untrusted"))));
    assert!(ids(&untrusted, "instruction").is_empty());
    assert!(untrusted.warnings.iter().any(|w| w.code == "project-untrusted-docs"));
}

#[test]
fn agents_md_one_per_folder_root_first_with_a_shared_budget() {
    let fs = base(Some("trusted"))
        .file(format!("{CX}/AGENTS.override.md"), "   ")
        .file(format!("{CX}/AGENTS.md"), "  global  ")
        .file(format!("{ROOT}/AGENTS.md"), "root docs")
        .file(format!("{ROOT}/AGENTS.override.md"), "root override")
        .file(format!("{REPO}/TEAM.md"), "fallback docs");
    let h = run(&fs.clone().file(
        format!("{CX}/config.toml"),
        format!("project_doc_fallback_filenames = [\"TEAM.md\"]\n[projects.\"{ROOT}\"]\ntrust_level = \"trusted\"\n"),
    ));
    let names: Vec<&str> =
        h.items.iter().filter(|i| i.detail.kind() == "instruction").map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["~/.codex/AGENTS.md", "~/work/shop/AGENTS.override.md", "TEAM.md"]);
    assert_eq!(item(&h, "codex:instruction:user:~/.codex/AGENTS.md").load.chars.value, 6, "the global file is trimmed");

    // A 10-byte budget: the root file (13 bytes) is cut to 10, the next is dropped.
    let tight = fs.file(
        format!("{CX}/config.toml"),
        format!("project_doc_max_bytes = 10\nproject_doc_fallback_filenames = [\"TEAM.md\"]\n[projects.\"{ROOT}\"]\ntrust_level = \"trusted\"\n"),
    );
    let h = run(&tight);
    assert_eq!(item(&h, "codex:instruction:project:~/work/shop/AGENTS.override.md").load.chars.value, 10);
    let dropped = item(&h, "codex:instruction:project:TEAM.md");
    assert!(!dropped.effective);
    assert_eq!(h.warnings.iter().filter(|w| w.code == "agents-md-budget").count(), 2);
}

#[test]
fn skills_from_every_place_same_names_both_listed_and_config_disables() {
    let skill = |d: &str| format!("---\ndescription: {d}\n---\nbody");
    let fs = base(Some("trusted"))
        .file(format!("{REPO}/.agents/skills/deploy/SKILL.md"), skill("app deploy"))
        .file(format!("{ROOT}/.agents/skills/deploy/SKILL.md"), skill("root deploy"))
        .file(format!("{HOME}/.agents/skills/notes/SKILL.md"), skill("notes"))
        .file(format!("{CX}/skills/old/SKILL.md"), skill("old place"))
        .file("/etc/codex/skills/policy/SKILL.md", skill("admin"))
        .file(format!("{CX}/config.toml"), format!("[[skills.config]]\nname = \"notes\"\nenabled = false\n[projects.\"{ROOT}\"]\ntrust_level = \"trusted\"\n"));
    let h = run(&fs);
    let deploys: Vec<&Item> = h.items.iter().filter(|i| i.name == "deploy").collect();
    assert_eq!(deploys.len(), 2);
    assert!(deploys.iter().all(|i| i.effective), "Codex lists both same-named skills");
    assert!(!item(&h, "codex:skill:user:notes").effective);
    assert!(item(&h, "codex:skill:system:policy").effective);
    assert!(h.warnings.iter().any(|w| w.code == "deprecated-location"));
}

#[test]
fn prompts_load_on_invoke_and_agents_are_toml() {
    let fs = base(Some("trusted"))
        .file(format!("{CX}/prompts/review.md"), "---\ndescription: review the diff\n---\nDo it.")
        .file(
            format!("{CX}/agents/helper.toml"),
            "name = \"helper\"\ndescription = \"user helper\"\ndeveloper_instructions = \"x\"",
        )
        .file(
            format!("{REPO}/.codex/agents/h.toml"),
            "name = \"helper\"\ndescription = \"project helper\"\nmodel = \"small\"\ndeveloper_instructions = \"x\"",
        );
    let h = run(&fs);
    let p = item(&h, "codex:command:user:prompts:review");
    assert_eq!(p.load.when, LoadWhen::OnInvoke);
    assert_eq!(item(&h, "codex:agent:user:helper").shadowed_by.as_deref(), Some("codex:agent:project:helper"));
    assert!(
        matches!(&item(&h, "codex:agent:project:helper").detail, ItemDetail::Agent(a) if a.model.as_deref() == Some("small"))
    );
    assert!(!h.warnings.iter().any(|w| w.code == "agent-all-tools"));

    let untrusted =
        fs.file(format!("{CX}/config.toml"), format!("[projects.\"{ROOT}\"]\ntrust_level = \"untrusted\"\n"));
    assert!(item(&run(&untrusted), "codex:agent:user:helper").effective, "project agents need a trusted project");
}

#[test]
fn hooks_from_json_and_config_all_load_unless_the_feature_is_off() {
    let json = r#"{"hooks":{"PreToolUse":[{"matcher":"^Bash$","hooks":[{"type":"command","command":"python3 ~/.codex/hooks/guard.py --token SECRET-CANARY"}]}]}}"#;
    let inline = "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"notify.sh\"\n";
    let fs = base(None).file(format!("{CX}/hooks.json"), json).file(format!("{CX}/config.toml"), inline);
    let h = run(&fs);
    let guard = item(&h, "codex:hook:user:PreToolUse-1");
    assert!(
        matches!(&guard.detail, ItemDetail::Hook(x) if x.script.as_deref() == Some("guard.py") && x.event == "before-tool")
    );
    assert!(item(&h, "codex:hook:user:config.toml:Stop-1").effective);
    assert!(!serde_json::to_string(&h).unwrap().contains("SECRET-CANARY"));

    let off = fs.file(format!("{CX}/config.toml"), format!("{inline}[features]\nhooks = false\n"));
    let h = run(&off);
    assert!(h.items.iter().filter(|i| i.detail.kind() == "hook").all(|i| !i.effective));
}

#[test]
fn mcp_layers_enabled_flag_and_requirements_allowlist() {
    let user = format!(
        "[mcp_servers.docs]\ncommand = \"npx\"\nargs = [\"-y\", \"ARG-CANARY\"]\nenv = {{ KEY = \"ENV-CANARY\" }}\n\
         [mcp_servers.tickets]\nurl = \"https://t.example.com/PATH-CANARY\"\nbearer_token_env_var = \"TOKEN_VAR\"\n\
         [mcp_servers.off]\ncommand = \"x\"\nenabled = false\n\
         [projects.\"{ROOT}\"]\ntrust_level = \"trusted\"\n"
    );
    let fs = base(None)
        .file(format!("{CX}/config.toml"), user)
        .file(format!("{REPO}/.codex/config.toml"), "[mcp_servers.docs]\ncommand = \"docs-server\"\n");
    let h = run(&fs);
    assert!(item(&h, "codex:mcp:project:docs").effective);
    assert_eq!(item(&h, "codex:mcp:user:docs").shadowed_by.as_deref(), Some("codex:mcp:project:docs"));
    assert!(
        matches!(&item(&h, "codex:mcp:user:tickets").detail, ItemDetail::Mcp(m) if m.target.as_deref() == Some("https://t.example.com"))
    );
    assert!(matches!(&item(&h, "codex:mcp:user:off").detail, ItemDetail::Mcp(m) if m.state == McpState::Disabled));
    let json = serde_json::to_string(&h).unwrap();
    for c in ["ARG-CANARY", "ENV-CANARY", "PATH-CANARY", "TOKEN_VAR"] {
        assert!(!json.contains(c), "{c} leaked");
    }

    let allow =
        fs.file("/etc/codex/requirements.toml", "[mcp_servers.docs]\nidentity = { command = \"docs-server\" }\n");
    let h = run(&allow);
    assert!(matches!(&item(&h, "codex:mcp:user:tickets").detail, ItemDetail::Mcp(m) if m.state == McpState::Denied));
    assert!(item(&h, "codex:mcp:project:docs").effective);
}

#[test]
fn installed_plugins_contribute_and_can_be_switched_off() {
    let v = format!("{CX}/plugins/cache/market/kit/1.2.0");
    let fs = base(Some("trusted"))
        .file(format!("{v}/plugin.json"), r#"{"name":"kit"}"#)
        .file(format!("{v}/skills/fmt/SKILL.md"), "---\ndescription: formats\n---\n")
        .file(
            format!("{v}/hooks/hooks.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"start.sh"}]}]}}"#,
        )
        .file(format!("{v}/.mcp.json"), r#"{"mcpServers":{"kitdb":{"command":"kitdb"}}}"#)
        .dir(format!("{CX}/plugins/cache/market/kit/1.0.0"));
    let h = run(&fs);
    let pkg = h.packages.iter().find(|p| p.id == "kit@market").unwrap();
    assert!(pkg.enabled);
    let layer = "plugin:kit@market";
    for id in [
        format!("codex:skill:{layer}:kit:fmt"),
        format!("codex:hook:{layer}:SessionStart-1"),
        format!("codex:mcp:{layer}:kitdb"),
    ] {
        assert!(pkg.contributes.contains(&id), "{id}");
    }
    assert!(
        item(&h, &format!("codex:skill:{layer}:kit:fmt")).origin.file.as_deref().unwrap().contains("1.2.0"),
        "the newest version folder"
    );

    let off = fs.clone().file(
        format!("{CX}/config.toml"),
        format!("[plugins.\"kit@market\"]\nenabled = false\n[projects.\"{ROOT}\"]\ntrust_level = \"trusted\"\n"),
    );
    let h = run(&off);
    let pkg = h.packages.iter().find(|p| p.id == "kit@market").unwrap();
    assert!(!pkg.enabled);
    assert_eq!(pkg.decided_by.as_deref(), Some("user"));
    assert!(h.items.iter().all(|i| !i.origin.layer.starts_with("plugin:")));

    let feature_off = fs.file(
        format!("{CX}/config.toml"),
        format!("[features]\nplugins = false\n[projects.\"{ROOT}\"]\ntrust_level = \"trusted\"\n"),
    );
    assert!(!run(&feature_off).packages[0].enabled);
}

#[test]
fn detection_prefers_codex_folders() {
    let fs = base(None).dir(format!("{REPO}/.codex"));
    let ctx =
        Ctx { fs: &fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
    let d = Codex.detect(&ctx);
    assert_eq!(d.score, 100);
    assert!(d.evidence.contains(&".codex/".to_string()));
}
