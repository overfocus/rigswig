//! `rigswig audit` on invented setups: one kind of finding per test, plus fleet mode.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::{ClaudeCode, Dsh};
use rigswig_core::audit::{self, Audit, Finding, Options, Recommendation, Severity, Signal};
use rigswig_core::fs::MemFs;
use rigswig_core::model::Os;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop";

fn ctx(fs: &MemFs) -> Ctx<'_> {
    Ctx { fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux }
}

fn run_with(fs: &MemFs, opts: &Options) -> Audit {
    let c = ctx(fs);
    let h = ClaudeCode.resolve(&c);
    let a = audit::audit(&c, &h, opts);
    for f in &a.findings {
        let all = format!("{} {} {}", f.title, f.evidence.join(" "), f.proposal).to_lowercase();
        assert!(!all.contains("unused"), "findings never claim usage: {all}");
    }
    a
}

fn run(fs: &MemFs) -> Audit {
    run_with(fs, &Options::default())
}

/// `rigswig audit --tool dsh` on the same harness pick a dsh-only machine gets.
fn run_dsh(fs: &MemFs) -> Audit {
    let c = ctx(fs);
    let h = Dsh.resolve(&c);
    let a = audit::audit(&c, &h, &Options::default());
    for f in &a.findings {
        let all = format!("{} {} {}", f.title, f.evidence.join(" "), f.proposal).to_lowercase();
        assert!(!all.contains("unused"), "findings never claim usage: {all}");
    }
    a
}

fn base() -> MemFs {
    MemFs::new().dir(format!("{REPO}/.claude")).dir(format!("{HOME}/.claude"))
}

fn find<'a>(a: &'a Audit, code: &str) -> Vec<&'a Finding> {
    a.findings.iter().filter(|f| f.code == code).collect()
}

/// An agent whose listing costs about 200 tokens.
fn agent(name: &str) -> String {
    format!("---\nname: {name}\ndescription: {}\ntools: Read\n---\nbody\n", "Reviews things carefully. ".repeat(30))
}

#[test]
fn user_scope_agents_nothing_references_are_one_finding_with_a_saving() {
    let fs = base()
        .file(format!("{HOME}/.claude/agents/seo-specialist.md"), agent("SEO Specialist"))
        .file(format!("{HOME}/.claude/agents/resume-tailor.md"), agent("Resume Tailor"))
        .file(format!("{HOME}/.claude/agents/prompt-engineer.md"), agent("Prompt Engineer"))
        .file(format!("{HOME}/.claude/agents/code-reviewer.md"), agent("Code Reviewer"))
        .file(format!("{REPO}/docs/review.md"), "Hand the diff to the code-reviewer agent.\n");
    let a = run(&fs);
    let f = find(&a, "heavy-user-scope");
    assert_eq!(f.len(), 1, "{:#?}", a.findings);
    let f = f[0];
    assert_eq!(f.items.len(), 3, "the referenced agent is left out");
    assert!(f.items.iter().all(|i| !i.contains("Code Reviewer")));
    assert!(f.evidence.iter().any(|e| e.contains("referenced here: Code Reviewer")));
    let s = f.saving.as_ref().expect("a saving");
    assert!(s.tokens > 400, "{}", s.tokens);
    assert_eq!(s.without.len(), 3);
    let reviewer = a.user_scope.iter().find(|u| u.name == "Code Reviewer").unwrap();
    assert_eq!(reviewer.needed, Some(true));

    // Two unreferenced agents are below the bar.
    let small = base()
        .file(format!("{HOME}/.claude/agents/a.md"), agent("Alpha One"))
        .file(format!("{HOME}/.claude/agents/b.md"), agent("Beta Two"));
    assert!(find(&run(&small), "heavy-user-scope").is_empty());
}

