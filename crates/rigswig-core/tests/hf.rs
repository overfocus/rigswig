//! The Hugging Face CLI adapter on invented setups, one rule per test (docs/adapters/hf.md). Every output
//! keeps the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Hf;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::Os;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const GLOBAL: &str = "/home/dev/.agents/skills";
const MIRROR: &str = "/home/dev/.claude/skills";

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
    let h = Hf.resolve(&ctx(env, fs));
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

/// The generated hf-cli skill exactly as the CLI writes it: a version stamp line in the body.
fn generated_skill(version: &str) -> String {
    format!(
        "---\nname: hf-cli\ndescription: \"Hugging Face Hub CLI.\"\n---\n\nUse `hf --help`.\n\nGenerated with \
         `huggingface_hub v{version}`. Run `hf skills add --force` to regenerate.\n"
    )
}

/// The standalone install the official installer leaves behind: the version rigswig reads from disk.
fn installed_cli(fs: MemFs, version: &str) -> MemFs {
    fs.dir(format!("{HOME}/.hf-cli/venv/lib/python3.14/site-packages/huggingface_hub-{version}.dist-info"))
        .file(format!("{HOME}/.hf-cli/venv/bin/hf-entry"), "")
}

fn detect(fs: &MemFs) -> u8 {
    Hf.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new()), 0);
    let global_only = MemFs::new().dir(GLOBAL);
    assert_eq!(detect(&global_only), 45, "the global skills root alone is home-folder evidence");
    let cli_only = MemFs::new().dir(format!("{HOME}/.hf-cli"));
    assert_eq!(detect(&cli_only), 45, "the installed CLI alone is home-folder evidence");
    let repo = MemFs::new().dir(format!("{REPO}/.agents/skills"));
    assert_eq!(detect(&repo), 100, "the repo's skills store is repo evidence");
}

#[test]
fn the_global_skills_root_listed_in_every_session_and_the_project_one_per_repo() {
    let fs = installed_cli(
        MemFs::new().dir(REPO).file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0")),
        "2.4.0",
    );
    let h = run(&fs);
    item(&h, "hf:skill:user:hf-cli");
    assert!(h.layers.iter().any(|l| l.id == "user" && l.present));
    // The store layer exists but sits empty: the repo has no skills of its own.
    assert!(!h.layers.iter().any(|l| l.id == "project" && l.present));
    assert!(h.layers.iter().all(|l| !(l.id.starts_with("imports") && l.present)));
}

#[test]
fn the_claude_mirror_shows_as_a_twin_and_the_canonical_copy_wins() {
    let fs = installed_cli(
        MemFs::new()
            .dir(REPO)
            .file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0"))
            .symlink(format!("{MIRROR}/hf-cli"), "../../.agents/skills/hf-cli"),
        "2.4.0",
    );
    let h = run(&fs);
    let canonical = item(&h, "hf:skill:user:hf-cli");
    assert!(canonical.effective);
    let mirror = item(&h, "hf:skill:imports_user:hf-cli");
    assert!(!mirror.effective, "the mirror stays listed but the canonical copy wins the name");
    assert!(mirror.load.content_hash == canonical.load.content_hash, "the mirror twins by content");
}

#[test]
fn a_claude_skill_without_a_pair_in_the_store_stays_out() {
    let fs = installed_cli(
        MemFs::new()
            .dir(REPO)
            .file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0"))
            // Another tool's skill inside the mirror: canonicalizes nowhere among hf's stores, so it
            // belongs to the Claude Code adapter, not here.
            .file(format!("{MIRROR}/synced/SKILL.md"), "---\nname: synced\n---\nOther tool's skill."),
        "2.4.0",
    );
    let h = run(&fs);
    item(&h, "hf:skill:user:hf-cli");
    assert!(h.items.iter().all(|i| i.name != "synced"), "the other tool's skill is not hf's to list");
}

