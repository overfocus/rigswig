//! The oh-my-pi adapter on invented setups, one rule per test (docs/adapters/omp.md). Every output keeps the
//! contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Omp;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const AGENT: &str = "/home/dev/.omp/agent";

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
    let h = Omp.resolve(&ctx(env, fs));
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

fn has_warning(h: &Harness, code: &str) -> bool {
    h.warnings.iter().any(|w| w.code == code)
}

/// A repo in a git checkout.
fn repo() -> MemFs {
    MemFs::new().dir(format!("{REPO}/.git"))
}

fn skill(name: &str, desc: &str) -> String {
    format!("---\nname: {name}\ndescription: {desc}\n---\n\nDo the thing.\n")
}

fn effective_instructions(h: &Harness) -> Vec<&str> {
    h.items.iter().filter(|i| i.effective && i.detail.kind() == "instruction").map(|i| i.name.as_str()).collect()
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    let d = |fs: &MemFs| Omp.detect(&ctx(&[], fs)).score;
    assert_eq!(d(&repo()), 0);
    assert_eq!(d(&repo().dir(AGENT)), 45);
    assert_eq!(d(&repo().file(format!("{REPO}/.omp/AGENTS.md"), "x")), 100);
}

#[test]
fn a_profile_moves_the_agent_dir_and_ignores_pi_coding_agent_dir() {
    let fs = repo()
        .file(format!("{HOME}/.omp/profiles/work/agent/AGENTS.md"), "Work profile.")
        .file("/opt/agent/AGENTS.md", "Moved.");
    let h = run_env(&[("OMP_PROFILE", "work"), ("PI_CODING_AGENT_DIR", "/opt/agent")], &fs);
    assert_eq!(effective_instructions(&h), ["~/.omp/profiles/work/agent/AGENTS.md"]);
    let h = run_env(&[("PI_CODING_AGENT_DIR", "/opt/agent")], &fs);
    assert_eq!(effective_instructions(&h), ["/opt/agent/AGENTS.md"]);
}

#[test]
fn one_user_context_file_survives_and_foreign_homes_are_opt_in() {
    let fs = repo()
        .file(format!("{HOME}/.claude/CLAUDE.md"), "Claude home.")
        .file(format!("{HOME}/.agents/AGENTS.md"), "Agents home.");
    let h = run(&fs);
    assert_eq!(effective_instructions(&h), ["~/.agents/AGENTS.md"], "~/.claude is opt-in; ~/.agents is on by default");

    let opted = fs.clone().file(format!("{AGENT}/config.yml"), "enabledProviders:\n  - claude\n");
    let h = run(&opted);
    assert_eq!(
        effective_instructions(&h),
        ["~/.claude/CLAUDE.md"],
        "claude (80) beats agents (70) for the one user slot"
    );
    let lost = item(&h, "omp:instruction:agents:user:~/.agents/AGENTS.md");
    assert_eq!(lost.shadowed_by.as_deref(), Some("omp:instruction:claude:user:~/.claude/CLAUDE.md"));

    let native = opted.file(format!("{AGENT}/AGENTS.md"), "Native home.");
    assert_eq!(effective_instructions(&run(&native)), ["~/.omp/agent/AGENTS.md"]);
}

#[test]
fn claude_config_dir_opts_claude_in() {
    let fs = repo().file("/opt/claude/CLAUDE.md", "Moved Claude home.");
    let h = run_env(&[("CLAUDE_CONFIG_DIR", "/opt/claude")], &fs);
    assert_eq!(effective_instructions(&h), ["/opt/claude/CLAUDE.md"]);
}

