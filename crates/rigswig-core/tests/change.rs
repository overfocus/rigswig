//! Planning changes (write mode): the right file, an exact after-budget, an honest "no effect", and refusals.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::{ClaudeCode, Codex};
use rigswig_core::change::{Change, Scope};
use rigswig_core::fs::MemFs;
use rigswig_core::model::Os;

const HOME: &str = "/home/dev";
const REPO: &str = "/w/app";

fn ctx(fs: &MemFs) -> Ctx<'_> {
    Ctx { fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux }
}

/// A user-enabled plugin with one skill, a project .mcp.json with one server waiting for approval.
fn setup() -> MemFs {
    let mk = format!("{HOME}/mk");
    MemFs::new()
        .file(
            format!("{HOME}/.claude/plugins/known_marketplaces.json"),
            format!(r#"{{"mk":{{"source":{{"source":"directory","path":"{mk}"}}}}}}"#),
        )
        .file(format!("{mk}/.claude-plugin/marketplace.json"), r#"{"plugins":[{"name":"fmt","source":"./fmt"}]}"#)
        .file(
            format!("{mk}/fmt/skills/pretty/SKILL.md"),
            format!("---\ndescription: {}\n---\n", "formats code ".repeat(40)),
        )
        .file(
            format!("{HOME}/.claude/settings.json"),
            "{\n  \"model\": \"big\",\n  \"enabledPlugins\": {\n    \"fmt@mk\": true\n  }\n}\n",
        )
        .file(format!("{REPO}/.mcp.json"), r#"{"mcpServers":{"docs":{"command":"docs-server"}}}"#)
        .dir(format!("{REPO}/.claude"))
}

fn plugin(scope: Scope, on: bool) -> Change {
    Change::Plugin { id: "fmt@mk".into(), scope, on }
}

#[test]
fn turning_a_plugin_off_locally_creates_the_local_file_and_lowers_the_budget() {
    let fs = setup();
    let p = ClaudeCode.plan(&ctx(&fs), &plugin(Scope::Local, false)).unwrap();
    assert_eq!(p.file, ".claude/settings.local.json");
    assert!(p.creates_file);
    assert_eq!(p.before_hash, None);
    assert_eq!(p.after, "{\n  \"enabledPlugins\": {\n    \"fmt@mk\": false\n  }\n}\n");
    assert!(p.tokens_after < p.tokens_before, "{} -> {}", p.tokens_before, p.tokens_after);
    assert!(p.takes_effect);
    assert!(p.diff.iter().all(|l| l.op == '+'));
    assert!(p.notes[0].contains("next Claude Code session"));
}

#[test]
fn editing_an_existing_file_keeps_its_other_keys_in_order() {
    let fs = setup();
    let p = ClaudeCode.plan(&ctx(&fs), &plugin(Scope::User, false)).unwrap();
    assert_eq!(p.after, "{\n  \"model\": \"big\",\n  \"enabledPlugins\": {\n    \"fmt@mk\": false\n  }\n}\n");
    assert!(p.before_hash.is_some());
    let changed: Vec<_> = p.diff.iter().filter(|l| l.op != ' ').map(|l| (l.op, l.text.trim())).collect();
    assert_eq!(changed, [('-', "\"fmt@mk\": true"), ('+', "\"fmt@mk\": false")]);
    assert!(p.notes.iter().any(|n| n.contains("every repo")));
}

#[test]
fn a_change_a_higher_layer_overrides_says_it_has_no_effect() {
    let fs = setup().file(format!("{REPO}/.claude/settings.local.json"), r#"{"enabledPlugins":{"fmt@mk":true}}"#);
    let p = ClaudeCode.plan(&ctx(&fs), &plugin(Scope::User, false)).unwrap();
    assert!(!p.takes_effect);
    assert_eq!(p.tokens_after, p.tokens_before);
    assert!(p.notes.iter().any(|n| n.starts_with("No effect: local")));
}

#[test]
fn project_mcp_servers_can_be_approved_and_disabled() {
    let fs = setup();
    let on =
        ClaudeCode.plan(&ctx(&fs), &Change::McpJson { name: "docs".into(), scope: Scope::Local, on: true }).unwrap();
    assert!(on.takes_effect);
    assert!(on.after.contains("\"enabledMcpjsonServers\": [\n    \"docs\"\n  ]"));
    let fs2 = fs
        .clone()
        .file(format!("{REPO}/.claude/settings.local.json"), "{\n  \"enabledMcpjsonServers\": [\"docs\"]\n}\n");
    let off =
        ClaudeCode.plan(&ctx(&fs2), &Change::McpJson { name: "docs".into(), scope: Scope::Local, on: false }).unwrap();
    assert!(off.takes_effect);
    assert!(off.after.contains("disabledMcpjsonServers") && !off.after.contains("enabledMcpjsonServers"));
    let project_note =
        ClaudeCode.plan(&ctx(&fs), &Change::McpJson { name: "docs".into(), scope: Scope::Project, on: true }).unwrap();
    assert!(project_note.notes.iter().any(|n| n.contains("for everyone")));
}

#[test]
fn refusals() {
    let fs = setup();
    let c = ctx(&fs);
    let err = |fs: &MemFs, ch: Change| ClaudeCode.plan(&ctx(fs), &ch).unwrap_err();
    assert!(
        ClaudeCode
            .plan(&c, &Change::Plugin { id: "ghost@mk".into(), scope: Scope::Local, on: false })
            .unwrap_err()
            .contains("no plugin")
    );
    assert!(
        ClaudeCode
            .plan(&c, &Change::Plugin { id: "nomarket".into(), scope: Scope::Local, on: false })
            .unwrap_err()
            .contains("name@marketplace")
    );
    assert!(
        ClaudeCode
            .plan(&c, &Change::McpJson { name: "ghost".into(), scope: Scope::Local, on: true })
            .unwrap_err()
            .contains(".mcp.json")
    );
    assert!(ClaudeCode.plan(&c, &plugin(Scope::User, true)).unwrap_err().contains("already"));

    let managed = setup().file("/etc/claude-code/managed-settings.json", r#"{"enabledPlugins":{"fmt@mk":true}}"#);
    assert!(err(&managed, plugin(Scope::Local, false)).contains("managed settings decide"));
    let bad = setup().file(format!("{REPO}/.claude/settings.local.json"), "{ not json");
    assert!(err(&bad, plugin(Scope::Local, false)).contains("not valid JSON"));
    let link = setup()
        .file("/elsewhere/x.json", "{}")
        .symlink(format!("{REPO}/.claude/settings.local.json"), "/elsewhere/x.json");
    assert!(err(&link, plugin(Scope::Local, false)).contains("link"));

    assert!(Codex.plan(&c, &plugin(Scope::Local, false)).unwrap_err().contains("cannot change codex"));
}
