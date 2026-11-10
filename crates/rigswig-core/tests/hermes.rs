//! The Hermes adapter on invented setups, one rule per test (docs/adapters/hermes.md). Every output keeps the
//! contract.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::Hermes;
use rigswig_core::check::contract_errors;
use rigswig_core::fs::MemFs;
use rigswig_core::model::*;

const HOME: &str = "/home/dev";
const ROOT: &str = "/home/dev/work/shop";
const REPO: &str = "/home/dev/work/shop/app";
const HH: &str = "/home/dev/.hermes";
const MANAGED: &str = "/etc/hermes";

fn run_env(env: &[(&str, &str)], fs: &MemFs) -> Harness {
    let ctx = Ctx {
        fs,
        env: env.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect::<BTreeMap<_, _>>(),
        home: PathBuf::from(HOME),
        repo: PathBuf::from(REPO),
        os: Os::Linux,
    };
    let h = Hermes.resolve(&ctx);
    assert_eq!(contract_errors(&h), Vec::<String>::new(), "contract");
    h
}

fn run(fs: &MemFs) -> Harness {
    run_env(&[], fs)
}

/// A git project (`shop/` is the root, `app/` the working folder) with an empty Hermes home.
fn base() -> MemFs {
    MemFs::new().dir(format!("{ROOT}/.git")).dir(REPO).dir(HH)
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

fn user_config(yaml: &str) -> MemFs {
    base().file(format!("{HH}/config.yaml"), yaml)
}

#[test]
fn the_managed_scope_pins_a_key_over_everything() {
    let fs = base()
        .file(
            format!("{HH}/config.yaml"),
            "model:\n  default: user-model\n  provider: nous\n  base_url: https://user:pw@relay.example.com:8443/v1\napprovals:\n  mode: manual\n",
        )
        .file(format!("{MANAGED}/config.yaml"), "model:\n  default: org-model\ncontext_file_max_chars: 5000\n");
    let h = run_env(&[("HERMES_INFERENCE_MODEL", "env-model")], &fs);
    let model = &h.settings["model"];
    assert_eq!(model.value, "org-model", "the managed scope wins over the environment and the user config");
    assert_eq!(model.source.as_deref(), Some("managed"));
    let trail: Vec<&str> = model.trail.iter().map(|t| t.layer.as_str()).collect();
    assert_eq!(trail, ["managed", "session", "user"]);
    assert_eq!(h.settings["provider"].value, "nous");
    assert_eq!(h.settings["approvalMode"].value, "manual");
    assert!(
        !h.settings.contains_key("contextLength"),
        "nothing sets the context length, so the default is unexplained"
    );
    assert_eq!(h.settings["modelBaseUrl"].value, "https://relay.example.com", "no user info, port or path");
    // The pin moves the cap, visible as a setting too.
    assert_eq!(h.settings["contextFileMaxChars"].value, 5000);

    let user_only = run(&user_config("model:\n  default: user-model\n"));
    assert_eq!(user_only.settings["model"].value, "user-model");
    assert_eq!(user_only.settings["model"].source.as_deref(), Some("user"));
}

#[test]
fn ollama_launch_models_carry_their_execution_not_just_their_local_endpoint() {
    for (model, execution) in
        [("gemma4:31b", "local"), ("kimi-k3.1-pro:cloud", "cloud"), ("gpt-oss:120b-cloud", "cloud")]
    {
        let fs = user_config(&format!(
            "model:\n  default: {model}\n  provider: ollama-launch\n  base_url: http://127.0.0.1:11434/v1\n"
        ));
        let h = run(&fs);
        assert_eq!(h.settings["provider"].value, "ollama-launch");
        assert_eq!(h.settings["modelBaseUrl"].value, "http://127.0.0.1");
        assert_eq!(h.settings["modelExecution"].value, execution);
        assert_eq!(h.settings["modelExecution"].source.as_deref(), Some("user"));
    }

    let custom = run(&user_config(
        "model:\n  default: kimi-k3.1-pro:cloud\n  provider: custom\n  base_url: http://127.0.0.1:11434/v1\n",
    ));
    assert!(!custom.settings.contains_key("modelExecution"), "a custom endpoint does not prove Ollama Launch");
}

#[test]
fn the_environment_overrides_the_user_config_without_a_managed_pin() {
    let fs = user_config("model:\n  default: user-model\n");
    let h = run_env(&[("HERMES_MODEL", "shell-model")], &fs);
    assert_eq!(h.settings["model"].value, "shell-model");
    assert_eq!(h.settings["model"].source.as_deref(), Some("session"));
    assert_eq!(h.settings["model"].trail.len(), 2, "both the environment and the user config had a say");
}

#[test]
fn only_one_project_context_type_loads() {
    let fs = user_config("")
        .file(format!("{ROOT}/.hermes.md"), "parent rules")
        .file(format!("{REPO}/AGENTS.md"), "agent rules")
        .file(format!("{REPO}/CLAUDE.md"), "claude rules")
        .file(format!("{REPO}/.cursorrules"), "cursor rules");
    let h = run(&fs);
    let winner = "hermes:instruction:project:~/work/shop/.hermes.md";
    assert!(ids(&h, "instruction").contains(&winner.to_string()));
    let agents = item(&h, "hermes:instruction:project:AGENTS.md");
    assert!(!agents.effective, "AGENTS.md loads only when .hermes.md is absent everywhere");
    assert_eq!(agents.shadowed_by.as_deref(), Some(winner));
    assert!(!item(&h, "hermes:instruction:project:CLAUDE.md").effective);
    assert!(!item(&h, "hermes:instruction:project:.cursorrules").effective);
    assert_eq!(h.context.chars.value, 12, "only the winner's text loads");

    let fallback = run(&user_config("").file(format!("{REPO}/AGENTS.md"), "agent rules"));
    let agents = item(&fallback, "hermes:instruction:project:AGENTS.md");
    assert!(agents.effective);
    assert_eq!(fallback.context.chars.value, 11);
}

#[test]
fn context_files_are_capped_at_the_configured_few_thousand_chars() {
    let long = "x".repeat(25_000);
    let fs = user_config("").file(format!("{REPO}/.hermes.md"), long.clone());
    let h = run(&fs);
    assert_eq!(h.context.chars.value, 20_000, "the dynamic cap's floor is modelled");
    assert!(h.warnings.iter().any(|w| w.code == "context-file-cap" && w.message.contains("20000")));

    let pinned = run(&user_config("context_file_max_chars: 50\n").file(format!("{REPO}/.hermes.md"), long));
    assert_eq!(pinned.context.chars.value, 50);
}

#[test]
fn nested_agents_md_load_on_match_only_when_their_type_wins() {
    let fs = user_config("")
        .file(format!("{REPO}/AGENTS.md"), "root rules")
        .file(format!("{REPO}/pkg/AGENTS.md"), "pkg rules");
    let h = run(&fs);
    let nested = h
        .items
        .iter()
        .find(|i| i.name == "pkg/AGENTS.md")
        .unwrap_or_else(|| panic!("the nested AGENTS.md is modelled"));
    assert!(nested.effective);
    assert_eq!(nested.load.when, LoadWhen::OnMatch);
    match &nested.detail {
        ItemDetail::Instruction(a) => assert_eq!(a.applies_to, vec!["pkg/**".to_string()]),
        other => panic!("not an instruction: {other:?}"),
    }

    let other_type = run(&user_config("")
        .file(format!("{REPO}/.hermes.md"), "hermes rules")
        .file(format!("{REPO}/pkg/AGENTS.md"), "pkg rules"));
    assert!(
        !other_type.items.iter().any(|i| i.name == "pkg/AGENTS.md"),
        "the nested copy belongs to a type that lost; it never loads here"
    );
}

#[test]
fn soul_memory_and_their_switches() {
    let fs = user_config("memory:\n  memory_char_limit: 20\n")
        .file(format!("{HH}/SOUL.md"), "the agent's soul")
        .file(format!("{HH}/memories/MEMORY.md"), "memory notes")
        .file(format!("{HH}/memories/USER.md"), "profile facts");
    let h = run(&fs);
    let soul = item(&h, "hermes:instruction:user:SOUL.md");
    assert!(soul.effective && soul.origin.layer == "user");
    assert_eq!(soul.load.chars.value, 16);
    assert_eq!(item(&h, "hermes:instruction:user:MEMORY.md").load.chars.value, 12);
    assert_eq!(item(&h, "hermes:instruction:user:USER.md").load.chars.value, 13);
    assert_eq!(h.context.chars.value, 41);

    let big = run(
        &user_config("memory:\n  memory_char_limit: 20\n").file(format!("{HH}/memories/MEMORY.md"), "m".repeat(30))
    );
    assert!(big.warnings.iter().any(|w| w.code == "memory-over-limit" && w.message.contains("MEMORY.md")));

    let off = run(&user_config("memory:\n  memory_enabled: false\n  user_profile_enabled: false\n")
        .file(format!("{HH}/SOUL.md"), "soul"));
    assert_eq!(ids(&off, "instruction"), ["hermes:instruction:user:SOUL.md".to_string()]);

    let provider = run(&user_config("memory:\n  provider: honcho\n").file(format!("{HH}/memories/MEMORY.md"), "notes"));
    assert!(
        ids(&provider, "instruction").is_empty(),
        "an external memory provider replaces the on-disk files in the model"
    );
    assert!(provider.coverage.not_modelled.iter().any(|n| n.contains("honcho")));
}

#[test]
fn skills_by_tier_with_project_trust_platform_and_auto_load() {
    let fs = user_config("skills:\n  trusted_project_dirs:\n    - ~/work/shop\n")
        .file(
            format!("{HH}/skills/devops/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Ships it\n---\nlocal body",
        )
        .file(
            format!("{ROOT}/.hermes/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Repo version\n---\nrepo body",
        );
    let h = run(&fs);
    let project = item(&h, "hermes:skill:project:deploy");
    assert!(project.effective, "a trusted repo's skill wins its name for sessions inside the repo");
    assert_eq!(project.load.chars.value, 6 + 12, "the listing is the name and description");
    let user = item(&h, "hermes:skill:user:deploy");
    assert_eq!(user.shadowed_by.as_deref(), Some("hermes:skill:project:deploy"));
    assert!(h.warnings.iter().any(|w| w.code == "skill-shadowed"));

    let untrusted =
        run(&user_config("").file(format!("{ROOT}/.hermes/skills/deploy/SKILL.md"), "---\nname: deploy\n---\nbody"));
    assert!(ids(&untrusted, "skill").is_empty(), "an untrusted repo's project skills are not loaded");
    assert!(untrusted.warnings.iter().any(|w| w.code == "project-skills-untrusted"));

    let platform = run(&user_config("").file(
        format!("{HH}/skills/os/imsg/SKILL.md"),
        "---\nname: imsg\ndescription: Apple only\nplatforms: [macos]\n---\nbody",
    ));
    assert!(!item(&platform, "hermes:skill:user:imsg").effective, "the skill hides on Linux");

    let pinned = run(&user_config("skills:\n  auto_load:\n    - deploy\n")
        .file(format!("{HH}/skills/deploy/SKILL.md"), "---\nname: deploy\ndescription: Ships it\n---\nlong body text"));
    let deploy = item(&pinned, "hermes:skill:user:deploy");
    assert_eq!(deploy.load.chars.value, 6 + 8 + 14, "the pinned skill's body loads too");

    let missing = run(&user_config("skills:\n  auto_load:\n    - ghost\n")
        .file(format!("{HH}/skills/deploy/SKILL.md"), "---\nname: deploy\n---\n"));
    assert!(missing.warnings.iter().any(|w| w.code == "auto-load-missing"));
}

#[test]
fn same_named_skills_in_one_tier_are_listed_by_their_relative_path() {
    let fs = user_config("")
        .file(format!("{HH}/skills/a/one/SKILL.md"), "---\nname: one\ndescription: A\n---\n")
        .file(format!("{HH}/skills/b/two/SKILL.md"), "---\nname: one\ndescription: B\n---\n");
    let h = run(&fs);
    assert!(h.items.iter().any(|i| i.name == "a/one" && i.effective));
    assert!(h.items.iter().any(|i| i.name == "b/two" && i.effective), "neither is guessed between");
    assert!(h.warnings.iter().any(|w| w.code == "incomplete-ambiguous-skill"));
}

#[test]
fn folder_plugins_run_unless_disabled_and_declare_hooks() {
    let manifest = "name: hello-world\nversion: \"1.0\"\ndescription: A minimal example plugin\nprovides_hooks:\n  - post_tool_call\n";
    let enabled = run(&user_config("").file(format!("{HH}/plugins/hello-world/plugin.yaml"), manifest));
    let pkg = enabled.packages.iter().find(|p| p.id == "hello-world").expect("the plugin is listed");
    assert!(pkg.enabled);
    assert!(enabled.layers.iter().any(|l| l.id == "plugin:hello-world"));
    let hook = enabled.items.iter().find(|i| i.detail.kind() == "hook").expect("the hook is counted");
    match &hook.detail {
        ItemDetail::Hook(a) => {
            assert_eq!(a.native_event, "post_tool_call");
            assert_eq!(a.event, "after-tool");
        }
        other => panic!("not a hook: {other:?}"),
    }
    assert!(
        enabled.context.not_measured.iter().any(|n| n.what.contains("hello-world") || n.why.contains("plugin")),
        "the plugin's registered tools are unmeasured"
    );

    let disabled = run(&user_config("plugins:\n  disabled:\n    - hello-world\n")
        .file(format!("{HH}/plugins/hello-world/plugin.yaml"), manifest));
    assert!(!disabled.packages.iter().find(|p| p.id == "hello-world").expect("listed").enabled);
    assert!(!disabled.items.iter().any(|i| i.detail.kind() == "hook"));

    let unlisted = run(&user_config("plugins:\n  enabled:\n    - other\n")
        .file(format!("{HH}/plugins/hello-world/plugin.yaml"), manifest));
    assert!(unlisted.packages.iter().find(|p| p.id == "hello-world").expect("listed").enabled);
    assert!(unlisted.warnings.iter().any(|w| w.code == "plugin-not-listed"));
}

#[test]
fn mcp_servers_with_transport_state_and_a_managed_pin() {
    let fs = user_config(
        "mcp_servers:\n  time:\n    command: uvx\n    args: [mcp-server-time]\n  legacy:\n    url: 'https://mcp.example.com/v1'\n    transport: sse\n    enabled: false\n",
    );
    let h = run(&fs);
    let time = item(&h, "hermes:mcp:user:time");
    match &time.detail {
        ItemDetail::Mcp(a) => {
            assert_eq!(a.transport, McpTransport::Stdio);
            assert_eq!(a.target.as_deref(), Some("uvx"));
            assert_eq!(a.state, McpState::Active);
        }
        other => panic!("not an mcp item: {other:?}"),
    }
    let legacy = item(&h, "hermes:mcp:user:legacy");
    match &legacy.detail {
        ItemDetail::Mcp(a) => {
            assert_eq!(a.transport, McpTransport::Sse);
            assert_eq!(a.state, McpState::Disabled);
        }
        other => panic!("not an mcp item: {other:?}"),
    }
    assert!(!legacy.effective, "an enabled: false server is not measured");

    let pinned = run(&fs.clone().file(format!("{MANAGED}/config.yaml"), "mcp_servers:\n  time:\n    command: npx\n"));
    let winner = item(&pinned, "hermes:mcp:managed:time");
    match &winner.detail {
        ItemDetail::Mcp(a) => assert_eq!(a.target.as_deref(), Some("npx")),
        other => panic!("not an mcp item: {other:?}"),
    }
    assert!(!item(&pinned, "hermes:mcp:user:time").effective, "the managed server wins the name");
}

#[test]
fn a_profile_is_its_own_hermes_home() {
    let fs = user_config("model:\n  default: main-model\n")
        .file(format!("{HH}/profiles/coder/config.yaml"), "model:\n  default: coder-model\n")
        .file(format!("{HH}/profiles/coder/SOUL.md"), "coder soul");
    let h = run_env(&[("HERMES_PROFILE", "coder")], &fs);
    assert_eq!(h.settings["model"].value, "coder-model", "the profile's config is the active one");
    assert!(item(&h, "hermes:instruction:user:SOUL.md").load.chars.value == 10);

    let unresolved = run_env(&[("HERMES_PROFILE", "ghost")], &fs);
    assert_eq!(unresolved.settings["model"].value, "main-model");
    assert!(unresolved.warnings.iter().any(|w| w.code == "incomplete-profile-unresolved"));
}

#[test]
fn detection_sees_the_home_and_the_project_side() {
    let env = BTreeMap::new();
    let score_of = |fs: &MemFs| {
        let ctx = Ctx { fs, env: env.clone(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
        Hermes.detect(&ctx).score
    };
    let empty = MemFs::new();
    assert_eq!(score_of(&empty), 0);
    let with_cfg = user_config("");
    assert_eq!(score_of(&with_cfg), 50, "the home folder alone stays at 50");
    let in_repo = user_config("").file(format!("{REPO}/.hermes.md"), "# repo context");
    assert_eq!(score_of(&in_repo), 100, "the repo's own Hermes file");
    assert!(
        Hermes
            .detect(&Ctx {
                fs: &with_cfg,
                env: BTreeMap::new(),
                home: PathBuf::from(HOME),
                repo: PathBuf::from(REPO),
                os: Os::Linux,
            })
            .evidence
            .contains(&"config.yaml".to_string())
    );
}