#[test]
fn one_project_file_per_depth_the_higher_priority_provider_winning() {
    let fs = repo()
        .file(format!("{REPO}/AGENTS.md"), "Standalone agents.")
        .file(format!("{REPO}/CLAUDE.md"), "Standalone claude.")
        .file(format!("{REPO}/.github/copilot-instructions.md"), "Copilot.")
        .file(format!("{HOME}/work/AGENTS.md"), "Workspace.");
    let h = run(&fs);
    assert_eq!(
        effective_instructions(&h),
        ["~/work/AGENTS.md", ".github/copilot-instructions.md"],
        "github (30) beats agents-md and claude-md (10) at depth 0; the workspace file above the repo loads first"
    );
    assert!(!item(&h, "omp:instruction:agents-md:project:AGENTS.md").effective);
    assert!(!item(&h, "omp:instruction:claude-md:project:CLAUDE.md").effective);

    let native = fs.file(format!("{REPO}/.omp/AGENTS.md"), "Native project.");
    let h = run(&native);
    assert!(item(&h, "omp:instruction:project:.omp/AGENTS.md").effective);
    assert!(!item(&h, "omp:instruction:github:project:.github/copilot-instructions.md").effective);
}

#[test]
fn agents_md_beats_claude_md_at_a_tie_and_empty_files_claim_nothing() {
    let fs = repo().file(format!("{REPO}/AGENTS.md"), "").file(format!("{REPO}/CLAUDE.md"), "Claude.");
    assert_eq!(effective_instructions(&run(&fs)), ["CLAUDE.md"]);
    let fs = repo().file(format!("{REPO}/AGENTS.md"), "Agents.").file(format!("{REPO}/CLAUDE.md"), "Claude.");
    assert_eq!(effective_instructions(&run(&fs)), ["AGENTS.md"]);
}

#[test]
fn the_standalone_walk_stops_before_the_home() {
    let fs = repo().file(format!("{HOME}/AGENTS.md"), "Home file.").file(format!("{REPO}/AGENTS.md"), "Repo.");
    assert_eq!(effective_instructions(&run(&fs)), ["AGENTS.md"]);
}

#[test]
fn the_nearest_non_empty_omp_folder_owns_native_project_files() {
    let fs = repo()
        .file(format!("{REPO}/.omp/settings.json"), "{}")
        .file(format!("{HOME}/work/shop/.omp/AGENTS.md"), "Farther native.");
    let h = run(&fs);
    assert!(
        !h.items.iter().any(|i| i.name.contains("shop/.omp/AGENTS.md")),
        "a nearer .omp without AGENTS.md blocks it"
    );
}

#[test]
fn a_disabled_context_file_lets_the_one_it_shadowed_load() {
    let fs = repo()
        .file(format!("{REPO}/.claude/CLAUDE.md"), "Claude project.")
        .file(format!("{REPO}/AGENTS.md"), "Agents.")
        .file(format!("{REPO}/.omp/config.yml"), "disabledExtensions:\n  - context-file:project:CLAUDE.md\n");
    let h = run(&fs);
    assert_eq!(effective_instructions(&h), ["AGENTS.md"]);
    assert!(!item(&h, "omp:instruction:claude:project:.claude/CLAUDE.md").effective);
}

#[test]
fn a_farther_file_contained_in_a_closer_one_is_dropped() {
    let fs = repo()
        .file(format!("{HOME}/work/AGENTS.md"), "Shared rule.\n\nAnother.")
        .file(format!("{REPO}/AGENTS.md"), "# App\n\nShared rule.\n\nAnother.\n\nApp only.");
    let h = run(&fs);
    assert_eq!(effective_instructions(&h), ["AGENTS.md"]);
    assert!(has_warning(&h, "context-contained"));
}

#[test]
fn extra_names_load_beside_the_slot_winner() {
    let fs = repo()
        .file(format!("{AGENT}/config.yml"), "contextFiles:\n  extra:\n    - AGENTS.local.md\n")
        .file(format!("{REPO}/AGENTS.md"), "Shared.")
        .file(format!("{REPO}/AGENTS.local.md"), "Mine.");
    let h = run(&fs);
    assert_eq!(effective_instructions(&h), ["AGENTS.md", "AGENTS.local.md"]);
}

