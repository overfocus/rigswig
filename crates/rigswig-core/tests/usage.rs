//! Session-log usage (`rigswig usage`, `audit --usage`) on invented logs: the counts, the window, the dedupe, the
//! metadata-only guarantee (a canary in every field the reader must not keep), and that nothing reads a log unless
//! asked.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::adapters::{self, ClaudeCode};
use rigswig_core::audit::{self, Options, Severity};
use rigswig_core::fs::{DirEntry, EntryKind, Fs, MemFs};
use rigswig_core::model::Os;
use rigswig_core::usage::Usage;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/shop";
/// `/home/dev/work/shop` as Claude Code names its project folder.
const LOGS: &str = "/home/dev/.claude/projects/-home-dev-work-shop";
const CANARY: &str = "CANARY-SESSION-TEXT";

fn ctx(fs: &dyn Fs) -> Ctx<'_> {
    Ctx { fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux }
}

/// One assistant line: a response `id` with its usage, and `blocks` as its content.
fn assistant(session: &str, time: &str, id: &str, blocks: &str) -> String {
    format!(
        r#"{{"type":"assistant","sessionId":"{session}","timestamp":"{time}","cwd":"{REPO}","message":{{"id":"{id}","model":"model-a","role":"assistant","content":[{blocks}],"usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}}}}}}"#
    )
}