fn with_user_plugin(fs: MemFs, name: &str) -> MemFs {
    let mk = format!("{HOME}/mk");
    fs.file(
        format!("{HOME}/.claude/plugins/known_marketplaces.json"),
        format!(r#"{{"mk":{{"source":{{"source":"directory","path":"{mk}"}}}}}}"#),
    )
    .file(
        format!("{mk}/.claude-plugin/marketplace.json"),
        format!(r#"{{"plugins":[{{"name":"{name}","source":"./plugins/{name}"}}]}}"#),
    )
    .file(format!("{mk}/plugins/{name}/.claude-plugin/plugin.json"), format!(r#"{{"name":"{name}"}}"#))
    .file(format!("{mk}/plugins/{name}/skills/check/SKILL.md"), "---\ndescription: checks the code\n---\n")
    .file(format!("{HOME}/.claude/settings.json"), format!(r#"{{"enabledPlugins":{{"{name}@mk":true}}}}"#))
}

#[test]
fn a_user_scope_plugin_without_its_signal_files_is_a_finding() {
    let fs = with_user_plugin(base(), "rust-analyzer-lsp");
    let a = run(&fs);
    let f = find(&a, "plugin-fit");
    assert_eq!(f.len(), 1, "{:#?}", a.findings);
    assert!(f[0].evidence[0].contains("Cargo.toml"));
    assert!(f[0].proposal.contains("rust-analyzer-lsp@mk"));
    assert!(f[0].saving.as_ref().is_some_and(|s| s.without == ["rust-analyzer-lsp@mk"]));

    // A Rust repo fits; so does a .rs file one folder down.
    assert!(find(&run(&fs.clone().file(format!("{REPO}/Cargo.toml"), "")), "plugin-fit").is_empty());
    assert!(find(&run(&fs.clone().file(format!("{REPO}/src/main.rs"), "")), "plugin-fit").is_empty());

    // Turned on by the project itself: the user chose it for this repo.
    let project =
        fs.clone().file(format!("{REPO}/.claude/settings.json"), r#"{"enabledPlugins":{"rust-analyzer-lsp@mk":true}}"#);
    let pa = run(&project);
    assert!(find(&pa, "plugin-fit").is_empty(), "{:#?}", pa.findings);
}

#[test]
fn the_users_signals_judge_plugins_the_table_does_not_know() {
    let fs = with_user_plugin(base(), "deploybot");
    let a = run(&fs);
    assert!(find(&a, "plugin-fit").is_empty());
    assert!(a.notes.iter().any(|n| n.contains("no signal")));
    let row = a.user_scope.iter().find(|u| u.kind == "plugin").unwrap();
    assert_eq!(row.needed, None);

    let opts = Options { signals: vec![Signal { plugin: vec!["deploy*".into()], needs: vec!["deploy.yaml".into()] }] };
    assert_eq!(find(&run_with(&fs, &opts), "plugin-fit").len(), 1);

    // Read from ~/.rigswig/signals.json; a broken file is a note, not a failure.
    let file = fs.clone().file(
        format!("{HOME}/.rigswig/signals.json"),
        r#"{"signals":[{"plugin":["deploy*"],"needs":["deploy.yaml"]}]}"#,
    );
    assert_eq!(audit::user_signals(&ctx(&file)).0.len(), 1);
    let broken = fs.clone().file(format!("{HOME}/.rigswig/signals.json"), "{");
    assert!(audit::user_signals(&ctx(&broken)).1.is_some());
}

#[test]
fn an_instruction_file_over_200_lines_lists_its_largest_sections() {
    let mut text = String::from("# Shop\n\nIntro.\n\n## Deploying\n");
    text.push_str(&"Run the deploy script and watch it.\n".repeat(150));
    text.push_str("## Style\n");
    text.push_str(&"Short.\n".repeat(60));
    let fs = base().file(format!("{REPO}/CLAUDE.md"), text);
    let a = run(&fs);
    let f = find(&a, "instruction-budget");
    assert_eq!(f.len(), 1);
    assert!(f[0].title.contains("CLAUDE.md is 216 lines"), "{}", f[0].title);
    assert!(f[0].evidence[1].contains("Deploying"), "the largest section first: {:?}", f[0].evidence);
    assert!(f[0].saving.is_none(), "instruction files are never rewritten, so no saving is claimed");

    assert!(find(&run(&base().file(format!("{REPO}/CLAUDE.md"), "# Short\n")), "instruction-budget").is_empty());
}

#[test]
fn sentences_in_both_the_user_and_the_project_file_are_repeated_text() {
    let rule = "Never push to the main branch without the owner saying yes first.\n";
    let other = "Always run the full test suite before you open any pull request at all.\n";
    let fs = base().file(format!("{HOME}/.claude/CLAUDE.md"), format!("# Everywhere\n- {rule}- {other}")).file(
        format!("{REPO}/CLAUDE.md"),
        format!("# Shop\n{rule}\n**{other}**\nOnly here: use the shop's own ports.\n"),
    );
    let a = run(&fs);
    let f = find(&a, "repeated-text");
    assert_eq!(f.len(), 1, "{:#?}", a.findings);
    assert!(f[0].title.starts_with("2 sentences"), "{}", f[0].title);
    assert!(f[0].title.contains("(user)") && f[0].title.contains("(project)"));
}

#[test]
fn dead_references_are_paths_and_scripts_the_repo_does_not_have() {
    let doc = "\
See `docs/deploy.md` and `docs/gone.md:12`, and `src/` for code.
The kit lives at `example/agent-kit` (another repo), the build in `.next/cache/x`.
A placeholder like `data/<series>.ts` is not a path.
```sh
npm run build
npm run vanished -- --all
make test
make gone
```
Make sure you run `make lint` too.
1. Create the parser in `docs/parsers/my_parser.md`, then wire it up.
2. Add its spec in `docs/specs/my_parser_spec.md` and an `docs/example.md`.
The simplest way is probably a small script `docs/neurons/model.mjs` the runner spawns.
The credit is copied from the site; see that repo's `docs/CREDIT.md`.
Shared files are listed in the website repo's
`docs/shared-files.json`; run `npm run check:siblings` there, or `npm run vendor -- ../shop` in another.
";
    let fs = base()
        .file(format!("{REPO}/CLAUDE.md"), doc)
        .file(format!("{REPO}/docs/deploy.md"), "x")
        .dir("/home/dev/work/website")
        .dir(format!("{REPO}/src"))
        .dir(format!("{REPO}/.next"))
        .file(format!("{REPO}/.gitignore"), "/.next\nnode_modules\n")
        .file(format!("{REPO}/package.json"), r#"{"scripts":{"build":"next build"}}"#)
        .file(format!("{REPO}/Makefile"), "test: build\n\tcargo test\nlint:\n\tcargo clippy\nX := 1\n");
    let a = run(&fs);
    let f = find(&a, "dead-reference");
    assert_eq!(f.len(), 1, "{:#?}", a.findings);
    let ev = f[0].evidence.join("\n");
    assert!(ev.contains("`docs/gone.md`"), "{ev}");
    assert!(ev.contains("`npm run vanished`"), "{ev}");
    assert!(ev.contains("`make gone`"), "{ev}");
    for fine in [
        "deploy.md",
        "src/",
        "agent-kit",
        ".next",
        "series",
        "npm run build",
        "make test",
        "make lint",
        "shared-files",
        "check:siblings",
        "vendor",
        "my_parser",
        "model.mjs",
        "CREDIT.md",
    ] {
        assert!(!ev.contains(fine), "{fine} is not dead: {ev}");
    }
    assert!(f[0].title.contains("3 paths or scripts"), "{}", f[0].title);
}

#[test]
fn permission_rules_broad_one_off_and_covered() {
    let fs = base()
        .file(
            format!("{REPO}/.claude/settings.local.json"),
            r#"{"permissions":{"allow":[
                "Bash(rm:*)", "Bash(git push *)", "Read(~/.ssh/**)", "Bash(npm run lint)",
                "Bash(curl -s https://api.example.com/v1/items?id=42)",
                "Bash(npm run build -- --profile)", "Bash(git status)"
            ]}}"#,
        )
        .file(format!("{REPO}/.claude/settings.json"), r#"{"permissions":{"allow":["Bash(npm run:*)"]}}"#);
    let a = run(&fs);
    let broad = find(&a, "permission-broad");
    assert_eq!(broad.len(), 1);
    assert_eq!(broad[0].severity, Severity::High);
    assert_eq!(broad[0].items.len(), 3, "{:?}", broad[0].evidence);
    let one_off = find(&a, "permission-one-off");
    assert_eq!(one_off.len(), 1);
    assert!(one_off[0].evidence[0].contains("curl -s https://api.example.com"));
    let covered = find(&a, "permission-covered");
    assert_eq!(covered.len(), 1);
    assert_eq!(covered[0].items.len(), 2, "both npm run rules: {:?}", covered[0].evidence);
    assert!(
        covered[0].evidence[0].contains("covered by npm run:*") || covered[0].evidence[0].contains("Bash(npm run:*)")
    );
    assert_eq!(covered[0].file.as_deref(), Some(".claude/settings.local.json"));
}

#[test]
fn harness_warnings_become_findings_and_high_comes_first() {
    let fs = base()
        .file(format!("{HOME}/.claude/agents/free.md"), "---\nname: free\ndescription: no tools listed\n---\n")
        .file(format!("{REPO}/.claude/settings.local.json"), r#"{"permissions":{"allow":["Bash"]}}"#);
    let a = run(&fs);
    let w = find(&a, "warning:agent-all-tools");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].items, ["claude-code:agent:user:free"]);
    assert_eq!(a.findings[0].code, "permission-broad", "high severity first");
}

#[test]
fn the_memory_index_has_no_line_budget_finding() {
    let long: String = (0..400).map(|n| format!("- note {n}\n")).collect();
    let fs = base()
        .dir(format!("{REPO}/.git"))
        .file(format!("{HOME}/.claude/projects/-home-dev-work-shop/memory/MEMORY.md"), long);
    assert!(find(&run(&fs), "instruction-budget").is_empty());
}

#[test]
fn fleet_mode_recommends_by_how_many_repos_need_each_item() {
    let row = |id: &str, needed: Option<bool>| audit::UserScoped {
        id: id.into(),
        kind: "agent".into(),
        name: id.into(),
        tokens: 100,
        needed,
    };
    let repo = |name: &str, rows: Vec<audit::UserScoped>| Audit {
        tool: "claude-code".into(),
        repo: name.into(),
        tokens: 1000,
        findings: Vec::new(),
        user_scope: rows,
        notes: Vec::new(),
    };
    let audits = vec![
        repo(
            "a",
            vec![row("everywhere", Some(true)), row("one", Some(true)), row("none", Some(false)), row("plug", None)],
        ),
        repo(
            "b",
            vec![row("everywhere", Some(true)), row("one", Some(false)), row("none", Some(false)), row("plug", None)],
        ),
        repo(
            "c",
            vec![row("everywhere", Some(false)), row("one", Some(false)), row("none", Some(false)), row("plug", None)],
        ),
    ];
    let f = audit::fleet(&audits);
    let rec = |id: &str| f.items.iter().find(|i| i.id == id).unwrap();
    assert_eq!(rec("everywhere").recommendation, Recommendation::Keep);
    assert_eq!(rec("one").recommendation, Recommendation::MoveToProject);
    assert_eq!(rec("one").needed_in, ["a"]);
    assert_eq!(rec("none").recommendation, Recommendation::Archive);
    assert_eq!(rec("plug").recommendation, Recommendation::Unknown);
    assert_eq!(f.items[0].id, "none", "archive first");
    assert!(rec("one").recommendation.sentence(rec("one")).contains("needed in 1 of 3"));
    assert_eq!(f.repos.len(), 3);
    let mut plugin = rec("none").clone();
    plugin.kind = "plugin".into();
    assert!(plugin.recommendation.sentence(&plugin).contains("has the files it is for"), "plugins are judged by files");
}

#[test]
fn a_repo_instruction_file_linking_out_of_the_repo_is_never_quoted() {
    let fs = base()
        .dir(format!("{REPO}/docs"))
        .file("/home/dev/private/notes.md", "Secret: see `docs/CANARY-token.md` and run `npm run CANARY`.\n")
        .symlink(format!("{REPO}/CLAUDE.md"), "/home/dev/private/notes.md")
        .file(format!("{REPO}/package.json"), r#"{"scripts":{}}"#);
    let a = run(&fs);
    let all = serde_json::to_string(&a).unwrap();
    assert!(!all.contains("CANARY"), "{all}");
    assert!(a.notes.iter().any(|n| n.contains("CLAUDE.md links to a file outside the repo")), "{:?}", a.notes);
    // The harness still counts it: Claude Code would load it.
    let c = ctx(&fs);
    let h = ClaudeCode.resolve(&c);
    assert!(h.items.iter().any(|i| i.name == "CLAUDE.md" && i.effective));
}

#[test]
fn a_claude_md_that_mentions_agents_md_without_importing_it_is_a_finding() {
    let rules = "# Rules\n".to_string() + &"Always run the tests before you commit anything.\n".repeat(20);
    let fs = base()
        .file(format!("{REPO}/CLAUDE.md"), "See [AGENTS.md](AGENTS.md) for the rules.\n")
        .file(format!("{REPO}/AGENTS.md"), rules.clone())
        .file(format!("{REPO}/packages/api/CLAUDE.md"), "参考[AGENTS.md](AGENTS.md)")
        .file(format!("{REPO}/packages/api/AGENTS.md"), rules.clone())
        .file(format!("{REPO}/packages/web/CLAUDE.md"), "@AGENTS.md\n")
        .file(format!("{REPO}/packages/web/AGENTS.md"), rules.clone())
        .file(format!("{REPO}/packages/ui/CLAUDE.md"), "# UI\n")
        .file(format!("{REPO}/packages/ui/AGENTS.md"), rules);
    let a = run(&fs);
    let f = find(&a, "agents-md-not-imported");
    assert_eq!(f.len(), 2, "{:#?}", a.findings);
    let top = f.iter().find(|x| x.title.starts_with("CLAUDE.md mentions")).expect("the startup one");
    assert!(top.evidence[0].contains("See [AGENTS.md](AGENTS.md)"), "{:?}", top.evidence);
    let subs = f.iter().find(|x| x.title.contains("in subfolders")).expect("the subfolder one");
    assert!(subs.title.starts_with("1 CLAUDE.md in subfolders"), "{}", subs.title);
    assert!(subs.evidence[0].starts_with("packages/api/CLAUDE.md"), "{:?}", subs.evidence);
    let all = format!("{:?}", f);
    assert!(!all.contains("packages/web") && !all.contains("packages/ui"), "imported, or not mentioned: {all}");

    // Imported at the top: nothing to report there.
    let fixed = fs.clone().file(format!("{REPO}/CLAUDE.md"), "@AGENTS.md\n");
    assert!(find(&run(&fixed), "agents-md-not-imported").iter().all(|x| !x.title.starts_with("CLAUDE.md")));
}

#[test]
fn dsh_s_chain_budget_names_the_cut_line_and_quotes_the_text_at_it() {
    // Multilingual rules: one CJK character costs 3 bytes, so the chain budget (a byte figure) is exceeded
    // far before the chars/4 token estimate suggests it.
    let para = "各リポジトリのルールに従い、変更ごとにテストを実行し、差分をレビューに出してください。";
    let mut text = String::from("# Shared rules\n\nEnglish rules for the shop.\n\n");
    let mut section = 1;
    loop {
        text.push_str(&format!("## 第{section}節：共有ルール\n{para}\n"));
        section += 1;
        if text.len() > 70_000 {
            break;
        }
    }
    let fs = MemFs::new()
        .dir(format!("{REPO}/.claude"))
        .file(format!("{HOME}/.dsh/AGENTS.md"), "# House rules\n\nSmall house rules.\n")
        .file(format!("{REPO}/AGENTS.md"), &text);
    let a = run_dsh(&fs);
    let f = find(&a, "instruction-budget");
    assert_eq!(f.len(), 1, "{:#?}", a.findings.iter().map(|f| &f.code).collect::<Vec<_>>());
    let f = f[0];
    assert_eq!(f.severity, Severity::High, "the cut tail never runs");
    assert!(f.title.contains("65,536"), "the budget in the title: {}", f.title);
    assert!(f.title.contains("line"), "the cut is a line, not a figure only: {}", f.title);
    let all = f.evidence.join("\n");
    assert!(all.contains(&format!("{} bytes", text.len())), "exact bytes next to the estimate: {all}");
    assert!(all.contains("bytes a character"), "the divergence line: {all}");
    assert!(all.contains("chars/4"), "the token estimate stays alongside: {all}");
    assert!(all.contains("the cut lands on line"), "{all}");
    assert!(all.contains("⟨cut⟩"), "the boundary marker: {all}");
    // The quoted text is this file's own multilingual rules, not a generic sentence.
    assert!(all.contains("共有"), "the excerpt quotes the actual instructions: {all}");
    assert!(f.saving.is_none(), "instruction files are never rewritten");
    assert!(
        find(&a, "instruction-budget").len() == 1,
        "the small chain files stay out: {:#?}",
        a.findings.iter().map(|f| &f.code).collect::<Vec<_>>()
    );
}

#[test]
fn a_dsh_chain_under_the_budget_is_left_alone_and_claude_s_estimate_gets_bytes_only() {
    let small = MemFs::new()
        .dir(format!("{REPO}/.claude"))
        .file(format!("{HOME}/.dsh/AGENTS.md"), "# House rules\n\nSmall.\n")
        .file(format!("{REPO}/AGENTS.md"), "# Shop rules\n\nBe kind.\n");
    assert!(find(&run_dsh(&small), "instruction-budget").is_empty(), "{}", run_dsh(&small).findings.len());

    // The same oversized, multilingual file under Claude Code: bytes and the divergence are shown, but
    // Claude Code models no byte budget for the file, so there is no cut to name.
    let para = "各リポジトリのルールに従い、変更ごとにテストを実行し、差分をレビューに出してください。";
    let mut text = String::from("# Shared rules\n");
    let mut section = 1;
    loop {
        text.push_str(&format!("## 第{section}節：共有ルール\n{para}\n"));
        section += 1;
        if text.len() > 40_000 {
            break;
        }
    }
    let a = run(&MemFs::new().dir(format!("{REPO}/.claude")).file(format!("{REPO}/CLAUDE.md"), &text));
    let f = find(&a, "instruction-budget");
    assert_eq!(f.len(), 1);
    let all = f[0].evidence.join("\n");
    assert!(all.contains("bytes"), "the byte figure is there: {all}");
    assert!(all.contains("bytes a character"), "the divergence is visible: {all}");
    assert!(!all.contains("the cut lands on line"), "no modeled byte budget, no named cut: {all}");
}