#[test]
fn at_imports_are_listed_as_imported_items() {
    let fs = repo()
        .file(format!("{REPO}/AGENTS.md"), "See @docs/arch.md.")
        .file(format!("{REPO}/docs/arch.md"), "Architecture.");
    let h = run(&fs);
    let i = item(&h, "omp:instruction:agents-md:project:docs/arch.md");
    let ItemDetail::Instruction(a) = &i.detail else { panic!() };
    assert_eq!(a.imported_by.as_deref(), Some("omp:instruction:agents-md:project:AGENTS.md"));
}

#[test]
fn rules_md_is_sticky_and_rules_without_description_never_load() {
    let fs = repo()
        .file(format!("{REPO}/.omp/RULES.md"), "Never push.")
        .file(
            format!("{REPO}/.cursor/rules/style.mdc"),
            "---\ndescription: Code style\nglobs: [\"*.ts\"]\n---\nUse tabs.",
        )
        .file(format!("{REPO}/.cursor/rules/always.mdc"), "---\nalwaysApply: true\n---\nBe brief.")
        .file(format!("{REPO}/.windsurf/rules/plain.md"), "No front matter.")
        .file(format!("{REPO}/.clinerules"), "Cline rules.");
    let h = run(&fs);
    assert!(item(&h, "omp:instruction:project:.omp/RULES.md").effective);
    let style = item(&h, "omp:instruction:cursor:project:rule:style");
    assert_eq!(style.load.when, LoadWhen::Startup);
    assert_eq!(style.load.chars.value, ("style".len() + "Code style".len() + "*.ts".len()) as u64, "a rulebook line");
    assert_eq!(item(&h, "omp:instruction:cursor:project:rule:always").load.chars.value, "Be brief.".len() as u64);
    assert!(!item(&h, "omp:instruction:windsurf:project:rule:plain").effective);
    assert!(!item(&h, "omp:instruction:cline:project:rule:clinerules").effective);
    assert!(has_warning(&h, "rule-dropped"));
}

#[test]
fn github_instructions_become_rules_by_apply_to() {
    let fs = repo()
        .file(format!("{REPO}/.github/instructions/all.instructions.md"), "---\napplyTo: '**'\n---\nEverywhere.")
        .file(
            format!("{REPO}/.github/instructions/ts.instructions.md"),
            "---\napplyTo: '*.ts, *.tsx'\n---\nTypeScript.",
        );
    let h = run(&fs);
    assert_eq!(item(&h, "omp:instruction:github:project:rule:all").load.chars.value, "Everywhere.".len() as u64);
    let ts = item(&h, "omp:instruction:github:project:rule:ts");
    let ItemDetail::Instruction(a) = &ts.detail else { panic!() };
    assert_eq!(a.applies_to, ["*.ts", "*.tsx"]);
}

#[test]
fn an_unreadable_discovered_skill_is_an_incomplete_read() {
    let file = format!("{REPO}/.omp/skills/deploy/SKILL.md");
    let fs = repo().file(&file, skill("deploy", "Deploy.")).unreadable(&file);
    let h = run(&fs);
    let warning = h.warnings.iter().find(|w| w.code == "incomplete-read-failed").expect("incomplete read");
    assert_eq!(warning.file.as_deref(), Some(".omp/skills/deploy/SKILL.md"));
    assert!(!h.items.iter().any(|i| i.name == "deploy"));
}

#[test]
fn a_second_skill_of_a_name_loads_under_a_namespace() {
    let fs = repo()
        .file(format!("{REPO}/.omp/skills/deploy/SKILL.md"), skill("deploy", "Native deploy."))
        .file(format!("{REPO}/.claude/skills/deploy/SKILL.md"), skill("deploy", "Claude deploy."))
        .file(format!("{REPO}/.agents/skills/deploy/SKILL.md"), skill("deploy", "Native deploy."));
    let h = run(&fs);
    assert!(item(&h, "omp:skill:project:deploy").effective);
    assert!(item(&h, "omp:skill:claude:project:claude/deploy").effective, "different: namespaced, still loaded");
    let a = item(&h, "omp:skill:agents:project:deploy");
    assert_eq!(a.shadowed_by.as_deref(), Some("omp:skill:project:deploy"), "an identical copy is dropped");
    assert!(has_warning(&h, "skill-name-collision"));
}