fn tool(id: &str, name: &str, input: &str) -> String {
    format!(r#"{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}"#)
}

fn log(lines: &[String]) -> String {
    lines.join("\n") + "\n"
}

/// A session that calls a skill, two agents, and an MCP tool, with the lines Claude Code repeats.
fn session_a() -> String {
    log(&[
        format!(
            r#"{{"type":"user","sessionId":"s1","timestamp":"2026-10-20T09:00:00Z","cwd":"{REPO}","message":{{"role":"user","content":"please {CANARY} do it"}}}}"#
        ),
        assistant(
            "s1",
            "2026-10-20T09:00:01Z",
            "msg_1",
            &format!(r#"{{"type":"thinking","thinking":"{CANARY}"}},{{"type":"text","text":"{CANARY}"}}"#),
        ),
        // The same response again, with a tool call: its tokens count once.
        assistant(
            "s1",
            "2026-10-20T09:00:02Z",
            "msg_1",
            &tool("call_1", "Skill", &format!(r#"{{"skill":"deploy","args":"{CANARY}"}}"#)),
        ),
        assistant(
            "s1",
            "2026-10-20T09:00:03Z",
            "msg_2",
            &tool(
                "call_2",
                "Agent",
                &format!(r#"{{"subagent_type":"reviewer","prompt":"{CANARY}","description":"{CANARY}"}}"#),
            ),
        ),
        // A resumed session writes a call again: counted once.
        assistant(
            "s1",
            "2026-10-20T09:00:04Z",
            "msg_2",
            &tool("call_2", "Agent", r#"{"subagent_type":"reviewer","prompt":"x"}"#),
        ),
        assistant(
            "s1",
            "2026-10-20T09:00:05Z",
            "msg_3",
            &tool("call_3", "Agent", &format!(r#"{{"prompt":"{CANARY}"}}"#)),
        ),
        assistant(
            "s1",
            "2026-10-20T09:00:06Z",
            "msg_4",
            &tool("call_4", "mcp__tracker__create_issue", &format!(r#"{{"title":"{CANARY}"}}"#)),
        ),
        assistant(
            "s1",
            "2026-10-20T09:00:07Z",
            "msg_5",
            &tool("call_5", "Bash", &format!(r#"{{"command":"echo {CANARY}"}}"#)),
        ),
        format!(
            r#"{{"type":"user","sessionId":"s1","timestamp":"2026-10-20T09:00:08Z","cwd":"{REPO}","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"call_5","content":"{CANARY}"}}]}},"toolUseResult":{{"stdout":"{CANARY}"}}}}"#
        ),
        format!(
            r#"{{"type":"attachment","sessionId":"s1","timestamp":"2026-10-20T09:00:09Z","cwd":"{REPO}","attachment":{{"type":"invoked_skills","skills":[{{"name":"release-notes","path":"/x","content":"{CANARY}"}}]}}}}"#
        ),
        format!(r#"{{"type":"ai-title","sessionId":"s1","aiTitle":"{CANARY}"}}"#),
        "not json at all".into(),
    ])
}

/// The logs of a few sessions: `s1` (above) with a subagent log, `s2` in a folder inside the repo, `s3` before
/// the window, and a sibling repo whose folder name shares the prefix.
fn logs() -> MemFs {
    MemFs::new()
        .dir(format!("{REPO}/.claude"))
        .file(format!("{LOGS}/s1.jsonl"), session_a())
        .file(
            format!("{LOGS}/s1/subagents/agent-1.jsonl"),
            log(&[assistant("s1", "2026-10-20T09:01:00Z", "msg_9", &tool("call_9", "mcp__tracker__list", "{}"))]),
        )
        .file(format!("{LOGS}/s1/tool-results/big.txt"), CANARY)
        .file(format!("{LOGS}/memory/MEMORY.md"), "- a note\n")
        .file(
            format!("{LOGS}-web/s2.jsonl"),
            log(&[assistant("s2", "2026-10-21T10:00:00Z", "msg_20", "")
                .replace(&format!(r#""cwd":"{REPO}""#), &format!(r#""cwd":"{REPO}/web""#))]),
        )
        .file(
            format!("{LOGS}/s3.jsonl"),
            log(&[assistant("s3", "2026-09-01T10:00:00Z", "msg_30", &tool("call_30", "Skill", r#"{"skill":"old"}"#))]),
        )
        .file(
            "/home/dev/.claude/projects/-home-dev-work-shop-admin/s4.jsonl",
            log(&[
                assistant("s4", "2026-10-22T10:00:00Z", "msg_40", &tool("call_40", "Skill", r#"{"skill":"admin"}"#))
                    .replace(&format!(r#""cwd":"{REPO}""#), r#""cwd":"/home/dev/work/shop-admin""#),
                // The sibling's own logs carry lines with no `cwd`, as real ones do: they follow the file,
                // never this repo.
                assistant(
                    "s4",
                    "2026-10-22T10:00:01Z",
                    "msg_41",
                    &tool("call_41", "Agent", r#"{"subagent_type":"sibling-agent"}"#),
                )
                .replace(&format!(r#","cwd":"{REPO}""#), ""),
            ]),
        )
}

fn usage(fs: &MemFs, since: Option<&str>) -> Usage {
    ClaudeCode.usage(&ctx(fs), since).expect("usage")
}

#[test]
fn counts_sessions_tokens_models_and_what_was_invoked() {
    let u = usage(&logs(), Some("2026-10-01"));
    assert_eq!(u.sessions, 2, "s1 (with its subagent) and s2; not s3 (before the window) or the sibling repo");
    assert_eq!(
        u.files, 5,
        "s1, its subagent log, s2, s3, and the sibling's (opened by its prefix; its lines are not counted)"
    );
    assert_eq!(u.skipped_lines, 1);
    // msg_1..msg_5, msg_9 and msg_20: seven responses, each counted once.
    assert_eq!(u.models.get("model-a"), Some(&7));
    assert_eq!((u.tokens.input, u.tokens.output, u.tokens.cache_read, u.tokens.cache_write), (70, 35, 700, 140));

    let calls = |kind: &str, name: &str| u.find(kind, name).map(|i| (i.calls, i.sessions));
    assert_eq!(calls("skill", "deploy"), Some((1, 1)));
    assert_eq!(calls("agent", "reviewer"), Some((1, 1)), "a call id seen twice is one call");
    assert_eq!(calls("agent", "general-purpose"), Some((1, 1)), "an Agent call with no type is the default agent");
    assert_eq!(calls("mcp", "tracker"), Some((2, 1)), "the subagent's call counts with its parent session");
    assert_eq!(calls("skill", "release-notes"), Some((0, 1)), "listed as invoked: the session counts, no call");
    assert_eq!(calls("skill", "old"), None, "before the window");
    assert_eq!(calls("skill", "admin"), None, "another repo whose folder name starts the same");
    assert_eq!(
        calls("agent", "sibling-agent"),
        None,
        "another repo's call with no cwd line: it follows its file, not this repo"
    );
    assert!(u.invoked.iter().all(|i| i.name != "Bash"), "built-in tools are not counted");
    assert_eq!(u.first.as_deref(), Some("2026-10-20T09:00:00Z"));
    assert!(!u.not_counted.is_empty());

    let all = usage(&logs(), None);
    assert_eq!(all.sessions, 3);
    assert!(all.find("skill", "old").is_some());
}

/// A line with no `cwd` follows its file: a sibling's file is another repo's whole, and the repo's own files
/// count theirs.
#[test]
fn a_cwdless_line_follows_its_file() {
    // A sibling whose only line carries no `cwd` would otherwise count as this repo's session.
    let sibling_only = MemFs::new().dir(format!("{REPO}/.claude")).file(
        "/home/dev/.claude/projects/-home-dev-work-shop-admin/s9.jsonl",
        log(&[assistant(
            "s9",
            "2026-11-01T10:00:00Z",
            "msg_90",
            &tool("call_90", "Agent", r#"{"subagent_type":"remote"}"#),
        )
        .replace(&format!(r#","cwd":"{REPO}""#), "")]),
    );
    let u = usage(&sibling_only, None);
    assert_eq!(u.sessions, 0, "the sibling folder is not the repo's, however its lines are built");
    assert!(u.find("agent", "remote").is_none());
    assert_eq!(u.files, 1, "the file was opened, its lines are just not counted");

    // In the repo's own folder a line with no `cwd` follows a file that runs in the repo.
    let own = MemFs::new().dir(format!("{REPO}/.claude")).file(
        format!("{LOGS}/s8.jsonl"),
        log(&[
            assistant("s8", "2026-10-20T09:00:00Z", "msg_80", &tool("call_80", "Skill", r#"{"skill":"deploy"}"#)),
            assistant(
                "s8",
                "2026-10-20T09:00:01Z",
                "msg_81",
                &tool("call_81", "Agent", r#"{"subagent_type":"reviewer"}"#),
            )
            .replace(&format!(r#","cwd":"{REPO}""#), ""),
        ]),
    );
    let u = usage(&own, None);
    assert_eq!(u.sessions, 1);
    assert_eq!(u.find("agent", "reviewer").map(|i| (i.calls, i.sessions)), Some((1, 1)));
}

/// A line with no `timestamp` follows its file: inside the window its session and its figures count; a file
/// with no timestamps to place it loses nothing when the run asked for every log, and its lines count as
/// skipped under a window.
#[test]
fn an_untimed_line_follows_its_file() {
    let mixed = MemFs::new()
        .dir(format!("{REPO}/.claude"))
        .file(
            format!("{LOGS}/s7.jsonl"),
            log(&[
                assistant("s7", "2026-10-20T09:00:00Z", "msg_70", &tool("call_70", "Skill", r#"{"skill":"deploy"}"#)),
                format!(
                    r#"{{"type":"assistant","sessionId":"s7","cwd":"{REPO}","message":{{"id":"msg_71","content":[{{"type":"tool_use","id":"call_71","name":"Agent","input":{{"subagent_type":"reviewer"}}}}]}}}}"#
                ),
            ]),
        )
        .file(
            format!("{LOGS}/s6.jsonl"),
            log(&[format!(
                r#"{{"type":"assistant","sessionId":"s6","cwd":"{REPO}","message":{{"id":"msg_60","content":[{{"type":"tool_use","id":"call_60","name":"Skill","input":{{"skill":"lint"}}}}]}}}}"#
            )]),
        );
    let u = usage(&mixed, None);
    assert_eq!(u.sessions, 2, "both sessions, the all-untimed one included");
    assert_eq!(u.first.as_deref(), Some("2026-10-20T09:00:00Z"), "untimed lines do not move the window");
    assert_eq!(u.find("agent", "reviewer").map(|i| i.calls), Some(1), "the untimed line's call counts with its file");
    assert_eq!(u.find("skill", "lint").map(|i| i.calls), Some(1), "every log there is includes the all-untimed file");
    assert_eq!(u.skipped_lines, 0);

    let w = usage(&mixed, Some("2026-10-01"));
    assert_eq!(w.sessions, 1, "s7 is in the window; s6 has no timestamp to place it with");
    assert!(w.find("skill", "lint").is_none(), "figures outside the window's reckoning are not counted");
    assert_eq!(w.skipped_lines, 1, "the line s6 cannot place is counted, not kept");

    let later = usage(&mixed, Some("2026-11-01"));
    assert_eq!(later.sessions, 0);
    assert_eq!(
        later.skipped_lines, 1,
        "s7's timestamps put its untimed line before the window, which is no fault; only s6's cannot be placed"
    );
}

/// A session that moved folders: its file counts because a `cwd` line is inside the repo, a line with no `cwd`
/// follows the file, and a line whose `cwd` is outside is excluded on its own — with or without a timestamp.
#[test]
fn a_mixed_cwd_file_counts_only_its_lines_inside() {
    let elsewhere = |l: String| l.replace(&format!(r#""cwd":"{REPO}""#), r#""cwd":"/home/dev/elsewhere""#);
    let fs = MemFs::new().dir(format!("{REPO}/.claude")).file(
        format!("{LOGS}/s5.jsonl"),
        log(&[
            assistant("s5", "2026-10-20T09:00:00Z", "msg_50", &tool("call_50", "Skill", r#"{"skill":"deploy"}"#)),
            elsewhere(assistant(
                "s5",
                "2026-10-20T09:01:00Z",
                "msg_51",
                &tool("call_51", "Skill", r#"{"skill":"outside-timed"}"#),
            )),
            elsewhere(format!(
                r#"{{"type":"assistant","sessionId":"s5","cwd":"{REPO}","message":{{"id":"msg_52","content":[{{"type":"tool_use","id":"call_52","name":"Skill","input":{{"skill":"outside-untimed"}}}}]}}}}"#
            )),
            assistant(
                "s5",
                "2026-10-20T09:02:00Z",
                "msg_53",
                &tool("call_53", "Agent", r#"{"subagent_type":"reviewer"}"#),
            )
            .replace(&format!(r#","cwd":"{REPO}""#), ""),
        ]),
    );
    for since in [None, Some("2026-10-01")] {
        let u = usage(&fs, since);
        assert_eq!(u.sessions, 1, "{since:?}");
        assert_eq!(u.find("skill", "deploy").map(|i| i.calls), Some(1), "the line inside counts ({since:?})");
        assert_eq!(u.find("agent", "reviewer").map(|i| i.calls), Some(1), "the cwd-less line follows ({since:?})");
        assert!(u.find("skill", "outside-timed").is_none(), "a timed line outside is excluded ({since:?})");
        assert!(u.find("skill", "outside-untimed").is_none(), "an untimed line outside is excluded ({since:?})");
        assert_eq!(u.skipped_lines, 0, "{since:?}");
    }
}

/// The guarantee: nothing of a message, a tool's input or result, or an attachment's content comes out — not in
/// the usage JSON, not in an audit that uses it.
#[test]
fn no_message_text_tool_input_or_result_comes_out() {
    let fs = logs();
    let u = usage(&fs, None);
    let json = serde_json::to_string(&u).unwrap();
    assert!(!json.contains("CANARY"), "{json}");
    assert!(!format!("{u:?}").contains("CANARY"));

    let c = ctx(&fs);
    let h = ClaudeCode.resolve(&c);
    let a = audit::audit(&c, &h, &Options { usage: Some(u), ..Options::default() });
    assert!(!serde_json::to_string(&a).unwrap().contains("CANARY"));
}

#[test]
fn harnesses_without_a_log_reader_refuse() {
    let fs = logs();
    for a in adapters::all().iter().filter(|a| a.id() != "claude-code") {
        let e = a.usage(&ctx(&fs), None).expect_err("refuses");
        assert!(e.contains("does not read"), "{e}");
    }
    let e = ClaudeCode.usage(&ctx(&MemFs::new().dir(REPO)), None).expect_err("no logs folder");
    assert!(e.contains("no Claude Code session logs"), "{e}");
}

/// A file system that remembers every path it was asked about.
struct Recording<'a> {
    inner: &'a MemFs,
    seen: RefCell<Vec<PathBuf>>,
}

impl Fs for Recording<'_> {
    fn modified(&self, path: &Path) -> std::io::Result<std::time::SystemTime> {
        self.inner.modified(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.seen.borrow_mut().push(path.into());
        self.inner.read_to_string(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.seen.borrow_mut().push(path.into());
        self.inner.read_dir(path)
    }
    fn kind(&self, path: &Path) -> io::Result<EntryKind> {
        self.inner.kind(path)
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }
    fn for_each_line(&self, path: &Path, each: &mut dyn FnMut(Option<&str>)) -> io::Result<()> {
        self.seen.borrow_mut().push(path.into());
        self.inner.for_each_line(path, each)
    }
}

#[test]
fn nothing_opens_a_session_log_unless_the_run_asks() {
    let fs = logs();
    let rec = Recording { inner: &fs, seen: RefCell::new(Vec::new()) };
    let c = ctx(&rec);
    let h = ClaudeCode.resolve(&c);
    let _ = audit::audit(&c, &h, &Options::default());
    let seen = rec.seen.borrow();
    assert!(seen.iter().any(|p| p.ends_with("memory/MEMORY.md")), "the memory index is read, as before");
    for p in seen.iter() {
        let s = p.to_string_lossy();
        assert!(!s.ends_with(".jsonl") && !s.contains("/subagents"), "read without --usage: {s}");
        assert!(!s.ends_with("/.claude/projects") && p.as_path() != Path::new(LOGS), "listed without --usage: {s}");
    }
}

/// Five sessions in the window, each calling the skill `deploy`, the agent `reviewer` by its bare name, and nothing
/// else.
fn busy() -> MemFs {
    let mut fs = MemFs::new()
        .dir(format!("{REPO}/.claude"))
        .file(format!("{HOME}/.claude/settings.json"), r#"{"mcpServers":{}}"#)
        .file(format!("{REPO}/.mcp.json"), r#"{"mcpServers":{"tracker":{"command":"t"},"idle":{"command":"i"}}}"#)
        .file(format!("{REPO}/.claude/settings.json"), r#"{"enableAllProjectMcpServers":true}"#);
    for name in ["deploy", "lint", "translate"] {
        fs = fs.file(
            format!("{HOME}/.claude/skills/{name}/SKILL.md"),
            format!("---\nname: {name}\ndescription: {}\n---\nbody\n", "Does a thing well. ".repeat(20)),
        );
    }
    for name in ["reviewer", "planner"] {
        fs = fs.file(
            format!("{HOME}/.claude/agents/{name}.md"),
            format!("---\nname: {name}\ndescription: {}\n---\nbody\n", "Plans and reviews. ".repeat(20)),
        );
    }
    for n in 0..5 {
        fs = fs.file(
            format!("{LOGS}/b{n}.jsonl"),
            log(&[
                assistant(
                    &format!("b{n}"),
                    "2026-10-20T09:00:00Z",
                    &format!("m{n}a"),
                    &tool(&format!("c{n}a"), "Skill", r#"{"skill":"deploy"}"#),
                ),
                assistant(
                    &format!("b{n}"),
                    "2026-10-20T09:00:01Z",
                    &format!("m{n}b"),
                    &tool(&format!("c{n}b"), "Agent", r#"{"subagent_type":"reviewer"}"#),
                ),
                assistant(
                    &format!("b{n}"),
                    "2026-10-20T09:00:02Z",
                    &format!("m{n}c"),
                    &tool(&format!("c{n}c"), "mcp__tracker__list", "{}"),
                ),
            ]),
        );
    }
    fs
}

fn audit_with_usage(fs: &MemFs) -> audit::Audit {
    let c = ctx(fs);
    let h = ClaudeCode.resolve(&c);
    let u = ClaudeCode.usage(&c, None).unwrap();
    let a = audit::audit(&c, &h, &Options { usage: Some(u), ..Options::default() });
    for f in &a.findings {
        let all = format!("{} {} {}", f.title, f.evidence.join(" "), f.proposal).to_lowercase();
        assert!(!all.contains("unused"), "findings never claim more than the logs show: {all}");
    }
    a
}

#[test]
fn the_audit_names_what_loads_and_no_session_invoked() {
    let a = audit_with_usage(&busy());
    assert!(a.usage.as_ref().is_some_and(|u| u.sessions == 5));
    let not: Vec<_> = a.findings.iter().filter(|f| f.code == "not-invoked").collect();
    let items: Vec<&str> = not.iter().flat_map(|f| f.items.iter().map(String::as_str)).collect();
    // Invoked ones are not named, whatever the repo's files say.
    assert!(items.iter().all(|i| !i.ends_with(":deploy") && !i.ends_with(":reviewer") && !i.ends_with(":tracker")));
    let heavy: Vec<&str> = a
        .findings
        .iter()
        .filter(|f| f.code == "heavy-user-scope")
        .flat_map(|f| f.items.iter().map(String::as_str))
        .collect();
    // Each idle item is named once: by heavy-user-scope when it already covers it, else by not-invoked.
    for want in ["lint", "translate", "planner", "idle"] {
        let n = items.iter().chain(heavy.iter()).filter(|i| i.ends_with(&format!(":{want}"))).count();
        assert_eq!(n, 1, "{want}: {items:?} {heavy:?}");
    }
    let skills = not.iter().find(|f| f.items.iter().any(|i| i.contains(":skill:")));
    let s = skills.expect("lint and translate: a not-invoked skills finding");
    assert_ne!(s.severity, Severity::High, "a skill typed as /name is not counted, so never High");
    assert!(s.evidence.iter().any(|e| e.contains("/name")));
    // heavy-user-scope sees an invoked item as needed even though nothing in the repo names it.
    let row = a.user_scope.iter().find(|r| r.name == "deploy").unwrap();
    assert_eq!(row.needed, Some(true));
}

#[test]
fn a_plugin_item_called_by_its_bare_name_counts_as_invoked() {
    let u = Usage {
        sessions: 9,
        invoked: vec![rigswig_core::usage::Invoked {
            kind: "agent".into(),
            name: "builder".into(),
            calls: 3,
            sessions: 2,
        }],
        ..Usage::default()
    };
    let fs = MemFs::new()
        .dir(format!("{REPO}/.claude"))
        .file(
            format!("{HOME}/.claude/plugins/installed_plugins.json"),
            r#"{"version":2,"plugins":{"loop@kit":[{"scope":"user","installPath":"/home/dev/.claude/plugins/cache/kit/loop/1"}]}}"#,
        )
        .file(format!("{HOME}/.claude/settings.json"), r#"{"enabledPlugins":{"loop@kit":true}}"#)
        .file("/home/dev/.claude/plugins/cache/kit/loop/1/.claude-plugin/plugin.json", r#"{"name":"loop"}"#)
        .file(
            "/home/dev/.claude/plugins/cache/kit/loop/1/agents/builder.md",
            "---\nname: builder\ndescription: Builds.\n---\nbody\n",
        );
    let c = ctx(&fs);
    let h = ClaudeCode.resolve(&c);
    assert!(h.items.iter().any(|i| i.name == "loop:builder"), "fixture: the plugin agent loads");
    let a = audit::audit(&c, &h, &Options { usage: Some(u), ..Options::default() });
    assert!(
        a.findings
            .iter()
            .filter(|f| f.code == "not-invoked")
            .all(|f| f.items.iter().all(|i| !i.ends_with("loop:builder"))),
        "{:?}",
        a.findings
    );
}

#[test]
fn too_few_sessions_is_a_note_not_a_finding() {
    let fs = logs();
    let c = ctx(&fs);
    let h = ClaudeCode.resolve(&c);
    let u = ClaudeCode.usage(&c, None).unwrap();
    assert!(u.sessions < rigswig_core::usage::MIN_SESSIONS);
    let a = audit::audit(&c, &h, &Options { usage: Some(u), ..Options::default() });
    assert!(a.findings.iter().all(|f| f.code != "not-invoked"));
    assert!(a.notes.iter().any(|n| n.contains("too few")));
}
