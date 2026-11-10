//! The Claude Code adapter on invented setups, one rule per test (docs/adapters/claude-code.md). Every output must
//! also keep the contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::ClaudeCode;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop";
const SYS: &str = "/etc/claude-code";

fn run_env(fs: &MemFs, env: &[(&str, &str)]) -> Harness {
    let ctx = Ctx {
        fs,
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(),
        home: PathBuf::from(HOME),
        repo: PathBuf::from(REPO),
        os: Os::Linux,
    };
    let h = ClaudeCode.resolve(&ctx);
    assert_eq!(contract_errors(&h), Vec::<String>::new(), "contract");
    h
}

fn run(fs: &MemFs) -> Harness {
    run_env(fs, &[])
}

fn base() -> MemFs {
    MemFs::new().dir(format!("{REPO}/.claude")).dir(format!("{HOME}/.claude"))
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

#[test]
fn model_env_beats_model_keys_and_managed_is_a_default() {
    let fs = base()
        .file(format!("{SYS}/managed-settings.json"), r#"{"model":"managed-model"}"#)
        .file(format!("{REPO}/.claude/settings.json"), r#"{"model":"project-model"}"#)
        .file(format!("{HOME}/.claude/settings.json"), r#"{"env":{"ANTHROPIC_MODEL":"user-env-model"}}"#);
    let h = run(&fs);
    let m = &h.settings["model"];
    assert_eq!(m.value, "user-env-model");
    assert_eq!(m.source.as_deref(), Some("user"));
    let layers: Vec<&str> = m.trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(layers, ["user", "managed", "project"]);

    let shell = run_env(
        &base().file(format!("{REPO}/.claude/settings.json"), r#"{"model":"p"}"#),
        &[("ANTHROPIC_MODEL", "shell-model")],
    );
    assert_eq!(shell.settings["model"].value, "shell-model");
    assert_eq!(shell.settings["model"].source.as_deref(), Some("session"));

    let none = run(&base());
    assert_eq!(none.settings["model"].value, serde_json::Value::Null);
    assert_eq!(none.settings["model"].source, None);
}

#[test]
fn managed_dropins_merge_after_the_main_file() {
    let fs = base()
        .file(format!("{SYS}/managed-settings.json"), r#"{"permissions":{"deny":["Bash(curl:*)"]},"model":"a"}"#)
        .file(
            format!("{SYS}/managed-settings.d/10-more.json"),
            r#"{"permissions":{"deny":["Bash(wget:*)","Bash(curl:*)"]},"model":"b"}"#,
        )
        .file(format!("{SYS}/managed-settings.d/.hidden.json"), r#"{"model":"hidden"}"#);
    let h = run(&fs);
    assert_eq!(h.settings["model"].value, "b");
    assert_eq!(
        ids(&h, "permission"),
        ["claude-code:permission:managed:deny Bash(curl:*)", "claude-code:permission:managed:deny Bash(wget:*)"]
    );
    assert!(h.layers.iter().find(|l| l.id == "managed").unwrap().present);
}

#[test]
fn permission_lists_combine_across_layers() {
    let fs = base()
        .file(
            format!("{REPO}/.claude/settings.json"),
            r#"{"permissions":{"allow":["Read"],"defaultMode":"acceptEdits"}}"#,
        )
        .file(
            format!("{HOME}/.claude/settings.json"),
            r#"{"permissions":{"allow":["Read","Bash(npm test)"],"ask":["Bash(git push:*)"],"defaultMode":"plan"}}"#,
        );
    let h = run(&fs);
    let rules = ids(&h, "permission");
    assert_eq!(rules.len(), 4);
    assert!(h.items.iter().filter(|i| i.detail.kind() == "permission").all(|i| i.effective));
    let p = item(&h, "claude-code:permission:user:allow Bash(npm test)");
    assert!(
        matches!(&p.detail, ItemDetail::Permission(a) if a.tool == "Bash" && a.pattern.as_deref() == Some("npm test"))
    );
    assert_eq!(h.settings["defaultMode"].value, "acceptEdits");
    assert_eq!(h.settings["defaultMode"].source.as_deref(), Some("project"));
}

#[test]
fn instruction_files_imports_and_rules() {
    let fs = base()
        .file(format!("{HOME}/.claude/CLAUDE.md"), "user rules @~/.claude/style.md")
        .file(format!("{HOME}/.claude/style.md"), "style")
        .file(format!("{HOME}/work/CLAUDE.md"), "parent folder")
        .file(format!("{REPO}/CLAUDE.md"), "@AGENTS.md\n@/etc/passwd\n`@not-this.md`")
        .file(format!("{REPO}/CLAUDE.local.md"), "mine")
        .file(format!("{REPO}/AGENTS.md"), "@a1.md")
        .file(format!("{REPO}/a1.md"), "@a2.md")
        .file(format!("{REPO}/a2.md"), "@a3.md")
        .file(format!("{REPO}/a3.md"), "@a4.md")
        .file(format!("{REPO}/a4.md"), "@a5.md")
        .file(format!("{REPO}/a5.md"), "too deep")
        .file("/etc/passwd", "root")
        .file(format!("{REPO}/.claude/rules/always.md"), "always")
        .file(format!("{REPO}/.claude/rules/api/scoped.md"), "---\npaths:\n  - src/api/**\n---\nscoped");
    let h = run(&fs);
    let names: Vec<&str> =
        h.items.iter().filter(|i| i.detail.kind() == "instruction").map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "~/.claude/CLAUDE.md",
            "~/.claude/style.md",
            "~/work/CLAUDE.md",
            "CLAUDE.md",
            "AGENTS.md",
            "a1.md",
            "a2.md",
            "a3.md",
            "CLAUDE.local.md",
            ".claude/rules/always.md",
            ".claude/rules/api/scoped.md"
        ]
    );
    assert!(h.items.iter().all(|i| i.name != "a4.md"), "AGENTS.md is hop 1, so a4.md is hop 5: past four hops");
    let a1 = item(&h, "claude-code:instruction:project:a1.md");
    assert!(
        matches!(&a1.detail, ItemDetail::Instruction(x) if x.imported_by.as_deref() == Some("claude-code:instruction:project:AGENTS.md"))
    );
    assert_eq!(item(&h, "claude-code:instruction:local:CLAUDE.local.md").origin.layer, "local");
    let scoped = item(&h, "claude-code:instruction:project:.claude/rules/api/scoped.md");
    assert_eq!(scoped.load.when, LoadWhen::OnMatch);
    assert!(matches!(&scoped.detail, ItemDetail::Instruction(x) if x.applies_to == ["src/api/**"]));
    assert!(h.warnings.iter().any(|w| w.code == "import-outside"));
    assert!(h.items.iter().all(|i| !i.name.contains("passwd")));
    // context: every effective startup item's characters, nothing else.
    let want: u64 =
        h.items.iter().filter(|i| i.effective && i.load.when == LoadWhen::Startup).map(|i| i.load.chars.value).sum();
    assert_eq!(h.context.chars.value, want);
    assert_eq!(
        item(&h, "claude-code:instruction:project:CLAUDE.md").load.chars.value,
        "@AGENTS.md\n@/etc/passwd\n`@not-this.md`".chars().count() as u64
    );
}

#[test]
fn agents_md_only_without_claude_md() {
    let fs = base().file(format!("{REPO}/AGENTS.md"), "shared");
    assert_eq!(ids(&run(&fs), "instruction"), ["claude-code:instruction:project:AGENTS.md"]);
    let with_local = fs.clone().file(format!("{REPO}/CLAUDE.local.md"), "x");
    assert_eq!(ids(&run(&with_local), "instruction"), ["claude-code:instruction:local:CLAUDE.local.md"]);
}

#[test]
fn project_agent_shadows_user_agent_and_all_tools_is_warned() {
    let fs = base()
        .file(format!("{HOME}/.claude/agents/reviewer.md"), "---\nname: reviewer\ndescription: user one\n---\n")
        .file(
            format!("{REPO}/.claude/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: project one\ntools: Read, Grep\nmodel: small\n---\n",
        )
        .file(format!("{HOME}/.claude/agents/helper.md"), "---\ndescription: |\n  First.\n\n  Second.\n---\n");
    let h = run(&fs);
    let user = item(&h, "claude-code:agent:user:reviewer");
    assert!(!user.effective);
    assert_eq!(user.shadowed_by.as_deref(), Some("claude-code:agent:project:reviewer"));
    let proj = item(&h, "claude-code:agent:project:reviewer");
    assert!(
        matches!(&proj.detail, ItemDetail::Agent(a) if a.tools == ToolAccess::Only(vec!["Read".into(), "Grep".into()]) && a.model.as_deref() == Some("small"))
    );
    assert_eq!(proj.load.chars.value, ("reviewer".len() + "project one".len() + "Read, Grep".len()) as u64);
    let helper = item(&h, "claude-code:agent:user:helper");
    assert_eq!(
        helper.load.chars.value,
        ("helper".len() + "First.\n\nSecond.".len()) as u64,
        "a multi-paragraph description is counted whole"
    );
    assert!(h.warnings.iter().any(|w| w.code == "agent-all-tools" && w.message.starts_with("1 agent ")));
}

#[test]
fn skills_user_beats_project_and_beat_commands() {
    let long = "x".repeat(2000);
    let fs = base()
        .file(format!("{HOME}/.claude/skills/deploy/SKILL.md"), "---\ndescription: user deploy\n---\nbody")
        .file(format!("{REPO}/.claude/skills/deploy/SKILL.md"), "---\ndescription: project deploy\n---\nbody")
        .file(
            format!("{REPO}/.claude/skills/long/SKILL.md"),
            format!("---\nname: long\ndescription: {long}\n---\nthe body"),
        )
        .file(format!("{REPO}/.claude/commands/deploy.md"), "---\ndescription: old command\n---\n")
        .file(format!("{REPO}/.claude/commands/standup.md"), "---\ndescription: daily\n---\n");
    let h = run(&fs);
    assert!(item(&h, "claude-code:skill:user:deploy").effective);
    assert_eq!(
        item(&h, "claude-code:skill:project:deploy").shadowed_by.as_deref(),
        Some("claude-code:skill:user:deploy")
    );
    assert_eq!(
        item(&h, "claude-code:command:project:deploy").shadowed_by.as_deref(),
        Some("claude-code:skill:user:deploy")
    );
    assert!(item(&h, "claude-code:command:project:standup").effective);
    let long_skill = item(&h, "claude-code:skill:project:long");
    assert_eq!(long_skill.load.chars.value, 4 + 1536, "the listing cuts the description at 1,536 characters");
    assert!(matches!(&long_skill.detail, ItemDetail::Skill(s) if s.body_chars == Some(8)));
}

#[test]
fn hooks_disable_all_follows_precedence_and_spares_managed() {
    let hooks = |cmd: &str| {
        format!(r#"{{"PreToolUse":[{{"matcher":"Bash","hooks":[{{"type":"command","command":"{cmd}"}}]}}]}}"#)
    };
    let fs =
        base().file(format!("{SYS}/managed-settings.json"), format!(r#"{{"hooks":{}}}"#, hooks("/opt/guard.sh"))).file(
            format!("{HOME}/.claude/settings.json"),
            format!(r#"{{"disableAllHooks":true,"hooks":{}}}"#, hooks("TOKEN=s3cret node ~/h/log.mjs --key abc")),
        );
    let h = run(&fs);
    assert!(
        item(&h, "claude-code:hook:managed:PreToolUse-1").effective,
        "a user disableAllHooks cannot turn off managed hooks"
    );
    let user = item(&h, "claude-code:hook:user:PreToolUse-1");
    assert!(!user.effective);
    assert!(
        matches!(&user.detail, ItemDetail::Hook(x) if x.script.as_deref() == Some("log.mjs") && x.event == "before-tool")
    );

    let back_on = fs.clone().file(format!("{REPO}/.claude/settings.json"), r#"{"disableAllHooks":false}"#);
    assert!(
        item(&run(&back_on), "claude-code:hook:user:PreToolUse-1").effective,
        "a project false overrides a user true"
    );

    let managed_off = fs.clone().file(format!("{SYS}/managed-settings.d/x.json"), r#"{"disableAllHooks":true}"#);
    assert!(!item(&run(&managed_off), "claude-code:hook:managed:PreToolUse-1").effective);
}

/// A directory marketplace with one plugin, enabled at user level unless a test says otherwise.
fn with_plugin(fs: MemFs) -> MemFs {
    let mk = format!("{HOME}/mk");
    fs.file(
        format!("{HOME}/.claude/plugins/known_marketplaces.json"),
        format!(r#"{{"mk":{{"source":{{"source":"directory","path":"{mk}"}}}}}}"#),
    )
    .file(format!("{mk}/.claude-plugin/marketplace.json"), r#"{"plugins":[{"name":"tools","source":"./plugins/tools"}]}"#)
    .file(format!("{mk}/plugins/tools/.claude-plugin/plugin.json"), r#"{"name":"tools","skills":["./extra"],"hooks":{"SessionStart":[{"hooks":[{"type":"http","url":"https://hooks.example.com/secret/path?k=1"}]}]}}"#)
    .file(format!("{mk}/plugins/tools/agents/fixer.md"), "---\ndescription: fixes\ntools: Edit\n---\n")
    .file(format!("{mk}/plugins/tools/agents/review/security.md"), "---\ndescription: audits\ntools: Read\n---\n")
    .symlink(format!("{mk}/plugins/tools/agents/leak.md"), format!("{HOME}/.ssh/id.md"))
    .file(format!("{HOME}/.ssh/id.md"), "---\ndescription: private\n---\n")
    .file(format!("{mk}/plugins/tools/skills/lint/SKILL.md"), "---\ndescription: lints\n---\n")
    .file(format!("{mk}/plugins/tools/extra/SKILL.md"), "---\nname: extra\ndescription: extra skill\n---\n")
    .file(format!("{mk}/plugins/tools/commands/ship.md"), "---\ndescription: ships\n---\n")
    .file(format!("{mk}/plugins/tools/.mcp.json"), r#"{"mcpServers":{"tickets":{"url":"https://t.example.com/mcp"}}}"#)
    .file(format!("{HOME}/.claude/settings.json"), r#"{"enabledPlugins":{"tools@mk":true}}"#)
}

#[test]
fn plugin_contributions_are_namespaced_confined_and_linked() {
    let h = run(&with_plugin(base()));
    let pkg = h.packages.iter().find(|p| p.id == "tools@mk").unwrap();
    assert!(pkg.enabled);
    assert_eq!(pkg.decided_by.as_deref(), Some("user"));
    let layer = "plugin:tools@mk";
    for id in [
        format!("claude-code:agent:{layer}:tools:fixer"),
        format!("claude-code:agent:{layer}:tools:review:security"),
        format!("claude-code:skill:{layer}:tools:lint"),
        format!("claude-code:skill:{layer}:tools:extra"),
        format!("claude-code:command:{layer}:tools:ship"),
        format!("claude-code:hook:{layer}:SessionStart-1"),
        format!("claude-code:mcp:{layer}:tickets"),
    ] {
        assert!(pkg.contributes.contains(&id), "{id} not in contributes {:?}", pkg.contributes);
        assert_eq!(item(&h, &id).origin.package.as_deref(), Some("tools@mk"));
    }
    assert!(h.items.iter().all(|i| !i.name.contains("leak")), "a symlink out of the plugin folder is dropped");
    assert!(h.warnings.iter().any(|w| w.code == "plugin-path-escape"));
    let hook = item(&h, &format!("claude-code:hook:{layer}:SessionStart-1"));
    assert!(matches!(&hook.detail, ItemDetail::Hook(x) if x.script.as_deref() == Some("https://hooks.example.com")));
}

#[test]
fn a_higher_layer_can_switch_a_plugin_off() {
    let fs = with_plugin(base())
        .file(format!("{REPO}/.claude/settings.local.json"), r#"{"enabledPlugins":{"tools@mk":false}}"#);
    let h = run(&fs);
    let pkg = h.packages.iter().find(|p| p.id == "tools@mk").unwrap();
    assert!(!pkg.enabled);
    assert_eq!(pkg.decided_by.as_deref(), Some("local"));
    assert!(pkg.contributes.is_empty());
    assert!(h.items.iter().all(|i| !i.origin.layer.starts_with("plugin:")));
}

#[test]
fn managed_hooks_only_spares_force_enabled_plugins() {
    let fs =
        with_plugin(base()).file(format!("{SYS}/managed-settings.json"), r#"{"allowManagedHooksOnly":true}"#).file(
            format!("{REPO}/.claude/settings.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"prompt","prompt":"x"}]}]}}"#,
        );
    let h = run(&fs);
    assert!(!item(&h, "claude-code:hook:project:Stop-1").effective);
    assert!(!item(&h, "claude-code:hook:plugin:tools@mk:SessionStart-1").effective);
    let forced = fs.clone().file(
        format!("{SYS}/managed-settings.json"),
        r#"{"allowManagedHooksOnly":true,"enabledPlugins":{"tools@mk":true}}"#,
    );
    assert!(item(&run(&forced), "claude-code:hook:plugin:tools@mk:SessionStart-1").effective);
}

#[test]
fn mcp_precedence_approval_and_policy() {
    let cj = format!(
        r#"{{"mcpServers":{{"db":{{"command":"user-db"}},"search":{{"url":"https://s.example.com/x"}}}},
            "projects":{{"{REPO}":{{"mcpServers":{{"db":{{"command":"local-db"}}}},"enabledMcpjsonServers":["docs"]}}}}}}"#
    );
    let fs = base()
        .file(format!("{HOME}/.claude.json"), cj)
        .file(
            format!("{REPO}/.mcp.json"),
            r#"{"mcpServers":{"db":{"command":"project-db"},"docs":{"type":"sse","url":"https://d.example.com"},"new":{"command":"npx","args":["-y","pkg"]},"off":{"command":"x"}}}"#,
        )
        .file(format!("{REPO}/.claude/settings.json"), r#"{"disabledMcpjsonServers":["off"],"deniedMcpServers":[{"serverUrl":"https://s.example.com/*"}]}"#);
    let h = run(&fs);
    let state = |id: &str| match &item(&h, id).detail {
        ItemDetail::Mcp(m) => m.state,
        _ => unreachable!(),
    };
    assert!(item(&h, "claude-code:mcp:local:db").effective);
    assert_eq!(item(&h, "claude-code:mcp:user:db").shadowed_by.as_deref(), Some("claude-code:mcp:local:db"));
    assert_eq!(state("claude-code:mcp:project:docs"), McpState::Active);
    assert_eq!(state("claude-code:mcp:project:new"), McpState::Pending);
    assert_eq!(state("claude-code:mcp:project:off"), McpState::Disabled);
    assert_eq!(state("claude-code:mcp:user:search"), McpState::Denied);
    assert!(h.warnings.iter().any(|w| w.code == "mcp-pending"));
    assert!(h.context.not_measured.iter().any(|n| n.what.contains("MCP")));

    let exclusive = fs
        .clone()
        .file(format!("{SYS}/managed-mcp.json"), r#"{"mcpServers":{"corp":{"url":"https://corp.example.com"}}}"#);
    let h2 = run(&exclusive);
    let live: Vec<&str> =
        h2.items.iter().filter(|i| i.detail.kind() == "mcp" && i.effective).map(|i| i.name.as_str()).collect();
    assert_eq!(live, ["corp"], "managed-mcp.json takes exclusive control");
}

#[test]
fn secrets_never_reach_the_output() {
    let canaries =
        ["CANARY-ENV", "CANARY-ARG", "CANARY-HEADER", "CANARY-PATH", "CANARY-QUERY", "CANARY-HOOK", "canary-user"];
    let fs = base()
        .file(
            format!("{REPO}/.mcp.json"),
            r#"{"mcpServers":{"a":{"command":"run --token CANARY-ARG","args":["CANARY-ARG"],"env":{"K":"CANARY-ENV"}},
                "b":{"url":"https://canary-user:pw@b.example.com/CANARY-PATH?q=CANARY-QUERY","headers":{"Authorization":"CANARY-HEADER"}}}}"#,
        )
        .file(
            format!("{REPO}/.claude/settings.json"),
            r#"{"env":{"SECRET":"CANARY-ENV"},"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"python3 -c 'print(\"CANARY-HOOK\")'"}]}]}}"#,
        );
    let json = serde_json::to_string(&run(&fs)).unwrap();
    for c in canaries {
        assert!(!json.contains(c), "{c} leaked into the output");
    }
}

#[test]
fn malformed_json_is_a_warning_that_quotes_nothing() {
    let fs = base().file(format!("{REPO}/.claude/settings.json"), r#"{"model": "CANARY-MALFORMED"#);
    let h = run(&fs);
    let w = h.warnings.iter().find(|w| w.code == "incomplete-malformed-json").expect("a warning");
    assert_eq!(w.file.as_deref(), Some(".claude/settings.json"));
    assert!(!serde_json::to_string(&h).unwrap().contains("CANARY-MALFORMED"));
}

#[test]
fn detection_and_config_dir() {
    let fs = MemFs::new()
        .file("/cfg/settings.json", r#"{"model":"from-cfg"}"#)
        .file("/cfg/.claude.json", r#"{"mcpServers":{"x":{"command":"x"}}}"#)
        .dir(REPO);
    let h = run_env(&fs, &[("CLAUDE_CONFIG_DIR", "/cfg")]);
    assert_eq!(h.settings["model"].value, "from-cfg");
    assert!(h.items.iter().any(|i| i.id == "claude-code:mcp:user:x"));
    assert!(h.tool.detected_by.contains(&"user config folder".to_string()));
    assert!(!h.coverage.not_modelled.is_empty());
}

/// `/home/dev/work/shop` as Claude Code names its project folder.
const MEMORY_DIR: &str = "/home/dev/.claude/projects/-home-dev-work-shop/memory";

#[test]
fn the_auto_memory_index_loads_its_first_200_lines() {
    let long: String = (0..250).map(|n| format!("- note {n}\n")).collect();
    let fs = base()
        .dir(format!("{REPO}/.git"))
        .file(format!("{MEMORY_DIR}/MEMORY.md"), long.clone())
        .file(format!("{MEMORY_DIR}/topic.md"), "loaded only when read");
    let h = run(&fs);
    let id = "claude-code:instruction:user:~/.claude/projects/-home-dev-work-shop/memory/MEMORY.md";
    let m = item(&h, id);
    assert_eq!(m.load.when, LoadWhen::Startup);
    let first: String = long.split_inclusive('\n').take(200).collect();
    assert_eq!(m.load.chars.value, first.chars().count() as u64);
    assert!(h.items.iter().all(|i| !i.name.ends_with("topic.md")), "topic files are not startup items");

    // Off by setting or by environment variable.
    let off = fs.clone().file(format!("{REPO}/.claude/settings.json"), r#"{"autoMemoryEnabled":false}"#);
    assert!(run(&off).items.iter().all(|i| !i.name.ends_with("MEMORY.md")));
    let env_off = run_env(&fs, &[("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")]);
    assert!(env_off.items.iter().all(|i| !i.name.ends_with("MEMORY.md")));
}

#[test]
fn a_worktree_shares_the_main_checkouts_memory_and_the_folder_can_move() {
    // The repo is a worktree of /home/dev/work/main.
    let fs = base()
        .file(format!("{REPO}/.git"), "gitdir: /home/dev/work/main/.git/worktrees/shop\n")
        .file("/home/dev/.claude/projects/-home-dev-work-main/memory/MEMORY.md", "- shared\n");
    let h = run(&fs);
    assert!(h.items.iter().any(|i| i.name == "~/.claude/projects/-home-dev-work-main/memory/MEMORY.md"));

    let moved = base()
        .file(format!("{HOME}/.claude/settings.json"), r#"{"autoMemoryDirectory":"~/notes/mem"}"#)
        .file(format!("{HOME}/notes/mem/MEMORY.md"), "- moved\n");
    assert!(run(&moved).items.iter().any(|i| i.name == "~/notes/mem/MEMORY.md" && i.effective));
}

#[test]
fn session_start_hooks_and_mcp_servers_are_named_as_unmeasured() {
    let fs = base()
        .file(
            format!("{HOME}/.claude/settings.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"brief.sh"}]}]}}"#,
        )
        .file(format!("{REPO}/.mcp.json"), r#"{"mcpServers":{"docs":{"command":"docs-server"}}}"#)
        .file(format!("{REPO}/.claude/settings.json"), r#"{"enableAllProjectMcpServers":true}"#);
    let h = run(&fs);
    let what: Vec<&str> = h.context.not_measured.iter().map(|n| n.what.as_str()).collect();
    assert!(what.iter().any(|w| w.contains("1 SessionStart hook")), "{what:?}");
    assert!(what.iter().any(|w| w.contains("MCP server (docs)")), "{what:?}");
}

fn agent_md(name: &str) -> String {
    format!("---\nname: {name}\ndescription: does {name} things\ntools: Read\n---\n")
}

#[test]
fn agent_folders_are_scanned_recursively_and_a_subfolder_does_not_change_the_name() {
    let fs = base()
        .file(format!("{REPO}/.claude/agents/review/security.md"), agent_md("security"))
        .file(format!("{REPO}/.claude/agents/deep/a/b/c/tester.md"), agent_md("tester"))
        .file(format!("{HOME}/.claude/agents/team/writer.md"), agent_md("writer"))
        // Two files in one tree with one name: one loads, which one is not documented.
        .file(format!("{REPO}/.claude/agents/twin.md"), agent_md("twin"))
        .file(format!("{REPO}/.claude/agents/more/twin.md"), agent_md("twin"));
    let h = run(&fs);
    for id in
        ["claude-code:agent:project:security", "claude-code:agent:project:tester", "claude-code:agent:user:writer"]
    {
        assert!(item(&h, id).effective, "{id}");
    }
    assert_eq!(
        item(&h, "claude-code:agent:project:twin#2").shadowed_by.as_deref(),
        Some("claude-code:agent:project:twin")
    );
    assert!(h.warnings.iter().any(|w| w.code == "incomplete-duplicate-name"));
}

#[test]
fn project_agents_come_from_every_folder_up_to_the_git_root_and_the_closest_wins() {
    // The session starts in /home/dev/work/shop; the git root is /home/dev/work.
    let fs = base()
        .dir("/home/dev/work/.git")
        .file("/home/dev/work/.claude/agents/shared.md", agent_md("shared"))
        .file("/home/dev/work/.claude/agents/reviewer.md", agent_md("reviewer"))
        .file(format!("{REPO}/.claude/agents/reviewer.md"), agent_md("reviewer"))
        // Above the git root: not a project folder.
        .file("/home/.claude/agents/stray.md", agent_md("stray"));
    let h = run(&fs);
    assert!(item(&h, "claude-code:agent:project:shared").effective);
    let near = item(&h, "claude-code:agent:project:reviewer");
    assert_eq!(near.origin.file.as_deref(), Some(".claude/agents/reviewer.md"));
    assert!(near.effective);
    assert_eq!(
        item(&h, "claude-code:agent:project:reviewer#2").shadowed_by.as_deref(),
        Some("claude-code:agent:project:reviewer")
    );
    assert!(
        !h.warnings.iter().any(|w| w.code == "incomplete-duplicate-name"),
        "an override is not a duplicate: {:?}",
        h.warnings
    );
    assert!(h.items.iter().all(|i| i.name != "stray"));
}

#[test]
fn skills_in_subfolders_load_when_claude_works_there_and_never_clash_with_the_roots() {
    let skill = |d: &str| format!("---\ndescription: {d}\n---\nbody\n");
    let fs = base()
        .file(format!("{REPO}/.claude/skills/deploy/SKILL.md"), skill("deploys the shop"))
        .file(format!("{REPO}/apps/web/.claude/skills/deploy/SKILL.md"), skill("deploys the web app"))
        .file(format!("{REPO}/node_modules/pkg/.claude/skills/evil/SKILL.md"), skill("never read"))
        .file(format!("{REPO}/.cache/x/.claude/skills/hidden/SKILL.md"), skill("never read"));
    let h = run(&fs);
    let root = item(&h, "claude-code:skill:project:deploy");
    assert_eq!(root.load.when, LoadWhen::Startup);
    let nested = item(&h, "claude-code:skill:project:apps/web:deploy");
    assert!(nested.effective);
    assert_eq!(nested.load.when, LoadWhen::OnMatch);
    assert!(h.items.iter().all(|i| !i.name.contains("evil") && !i.name.contains("hidden")));
    let startup: u64 =
        h.items.iter().filter(|i| i.effective && i.load.when == LoadWhen::Startup).map(|i| i.load.chars.value).sum();
    assert_eq!(h.context.chars.value, startup, "a nested skill is not counted before you type");
}

#[test]
fn claude_md_in_subfolders_loads_on_demand_and_links_are_not_followed() {
    let fs = base()
        .file(format!("{REPO}/CLAUDE.md"), "# Shop\n")
        .file(format!("{REPO}/packages/api/CLAUDE.md"), "# API rules\n@notes.md\n")
        .file(format!("{REPO}/packages/api/notes.md"), "more\n")
        .file(format!("{REPO}/packages/api/CLAUDE.local.md"), "mine\n")
        .file(format!("{REPO}/vendor/lib/CLAUDE.md"), "never read\n")
        .file("/home/dev/elsewhere/CLAUDE.md", "never read\n")
        .symlink(format!("{REPO}/linked"), "/home/dev/elsewhere");
    let h = run(&fs);
    let api = item(&h, "claude-code:instruction:project:packages/api/CLAUDE.md");
    assert_eq!(api.load.when, LoadWhen::OnMatch);
    assert!(matches!(&api.detail, ItemDetail::Instruction(a) if a.applies_to == ["packages/api/**"]));
    assert_eq!(
        item(&h, "claude-code:instruction:project:packages/api/notes.md").load.when,
        LoadWhen::OnMatch,
        "its import too"
    );
    assert_eq!(item(&h, "claude-code:instruction:local:packages/api/CLAUDE.local.md").load.when, LoadWhen::OnMatch);
    assert_eq!(item(&h, "claude-code:instruction:project:CLAUDE.md").load.when, LoadWhen::Startup);
    assert!(
        h.items.iter().all(|i| !i.name.contains("vendor") && !i.name.contains("linked")),
        "{:?}",
        ids(&h, "instruction")
    );
}