#[test]
fn an_identical_copy_of_a_skill_is_dropped() {
    let text = skill("lint", "Lint it.");
    let fs = repo()
        .file(format!("{REPO}/.omp/skills/lint/SKILL.md"), &text)
        .file(format!("{HOME}/.agents/skills/lint/SKILL.md"), &text);
    let h = run(&fs);
    assert_eq!(item(&h, "omp:skill:agents:user:lint").shadowed_by.as_deref(), Some("omp:skill:project:lint"));
}

#[test]
fn skills_are_one_level_deep_and_native_ones_need_a_description() {
    let fs = repo()
        .file(format!("{AGENT}/skills/group/inner/SKILL.md"), skill("inner", "Nested."))
        .file(format!("{AGENT}/skills/nodesc/SKILL.md"), "---\nname: nodesc\n---\nBody.")
        .file(format!("{REPO}/.claude/skills/bare/SKILL.md"), "---\nname: bare\n---\nBody.");
    let h = run(&fs);
    assert!(!h.items.iter().any(|i| i.name == "inner" || i.name == "nodesc"));
    assert!(item(&h, "omp:skill:claude:project:bare").effective, "Claude skills load without a description");
}

#[test]
fn a_skills_listing_is_its_name_and_a_100_character_preview() {
    let long = "word ".repeat(60);
    let fs = repo()
        .file(format!("{AGENT}/skills/big/SKILL.md"), skill("big", &long))
        .file(format!("{AGENT}/skills/quiet/SKILL.md"), "---\nname: quiet\ndescription: Hidden.\nhide: true\n---\nB.");
    let h = run(&fs);
    assert_eq!(item(&h, "omp:skill:user:big").load.chars.value, 3 + 100);
    let q = item(&h, "omp:skill:user:quiet");
    assert_eq!((q.load.when, q.load.chars.value), (LoadWhen::OnInvoke, 0));
}

#[test]
fn skill_filters_and_toggles_apply() {
    let fs = repo()
        .file(format!("{REPO}/.claude/skills/a/SKILL.md"), skill("a", "A."))
        .file(format!("{AGENT}/skills/third-party-pdf/SKILL.md"), skill("third-party-pdf", "PDF."))
        .file(
            format!("{AGENT}/config.yml"),
            "skills:\n  enableClaudeProject: false\n  ignoredSkills:\n    - \"third-party-*\"\n",
        );
    let h = run(&fs);
    assert!(!h.items.iter().any(|i| i.name == "a"));
    assert!(!item(&h, "omp:skill:user:third-party-pdf").effective);
}

#[test]
fn disabled_providers_drop_a_whole_source_and_arrays_replace_lower_layers() {
    let fs = repo()
        .file(format!("{AGENT}/config.yml"), "disabledProviders:\n  - claude\n")
        .file(format!("{REPO}/.claude/skills/a/SKILL.md"), skill("a", "A."))
        .file(format!("{REPO}/.github/skills/g/SKILL.md"), skill("g", "G."));
    let h = run(&fs);
    assert!(!h.items.iter().any(|i| i.name == "a"));
    let project = fs.file(format!("{REPO}/.omp/config.yml"), "disabledProviders:\n  - github\n");
    let h = run(&project);
    assert!(h.items.iter().any(|i| i.name == "a" && i.effective), "the project's list replaces the global one");
    assert!(!h.items.iter().any(|i| i.name == "g"));
}

