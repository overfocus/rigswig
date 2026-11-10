//! The Pi adapter on invented setups, one rule per test (docs/adapters/pi.md). Every output keeps the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Pi;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const AGENT: &str = "/home/dev/.pi/agent";

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
    let h = Pi.resolve(&ctx(env, fs));
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

/// A repo inside a git checkout, the trust store saying yes to it.
fn trusted() -> MemFs {
    MemFs::new().dir(format!("{REPO}/.git")).file(format!("{AGENT}/trust.json"), format!("{{\"{REPO}\": true}}"))
}

fn skill(name: &str, desc: &str) -> String {
    format!("---\nname: {name}\ndescription: {desc}\n---\n\nDo the thing.\n")
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    let d = |fs: &MemFs| Pi.detect(&ctx(&[], fs)).score;
    assert_eq!(d(&MemFs::new().dir(REPO)), 0);
    assert_eq!(d(&MemFs::new().dir(REPO).dir(AGENT)), 45, "an installed Pi alone is home-folder evidence");
    assert_eq!(d(&MemFs::new().dir(format!("{REPO}/.agents/skills"))), 60);
    assert_eq!(d(&MemFs::new().file(format!("{REPO}/.pi/settings.json"), "{}")), 100);
}

#[test]
fn the_agent_dir_moves_with_the_documented_variable() {
    let fs = MemFs::new().dir(REPO).file("/opt/pi-agent/AGENTS.md", "Company rules.");
    let h = run_env(&[("PI_CODING_AGENT_DIR", "/opt/pi-agent")], &fs);
    assert!(item(&h, "pi:instruction:user:/opt/pi-agent/AGENTS.md").effective);
    assert!(h.layers.iter().any(|l| l.id == "user" && l.present));
}

#[test]
fn context_files_load_from_the_agent_dir_then_root_down_to_the_repo() {
    let fs = MemFs::new()
        .file(format!("{AGENT}/AGENTS.md"), "Global.")
        .file(format!("{HOME}/work/AGENTS.md"), "Work.")
        .file(format!("{REPO}/AGENTS.md"), "Repo.");
    let h = run(&fs);
    let order: Vec<&str> =
        h.items.iter().filter(|i| i.effective && i.detail.kind() == "instruction").map(|i| i.name.as_str()).collect();
    assert_eq!(order, ["~/.pi/agent/AGENTS.md", "~/work/AGENTS.md", "AGENTS.md"]);
    assert_eq!(h.context.chars.value, ("Global.".len() + "Work.".len() + "Repo.".len()) as u64);
}

#[test]
fn the_first_candidate_in_a_directory_wins_and_claude_md_is_shadowed() {
    let fs = MemFs::new()
        .file(format!("{REPO}/AGENTS.md"), "Agents.")
        .file(format!("{REPO}/CLAUDE.md"), "Claude.")
        .file(format!("{HOME}/work/AGENTS.override.md"), "Override.")
        .file(format!("{HOME}/work/AGENTS.md"), "Shadowed.");
    let h = run(&fs);
    let claude = item(&h, "pi:instruction:project:CLAUDE.md");
    assert!(!claude.effective);
    assert_eq!(claude.shadowed_by.as_deref(), Some("pi:instruction:project:AGENTS.md"));
    assert!(item(&h, "pi:instruction:project:~/work/AGENTS.override.md").effective);
    assert!(!item(&h, "pi:instruction:project:~/work/AGENTS.md").effective);
}

#[test]
fn upper_case_spellings_count_only_when_the_folder_holds_them() {
    let fs = MemFs::new()
        .file(format!("{REPO}/AGENTS.md"), "Agents.")
        .file(format!("{REPO}/CLAUDE.md"), "Claude.")
        .file(format!("{REPO}/CLAUDE.MD"), "Claude, shouting.");
    let h = run(&fs);
    assert!(!item(&h, "pi:instruction:project:CLAUDE.MD").effective, "a real second file is listed, shadowed");
    let n = h.items.iter().filter(|i| i.detail.kind() == "instruction").count();
    assert_eq!(n, 3);
}

#[test]
fn a_repo_without_gated_resources_needs_no_trust_and_its_context_files_load() {
    let fs = MemFs::new().dir(format!("{REPO}/.git")).file(format!("{REPO}/AGENTS.md"), "Repo.");
    let h = run(&fs);
    assert!(!h.settings.contains_key("projectTrust"));
    assert!(h.warnings.is_empty(), "{:?}", h.warnings);
    assert!(item(&h, "pi:instruction:project:AGENTS.md").effective);
}

