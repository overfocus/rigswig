//! Comparing harnesses: two repos with one tool, and two tools on one repo.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::{ClaudeCode, Codex};
use rigswig_core::compare::compare;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";

fn resolve(a: &dyn Adapter, fs: &MemFs, repo: &str) -> Harness {
    let ctx = Ctx { fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(repo), os: Os::Linux };
    a.resolve(&ctx)
}

#[test]
fn two_repos_one_tool() {
    let agent = |d: &str| format!("---\ndescription: {d}\n---\n");
    let fs = MemFs::new()
        .file(format!("{HOME}/.claude/agents/shared.md"), agent("in both, from the user folder"))
        .file("/w/shop/CLAUDE.md", "shop rules, longer than the other")
        .file("/w/shop/.claude/agents/only-shop.md", agent("shop only"))
        .file("/w/shop/.claude/settings.json", r#"{"model":"big"}"#)
        .file("/w/blog/CLAUDE.md", "blog rules")
        .file("/w/blog/.claude/agents/only-blog.md", agent("blog only"));
    let c = compare(&resolve(&ClaudeCode, &fs, "/w/shop"), &resolve(&ClaudeCode, &fs, "/w/blog"));

    assert_eq!((c.a.repo.as_str(), c.b.repo.as_str()), ("shop", "blog"));
    assert_eq!(c.only_a.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["only-shop"]);
    assert_eq!(c.only_b.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["only-blog"]);
    assert_eq!(c.kinds["agent"].same, 1, "the user agent is the same file on both sides");
    let claude_md: Vec<_> = c.changed.iter().filter(|x| x.name == "CLAUDE.md").collect();
    assert_eq!(claude_md.len(), 1, "same name, different content");
    assert!(claude_md[0].a.chars > claude_md[0].b.chars);
    assert_eq!(c.settings.len(), 1);
    assert_eq!(c.settings[0].name, "model");
    assert_eq!(c.settings[0].a, "big");
    assert_eq!(c.settings[0].b, serde_json::Value::Null);
    assert!(c.a.chars > c.b.chars);
}

#[test]
fn two_tools_one_repo_share_agents_md_but_not_its_budget() {
    let long = "x".repeat(40_000);
    let fs = MemFs::new().dir("/w/app/.git").file("/w/app/AGENTS.md", long.clone());
    let cc = resolve(&ClaudeCode, &fs, "/w/app");
    let cx = resolve(&Codex, &fs, "/w/app");
    let c = compare(&cc, &cx);
    assert_eq!((c.a.tool.as_str(), c.b.tool.as_str()), ("claude-code", "codex"));
    let agents_md = c.changed.iter().find(|x| x.name == "AGENTS.md").expect("compared");
    assert_eq!(agents_md.a.chars, 40_000, "Claude Code reads the whole file");
    assert_eq!(agents_md.b.chars, 32 * 1024, "Codex cuts it at its 32 KiB budget");
}

#[test]
fn identical_harnesses_have_no_differences() {
    let fs = MemFs::new()
        .file("/w/a/CLAUDE.md", "same")
        .file(format!("{HOME}/.claude/agents/x.md",), "---\ndescription: x\n---\n");
    let h = resolve(&ClaudeCode, &fs, "/w/a");
    let c = compare(&h, &h);
    assert!(c.only_a.is_empty() && c.only_b.is_empty() && c.changed.is_empty() && c.settings.is_empty());
    assert!(c.kinds.values().all(|k| k.only_a == 0 && k.only_b == 0 && k.changed == 0));
}