#[test]
fn commands_follow_provider_order_and_prompts_favour_the_user() {
    let fs = repo()
        .file(format!("{REPO}/.omp/commands/ship.md"), "Native ship.")
        .file(format!("{REPO}/.claude/commands/ship.md"), "Claude ship.")
        .file(format!("{REPO}/.claude/commands/ops/deploy.md"), "Nested Claude command.")
        .file(format!("{AGENT}/prompts/review.md"), "User review.")
        .file(format!("{REPO}/.omp/prompts/review.md"), "Project review.");
    let h = run(&fs);
    assert!(item(&h, "omp:command:project:ship").effective);
    assert!(!item(&h, "omp:command:claude:project:ship").effective);
    assert!(item(&h, "omp:command:claude:project:deploy").effective, "Claude commands are found recursively");
    assert!(item(&h, "omp:command:user:prompt:review").effective);
    assert!(!item(&h, "omp:command:project:prompt:review").effective);
}

#[test]
fn task_agents_come_from_omp_folders_only() {
    let fs = repo()
        .file(
            format!("{REPO}/.omp/agents/scout2.md"),
            "---\nname: scout2\ndescription: Finds things.\nmodel: fast\n---\nBody.",
        )
        .file(format!("{AGENT}/agents/scout2.md"), "---\nname: scout2\ndescription: User copy.\n---\nBody.")
        .file(
            format!("{REPO}/.claude/agents/claude-agent.md"),
            "---\nname: claude-agent\ndescription: Not read.\n---\nBody.",
        );
    let h = run(&fs);
    let a = item(&h, "omp:agent:project:scout2");
    assert_eq!(a.load.chars.value, ("scout2".len() + "Finds things.".len()) as u64);
    assert!(!item(&h, "omp:agent:user:scout2").effective);
    assert!(!h.items.iter().any(|i| i.name == "claude-agent"), "OMP does not read .claude/agents");
}