#[test]
fn an_untrusted_project_keeps_its_pi_resources_out_but_not_its_agents_md() {
    let fs = MemFs::new()
        .dir(format!("{REPO}/.git"))
        .file(format!("{AGENT}/trust.json"), format!("{{\"{REPO}\": false}}"))
        .file(format!("{REPO}/AGENTS.md"), "Repo.")
        .file(format!("{REPO}/.pi/settings.json"), r#"{"defaultModel": "repo-model"}"#)
        .file(format!("{REPO}/.pi/skills/deploy/SKILL.md"), skill("deploy", "Ship it."))
        .file(format!("{REPO}/.pi/prompts/review.md"), "Review this.")
        .file(format!("{REPO}/.pi/APPEND_SYSTEM.md"), "Extra.")
        .file(format!("{REPO}/.pi/mcp.json"), r#"{"mcpServers": {"db": {"command": "db-mcp"}}}"#);
    let h = run(&fs);
    assert_eq!(h.settings["projectTrust"].value, "untrusted");
    assert!(has_warning(&h, "project-untrusted"));
    assert!(!h.settings.contains_key("model"), "untrusted project settings do not apply");
    for id in [
        "pi:skill:project:deploy",
        "pi:command:project:review",
        "pi:instruction:project:.pi/APPEND_SYSTEM.md",
        "pi:mcp:project:db",
    ] {
        let i = item(&h, id);
        assert!(!i.effective && i.load.when == LoadWhen::Never, "{id} must not load");
    }
    assert!(item(&h, "pi:instruction:project:AGENTS.md").effective);
}

#[test]
fn trust_comes_from_the_nearest_saved_ancestor_then_default_project_trust() {
    let gated = |fs: MemFs| fs.file(format!("{REPO}/.pi/skills/s/SKILL.md"), skill("s", "S."));
    let parent = gated(
        MemFs::new().file(format!("{AGENT}/trust.json"), format!("{{\"{HOME}/work\": true, \"{HOME}\": false}}")),
    );
    let h = run(&parent);
    assert_eq!(h.settings["projectTrust"].value, "trusted");
    assert!(item(&h, "pi:skill:project:s").effective);

    let always = gated(MemFs::new().file(format!("{AGENT}/settings.json"), r#"{"defaultProjectTrust": "always"}"#));
    assert_eq!(run(&always).settings["projectTrust"].value, "trusted");

    let never = gated(MemFs::new().file(format!("{AGENT}/settings.json"), r#"{"defaultProjectTrust": "never"}"#));
    assert!(!item(&run(&never), "pi:skill:project:s").effective);

    // A project cannot set defaultProjectTrust for itself.
    let own = gated(MemFs::new().file(format!("{REPO}/.pi/settings.json"), r#"{"defaultProjectTrust": "always"}"#));
    let h = run(&own);
    assert_eq!(h.settings["projectTrust"].value, "ask");
    assert!(!h.settings.contains_key("defaultProjectTrust"));
}

#[test]
fn an_undecided_project_is_counted_as_trusted_with_a_warning() {
    let fs = MemFs::new().file(format!("{REPO}/.pi/skills/s/SKILL.md"), skill("s", "S."));
    let h = run(&fs);
    assert_eq!(h.settings["projectTrust"].value, "ask");
    assert!(has_warning(&h, "project-trust-undecided"));
    assert!(item(&h, "pi:skill:project:s").effective);
}

#[test]
fn project_settings_win_over_user_settings_with_a_trail() {
    let fs = trusted()
        .file(format!("{AGENT}/settings.json"), r#"{"defaultModel": "user-model", "defaultProvider": "acme"}"#)
        .file(format!("{REPO}/.pi/settings.json"), r#"{"defaultModel": "repo-model"}"#);
    let h = run(&fs);
    let m = &h.settings["model"];
    assert_eq!(m.value, "repo-model");
    assert_eq!(m.source.as_deref(), Some("project"));
    assert_eq!(m.trail.len(), 2);
    assert_eq!(h.settings["provider"].source.as_deref(), Some("user"));
}

#[test]
fn skills_follow_pis_precedence_and_the_first_of_a_name_wins() {
    let fs = trusted()
        .file(format!("{REPO}/.pi/skills/deploy/SKILL.md"), skill("deploy", "Project deploy."))
        .file(format!("{AGENT}/skills/deploy/SKILL.md"), skill("deploy", "User deploy."))
        .file(format!("{HOME}/.agents/skills/lint/SKILL.md"), skill("lint", "Lint."))
        .file(format!("{REPO}/.agents/skills/lint/SKILL.md"), skill("lint", "Repo lint."));
    let h = run(&fs);
    assert!(item(&h, "pi:skill:project:deploy").effective);
    assert_eq!(item(&h, "pi:skill:user:deploy").shadowed_by.as_deref(), Some("pi:skill:project:deploy"));
    assert!(item(&h, "pi:skill:project:lint").effective, "project .agents/skills beat ~/.agents/skills");
    assert!(!item(&h, "pi:skill:user:lint").effective);
    assert!(has_warning(&h, "skill-name-collision"));
}

#[test]
fn skill_discovery_stops_at_skill_md_and_takes_loose_files_per_mode() {
    let fs = MemFs::new()
        .file(format!("{AGENT}/skills/top.md"), skill("top", "A loose file at the top of a Pi folder."))
        .file(format!("{AGENT}/skills/group/nested.md"), skill("nested", "Loose and nested: not in a Pi folder."))
        .file(format!("{AGENT}/skills/group/deep/SKILL.md"), skill("deep", "Found by recursion."))
        .file(format!("{AGENT}/skills/deep/SKILL.md"), skill("whole", "A skill folder."))
        .file(format!("{AGENT}/skills/deep/inner/SKILL.md"), skill("inner", "Below a skill: never read."))
        .file(format!("{HOME}/.agents/skills/top.md"), skill("agents-top", "Top of an Agent Skills folder: skipped."))
        .file(format!("{HOME}/.agents/skills/g/n.md"), skill("agents-nested", "Nested in an Agent Skills folder."))
        .file(format!("{AGENT}/skills/nodesc/SKILL.md"), "---\nname: nodesc\n---\nNo description.");
    let h = run(&fs);
    let names: Vec<&str> = h.items.iter().filter(|i| i.detail.kind() == "skill").map(|i| i.name.as_str()).collect();
    for n in ["top", "deep", "whole", "agents-nested"] {
        assert!(names.contains(&n), "{n} in {names:?}");
    }
    for n in ["nested", "inner", "agents-top", "nodesc"] {
        assert!(!names.contains(&n), "{n} must not be found: {names:?}");
    }
}

#[test]
fn a_skills_listing_is_its_name_description_and_path() {
    let path = format!("{AGENT}/skills/deploy/SKILL.md");
    let fs = MemFs::new().file(&path, skill("deploy", "Ship it.")).file(
        format!("{AGENT}/skills/quiet/SKILL.md"),
        "---\nname: quiet\ndescription: Only by command.\ndisable-model-invocation: true\n---\nBody.",
    );
    let h = run(&fs);
    let d = item(&h, "pi:skill:user:deploy");
    assert_eq!(d.load.when, LoadWhen::Startup);
    assert_eq!(d.load.chars.value, ("deploy".len() + "Ship it.".len() + path.len()) as u64);
    let q = item(&h, "pi:skill:user:quiet");
    assert_eq!((q.load.when, q.load.chars.value), (LoadWhen::OnInvoke, 0), "hidden from the model's listing");
}

#[test]
fn project_agents_skills_stop_at_the_git_root() {
    let fs = MemFs::new()
        .dir(format!("{REPO}/.git"))
        .file(format!("{AGENT}/trust.json"), format!("{{\"{REPO}\": true}}"))
        .file(format!("{HOME}/work/.agents/skills/above/SKILL.md"), skill("above", "Above the git root."));
    let h = run(&fs);
    assert!(!h.items.iter().any(|i| i.name == "above"));
}

#[test]
fn prompt_templates_are_commands_whose_body_loads_on_invoke() {
    let fs = trusted()
        .file(format!("{REPO}/.pi/prompts/review.md"), "---\ndescription: Review\n---\nReview the diff.")
        .file(format!("{AGENT}/prompts/review.md"), "User review.")
        .file(format!("{AGENT}/prompts/nested/skip.md"), "Not a direct child.");
    let h = run(&fs);
    let r = item(&h, "pi:command:project:review");
    assert_eq!((r.load.when, r.load.chars.value), (LoadWhen::OnInvoke, "Review the diff.".len() as u64));
    assert!(!item(&h, "pi:command:user:review").effective);
    assert!(!h.items.iter().any(|i| i.name == "skip"));
}

#[test]
fn system_files_take_the_project_copy_over_the_users() {
    let fs = trusted()
        .file(format!("{AGENT}/APPEND_SYSTEM.md"), "User append.")
        .file(format!("{REPO}/.pi/APPEND_SYSTEM.md"), "Repo append.")
        .file(format!("{AGENT}/SYSTEM.md"), "My own system prompt.");
    let h = run(&fs);
    assert!(item(&h, "pi:instruction:project:.pi/APPEND_SYSTEM.md").effective);
    assert!(!item(&h, "pi:instruction:user:~/.pi/agent/APPEND_SYSTEM.md").effective);
    assert!(item(&h, "pi:instruction:user:~/.pi/agent/SYSTEM.md").effective);
    assert!(h.context.not_measured.iter().any(|n| n.what.contains("SYSTEM.md replaces")));
}

#[test]
fn mcp_project_entries_replace_or_toggle_the_users_and_never_leak_secrets() {
    let fs = trusted()
        .file(
            format!("{AGENT}/mcp.json"),
            r#"{"mcpServers": {
                "docs": {"url": "https://docs.example.com/mcp/secret-path", "headers": {"Authorization": "Bearer s3cr3t"}},
                "files": {"command": "/usr/local/bin/files-mcp", "args": ["--token", "s3cr3t"]},
                "tools": {"command": "tools-mcp"}
            }}"#,
        )
        .file(
            format!("{REPO}/.pi/mcp.json"),
            r#"{"mcpServers": {"files": {"enabled": false}, "tools": {"command": "repo-tools"}}}"#,
        );
    let h = run(&fs);
    let docs = item(&h, "pi:mcp:user:docs");
    let ItemDetail::Mcp(m) = &docs.detail else { panic!() };
    assert_eq!(m.target.as_deref(), Some("https://docs.example.com"));
    assert!(!item(&h, "pi:mcp:user:files").effective, "the project turns the user's server off");
    assert!(item(&h, "pi:mcp:project:tools").effective);
    assert_eq!(item(&h, "pi:mcp:user:tools").shadowed_by.as_deref(), Some("pi:mcp:project:tools"));
    let json = serde_json::to_string(&h).unwrap();
    assert!(!json.contains("s3cr3t") && !json.contains("secret-path"));
}

#[test]
fn turning_off_builtin_mcp_turns_off_every_server() {
    let fs = MemFs::new()
        .file(format!("{AGENT}/settings.json"), r#"{"extensions": ["-builtin:mcp"]}"#)
        .file(format!("{AGENT}/mcp.json"), r#"{"mcpServers": {"docs": {"command": "docs-mcp"}}}"#);
    let h = run(&fs);
    assert!(!item(&h, "pi:mcp:user:docs").effective);
    assert!(has_warning(&h, "mcp-builtin-off"));
}

#[test]
fn installed_packages_contribute_skills_and_prompts_last() {
    let pkg = format!("{AGENT}/npm/node_modules/@acme/pi-kit");
    let fs = MemFs::new()
        .file(
            format!("{AGENT}/settings.json"),
            r#"{"packages": ["npm:@acme/pi-kit@1.2.0", "git:github.com/acme/pi-tools@v2"]}"#,
        )
        .file(format!("{pkg}/package.json"), r#"{"name": "@acme/pi-kit", "pi": {"skills": ["./res/skills"]}}"#)
        .file(format!("{pkg}/res/skills/kit/SKILL.md"), skill("kit", "From the kit."))
        .file(format!("{pkg}/res/skills/deploy/SKILL.md"), skill("deploy", "Package deploy."))
        .file(format!("{AGENT}/skills/deploy/SKILL.md"), skill("deploy", "User deploy."))
        .file(format!("{AGENT}/git/github.com/acme/pi-tools/prompts/ship.md"), "Ship.");
    let h = run(&fs);
    let kit = h.packages.iter().find(|p| p.source == "npm:@acme/pi-kit").expect("npm package");
    assert!(kit.contributes.iter().any(|c| c.ends_with(":kit")));
    assert!(h.packages.iter().any(|p| p.source == "github.com/acme/pi-tools"));
    assert!(h.items.iter().any(|i| i.name == "ship" && i.effective));
    let user_deploy = item(&h, "pi:skill:user:deploy");
    assert!(user_deploy.effective, "a user skill beats a package's");
}

#[test]
fn a_package_in_settings_but_not_installed_is_reported() {
    let fs = MemFs::new().file(format!("{AGENT}/settings.json"), r#"{"packages": ["npm:missing-kit"]}"#);
    let h = run(&fs);
    assert!(has_warning(&h, "incomplete-package-not-installed"));
    assert!(
        h.warnings.iter().find(|w| w.code == "incomplete-package-not-installed").is_some_and(|w| w.is_incomplete())
    );
    assert!(h.layers.iter().any(|l| l.kind == LayerKind::Package && !l.present));
}

#[test]
fn malformed_settings_warn_without_quoting_content() {
    let fs = MemFs::new().file(format!("{AGENT}/settings.json"), "{ \"defaultModel\": \"s3cr3t\"");
    let h = run(&fs);
    assert!(has_warning(&h, "incomplete-malformed-json"));
    assert!(!serde_json::to_string(&h).unwrap().contains("s3cr3t"));
}