#[test]
fn project_store_skills_and_the_project_mirror_work_the_same_way() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/.agents/skills/hf-mem/SKILL.md"), "---\nname: hf-mem\n---\nMemory skill.")
        .file(format!("{REPO}/.agents/skills/hf-cli/SKILL.md"), generated_skill("2.4.0"))
        .symlink(format!("{REPO}/.claude/skills/hf-mem"), "../../.agents/skills/hf-mem");
    let h = run(&fs);
    item(&h, "hf:skill:project:hf-mem");
    item(&h, "hf:skill:project:hf-cli");
    let mirror = item(&h, "hf:skill:imports_project:hf-mem");
    assert!(!mirror.effective, "the project mirror twin is listed but loses to the store's copy");
}

#[test]
fn a_fresh_generated_skill_warns_nothing() {
    let fs = installed_cli(
        MemFs::new().dir(REPO).file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0")),
        "2.4.0",
    );
    let h = run(&fs);
    assert!(h.warnings.is_empty(), "warnings: {:#?}", h.warnings);
}

#[test]
fn the_staleness_audit_warns_on_another_version_and_on_a_missing_stamp() {
    // Generated by an older CLI: the exact situation the CLI's own checker hints about.
    let fs = installed_cli(
        MemFs::new().dir(REPO).file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.3.9")),
        "2.4.0",
    );
    let h = run(&fs);
    let warn = h.warnings.iter().find(|w| w.code == "hf-cli-skill-stale").expect("a staleness warning");
    assert!(warn.message.contains("v2.3.9") && warn.message.contains("v2.4.0"), "{}", warn.message);
    assert!(warn.message.contains("global"), "the scope names the root: {}", warn.message);

    // No stamp at all (hand-written or future format): still stale, per the CLI's `match is None` rule.
    let fs = installed_cli(
        MemFs::new().dir(REPO).file(format!("{GLOBAL}/hf-cli/SKILL.md"), "---\nname: hf-cli\n---\nBody."),
        "2.4.0",
    );
    let h = run(&fs);
    assert!(
        h.warnings.iter().any(|w| w.code == "hf-cli-skill-stale" && w.message.contains("no generated-version stamp"))
    );

    // One stale copy per root: the project store is warned too, with its own scope.
    let fs = installed_cli(
        MemFs::new()
            .dir(REPO)
            .file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0"))
            .file(format!("{REPO}/.agents/skills/hf-cli/SKILL.md"), generated_skill("2.3.0")),
        "2.4.0",
    );
    let h = run(&fs);
    let scopes: Vec<String> =
        h.warnings.iter().filter(|w| w.code == "hf-cli-skill-stale").map(|w| w.message.clone()).collect();
    assert_eq!(scopes.len(), 1, "only the stale root warns: {scopes:?}");
    assert!(scopes[0].contains("project"));
}

#[test]
fn an_unfindable_cli_version_cannot_warn_a_mismatch_but_can_still_warn_a_missing_stamp() {
    // No dist-info anywhere: a stamp exists and matches the format, so no mismatch is claimed.
    let fs = MemFs::new().dir(REPO).file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.3.9"));
    let h = run(&fs);
    let warn = h.warnings.iter().find(|w| w.code == "hf-cli-skill-stale").expect("a staleness warning");
    assert!(warn.message.contains("no installed version was found"), "{}", warn.message);
}

#[test]
fn claude_config_dir_moves_the_mirror() {
    let fs = installed_cli(
        MemFs::new()
            .dir(REPO)
            .file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0"))
            .symlink("/opt/company/cc/skills/hf-cli", "../../../../home/dev/.agents/skills/hf-cli"),
        "2.4.0",
    );
    let h = run_env(&[("CLAUDE_CONFIG_DIR", "/opt/company/cc")], &fs);
    item(&h, "hf:skill:imports_user:hf-cli");
}

#[test]
fn config_layers_follow_the_modelled_pairs() {
    let fs = installed_cli(
        MemFs::new().dir(REPO).file(format!("{GLOBAL}/hf-cli/SKILL.md"), generated_skill("2.4.0")).dir(GLOBAL),
        "2.4.0",
    );
    let h = run(&fs);
    for id in ["project", "user", "imports_project", "imports_user"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
    // The version the tool reports is the release the docs follow.
    assert_eq!(h.tool.tool_version_assumed.as_deref(), Some("2.4.0"));
}
