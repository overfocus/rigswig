//! What-if removals keep the contract and recompute the totals.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::ClaudeCode;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;
use rigswig_core::whatif::without;

const HOME: &str = "/home/dev";
const REPO: &str = "/w/app";

fn harness() -> Harness {
    let mk = format!("{HOME}/mk");
    let fs = MemFs::new()
        .file(format!("{REPO}/CLAUDE.md"), "project rules")
        .file(format!("{HOME}/.claude/agents/a.md"), "---\ndescription: user agent a\n---\n")
        .file(format!("{HOME}/.claude/agents/b.md"), "---\ndescription: user agent b\n---\n")
        .file(format!("{REPO}/.claude/agents/a.md"), "---\ndescription: project a\n---\n")
        .file(
            format!("{HOME}/.claude/plugins/known_marketplaces.json"),
            format!(r#"{{"mk":{{"source":{{"source":"directory","path":"{mk}"}}}}}}"#),
        )
        .file(format!("{mk}/.claude-plugin/marketplace.json"), r#"{"plugins":[{"name":"fmt","source":"./fmt"}]}"#)
        .file(format!("{mk}/fmt/skills/pretty/SKILL.md"), "---\ndescription: formats code nicely\n---\n")
        .file(format!("{HOME}/.claude/settings.json"), r#"{"enabledPlugins":{"fmt@mk":true}}"#);
    let ctx =
        Ctx { fs: &fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
    ClaudeCode.resolve(&ctx)
}

#[test]
fn removing_a_plugin_a_kind_and_a_layer() {
    let h = harness();
    let before = h.context.chars.value;

    let (w, counts) = without(&h, &["fmt@mk".into()]);
    assert_eq!(counts, [("fmt@mk".to_string(), 1)]);
    assert_eq!(contract_errors(&w), Vec::<String>::new());
    let skill = h.items.iter().find(|i| i.name == "fmt:pretty").unwrap().load.chars.value;
    assert_eq!(w.context.chars.value, before - skill);
    assert!(w.warnings.iter().any(|x| x.code == "what-if"));

    let (w, counts) = without(&h, &["agent:*".into(), "layer:project".into()]);
    assert_eq!(counts[0].1, 2, "the two effective agents (the user `a` is already overridden)");
    assert_eq!(counts[1].1, 1, "CLAUDE.md is all that is left in the project layer");
    assert_eq!(contract_errors(&w), Vec::<String>::new());
    assert!(
        w.coverage.not_modelled.iter().any(|n| n.contains("not brought back")),
        "the project `a` overrode the user `a`"
    );

    let (same, counts) = without(&h, &["skill:nothing-like-this".into()]);
    assert_eq!(counts[0].1, 0);
    assert_eq!(same, h, "a pattern that matches nothing changes nothing");
}