#[test]
fn mcp_first_definition_wins_and_a_disabled_one_still_claims_its_name() {
    let fs = repo()
        .file(format!("{REPO}/.omp/mcp.json"), r#"{"mcpServers": {"db": {"command": "db-mcp", "enabled": false}}}"#)
        .file(format!("{REPO}/.mcp.json"), r#"{"mcpServers": {"db": {"command": "other-db"}, "docs": {"url": "https://docs.example.com/x/k3y-path"}}}"#)
        .file(format!("{REPO}/.cursor/mcp.json"), r#"{"mcpServers": {"docs": {"command": "cursor-docs"}}}"#);
    let h = run(&fs);
    let db = item(&h, "omp:mcp:project:db");
    assert!(!db.effective);
    assert!(!item(&h, "omp:mcp:mcp-json:project:db").effective, "shadowed by the disabled native db");
    assert!(item(&h, "omp:mcp:cursor:project:docs").effective, "cursor (50) is read before the root .mcp.json (5)");
    assert!(!serde_json::to_string(&h).unwrap().contains("k3y-path"));
}

#[test]
fn mcp_project_config_can_be_switched_off_and_servers_disabled_by_name() {
    let fs = repo()
        .file(format!("{REPO}/.mcp.json"), r#"{"mcpServers": {"db": {"command": "db-mcp"}}}"#)
        .file(
            format!("{AGENT}/mcp.json"),
            r#"{"mcpServers": {"web": {"url": "https://web.example.com"}}, "disabledServers": ["web"]}"#,
        )
        .file(format!("{AGENT}/config.yml"), "mcp:\n  enableProjectConfig: false\n");
    let h = run(&fs);
    assert!(!h.items.iter().any(|i| i.name == "db"));
    assert!(!item(&h, "omp:mcp:user:web").effective);
}

#[test]
fn a_standard_vscode_mcp_file_loads_nothing_and_says_so() {
    let fs = repo().file(format!("{REPO}/.vscode/mcp.json"), r#"{"servers": {"gh": {"command": "gh-mcp"}}}"#);
    let h = run(&fs);
    assert!(!h.items.iter().any(|i| i.detail.kind() == "mcp"));
    assert!(has_warning(&h, "vscode-mcp-not-read"));
}

#[test]
fn only_ts_and_js_hooks_run_and_claude_settings_hooks_are_ignored() {
    let fs = repo()
        .file(format!("{REPO}/.omp/hooks/pre/bash.ts"), "export default () => {}")
        .file(format!("{REPO}/.claude/hooks/post/edit.sh"), "#!/bin/sh")
        .file(format!("{REPO}/.claude/settings.json"), r#"{"hooks": {"PreToolUse": []}}"#);
    let h = run(&fs);
    let ts = item(&h, "omp:hook:project:pre:bash.ts");
    let ItemDetail::Hook(a) = &ts.detail else { panic!() };
    assert_eq!((a.event.as_str(), a.matcher.as_deref()), ("before-tool", Some("bash")));
    assert!(!item(&h, "omp:hook:claude:project:post:edit.sh").effective);
    assert!(has_warning(&h, "hook-not-run"));
    assert!(has_warning(&h, "claude-hooks-ignored"));
}

#[test]
fn foreign_project_settings_override_omps_own_except_model_roles() {
    let fs = repo()
        .file(
            format!("{REPO}/.omp/config.yml"),
            "tools:\n  approvalMode: always-ask\nmodelRoles:\n  default: native-model\n",
        )
        .file(
            format!("{REPO}/.claude/settings.json"),
            r#"{"tools": {"approvalMode": "write"}, "modelRoles": {"default": "claude-model"}}"#,
        );
    let h = run(&fs);
    assert_eq!(h.settings["approvalMode"].value, "write");
    assert_eq!(h.settings["model"].value, "native-model");
}

#[test]
fn approval_defaults_to_yolo_and_per_tool_rules_are_permissions() {
    let fs = repo().file(format!("{AGENT}/config.yml"), "tools:\n  approval:\n    bash: prompt\n    write: deny\n");
    let h = run(&fs);
    assert_eq!(h.settings["approvalMode"].value, "yolo");
    let ItemDetail::Permission(p) = &item(&h, "omp:permission:user:approval:write").detail else { panic!() };
    assert_eq!(p.effect, PermissionEffect::Deny);
}

#[test]
fn system_md_replaces_the_default_block_and_one_is_used() {
    let fs = repo()
        .file(format!("{REPO}/.omp/SYSTEM.md"), "Project system.")
        .file(format!("{AGENT}/SYSTEM.md"), "User system.")
        .file(format!("{REPO}/.claude/APPEND_SYSTEM.md"), "Appended.");
    let h = run(&fs);
    assert!(item(&h, "omp:instruction:project:.omp/SYSTEM.md").effective);
    assert!(!item(&h, "omp:instruction:user:~/.omp/agent/SYSTEM.md").effective);
    assert!(item(&h, "omp:instruction:claude:project:.claude/APPEND_SYSTEM.md").effective);
    assert!(h.context.not_measured.iter().any(|n| n.what.contains("replaces OMP's default")));
}

#[test]
fn each_provider_has_its_own_layer_in_priority_order() {
    let fs = repo()
        .file(format!("{REPO}/.omp/AGENTS.md"), "Native.")
        .file(format!("{REPO}/.cursor/rules/r.mdc"), "---\nalwaysApply: true\n---\nR.")
        .file(format!("{REPO}/.claude/skills/s/SKILL.md"), skill("s", "S."));
    let h = run(&fs);
    let ids: Vec<&str> = h.layers.iter().map(|l| l.id.as_str()).collect();
    let pos = |id: &str| ids.iter().position(|x| *x == id).unwrap_or_else(|| panic!("{id} in {ids:?}"));
    assert!(pos("project") < pos("claude:project") && pos("claude:project") < pos("cursor:project"));
}
