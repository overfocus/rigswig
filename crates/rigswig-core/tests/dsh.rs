//! The DeepSeek Harness adapter on invented setups, one rule per test (docs/adapters/dsh.md). Every output
//! keeps the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Dsh;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop/app";
const DSH: &str = "/home/dev/.dsh";

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
    let h = Dsh.resolve(&ctx(env, fs));
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
    Dsh.detect(&ctx(&[], fs)).score
}

#[test]
fn detect_prefers_the_repo_but_never_over_50_from_the_home_alone() {
    assert_eq!(detect(&MemFs::new().dir(REPO)), 0);
    let home = MemFs::new().dir(REPO).dir(DSH);
    assert_eq!(detect(&home), 45, "an installed harness alone is home-folder evidence");
    let agents = MemFs::new().dir(REPO).dir(format!("{REPO}/.agents/skills"));
    assert_eq!(detect(&agents), 60);
    let repo = MemFs::new().dir(REPO).file(format!("{REPO}/.dsh/skills/README.md"), "x");
    assert_eq!(detect(&repo), 100, "the repo's own .dsh/ is repo evidence");
}

#[test]
fn dsh_home_moves_with_the_documented_variable() {
    let fs = MemFs::new().dir(REPO).file("/opt/company/dsh/AGENTS.md", "Company rules.");
    let h = run_env(&[("DSH_HOME", "/opt/company/dsh")], &fs);
    assert!(h.items.iter().any(|i| i.name == "/opt/company/dsh/AGENTS.md" && i.effective));
    assert!(h.layers.iter().any(|l| l.id == "user" && l.present));
}

#[test]
fn config_layers_follow_the_documented_rank() {
    let fs = MemFs::new().dir(REPO).dir(DSH).file(format!("{DSH}/AGENTS.md"), "Home rules.");
    let h = run(&fs);
    for id in ["user", "project"] {
        assert!(h.layers.iter().any(|l| l.id == id), "layer {id}");
    }
    assert!(h.layers.iter().find(|l| l.id == "user").is_some_and(|l| l.present));
}

#[test]
fn the_user_global_agents_md_loads_into_every_session() {
    let fs = MemFs::new().dir(REPO).file(format!("{DSH}/AGENTS.md"), "Home always-on.");
    let h = run(&fs);
    let g = item(&h, "dsh:instruction:user:~/.dsh/AGENTS.md");
    assert_eq!(g.load.when, LoadWhen::Startup);
    assert_eq!(g.load.chars.value, 15);
}

#[test]
fn the_project_chain_loads_base_then_overlays_with_the_dedup_rule() {
    let fs = MemFs::new()
        .dir(REPO)
        // The base files load in order...
        .file(format!("{REPO}/AGENTS.md"), "Standard rules.")
        // ...the overlays load after them (additive)...
        .file(format!("{REPO}/CLAUDE.md"), "Claude rules.")
        .file(format!("{REPO}/CLAUDE.local.md"), "Personal overlay.")
        // ...and a directory-identical twin collapses behind the earlier candidate.
        .file(format!("{REPO}/AGENTS.local.md"), "Standard rules.");
    let h = run(&fs);
    assert_eq!(item(&h, "dsh:instruction:project:AGENTS.md").load.when, LoadWhen::Startup);
    item(&h, "dsh:instruction:project:CLAUDE.md");
    item(&h, "dsh:instruction:project:CLAUDE.local.md");
    assert!(
        !h.items.iter().any(|i| i.name == "AGENTS.local.md"),
        "the byte-identical overlay renders once, as the earlier candidate"
    );
    assert!(h.warnings.iter().any(|w| w.code == "instruction-duplicate"), "the collapse is said aloud");
}

#[test]
fn nested_instruction_files_are_lazy() {
    let fs = MemFs::new().dir(REPO).file(format!("{REPO}/packages/api/AGENTS.md"), "API rules.");
    let h = run(&fs);
    let nested = item(&h, "dsh:instruction:project:packages/api/AGENTS.md");
    assert_eq!(nested.load.when, LoadWhen::OnInvoke, "discovered on file touch, out of the startup total");
}

#[test]
fn skills_follow_the_documented_rank_and_layouts() {
    let fs = MemFs::new()
        .dir(REPO)
        .file(format!("{REPO}/.dsh/skills/review.md"), "Review steps.")
        .file(
            format!("{REPO}/.dsh/skills/audit/SKILL.md"),
            "---\nname: audit\ndescription: Audits code\n---\nAudit steps.",
        )
        .file(format!("{REPO}/.agents/skills/review.md"), "A lower-ranked twin.")
        .file(format!("{DSH}/skills/greet.md"), "Greet the user.")
        .file(format!("{HOME}/.agents/skills/plan.md"), "Plan the work.");
    let h = run(&fs);
    let review = item(&h, "dsh:skill:project:review");
    assert!(review.effective, "project-.dsh outranks project-.agents on the same name");
    assert!(h.warnings.iter().any(|w| w.code == "incomplete-duplicate-name"), "the loser is named");
    let audit = item(&h, "dsh:skill:project:audit");
    match &audit.detail {
        ItemDetail::Skill(s) => assert!(s.body_chars.unwrap() > 0),
        other => panic!("not a skill: {other:?}"),
    }
    item(&h, "dsh:skill:user:greet");
    item(&h, "dsh:skill:user:plan");
    let effective: Vec<&Item> = h.items.iter().filter(|i| i.detail.kind() == "skill" && i.effective).collect();
    assert_eq!(
        effective.len(),
        4,
        "the clashing twin is listed but not effective; have {:#?}",
        h.items.iter().filter(|i| i.detail.kind() == "skill").map(|i| (&i.id, i.effective)).collect::<Vec<_>>()
    );
}
