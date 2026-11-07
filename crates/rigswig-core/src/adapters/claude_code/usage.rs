//! Claude Code's session logs, as metadata (see [`crate::usage`] for the rule).
//!
//! Where they are: one JSON Lines file per session under `<config>/projects/<project folder>/`, the path a hook is
//! handed as `transcript_path` (<https://code.claude.com/docs/en/hooks.md>, "Hook input"); a subagent's log sits in
//! `<session>/subagents/` with its parent's `sessionId`. Claude Code deletes them after `cleanupPeriodDays`, 30 by
//! default (<https://code.claude.com/docs/en/settings.md>). The line format itself is not documented: the shapes
//! below are inferred from the files Claude Code writes, and a line in any other shape is skipped and counted.
//!
//! Two things the files themselves settle, because the folder name and the timestamp can both be missing: the
//! `cwd` lines of a file say which repo it is (sibling repos share the folder-name prefix), and a line without
//! its `timestamp` — Claude Code repeats some responses that way — follows its file, counting its session and
//! its figures inside the window, landing in the skipped count outside it.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, SeqAccess, Visitor};

use super::Paths;
use crate::adapter::Ctx;
use crate::fs::EntryKind;
use crate::usage::{Tally, Tokens, Usage};

// The only shapes a line is read into. A field that is not named here — message text, thinking, every tool input
// other than `skill` and `subagent_type`, tool results, attachments' content — is skipped by serde without being
// stored. Adding a field here widens what rigswig reads from a session log: it needs a reason in the commit and a
// canary in `tests/usage.rs`.

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    message: Option<Message>,
    attachment: Option<Attachment>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawTokens>,
    #[serde(default, deserialize_with = "blocks")]
    content: Vec<Block>,
}

#[derive(Deserialize)]
struct RawTokens {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Input>,
}

/// The two tool arguments that name what a call invoked. Nothing else of a tool's input is read.
#[derive(Deserialize)]
struct Input {
    skill: Option<String>,
    subagent_type: Option<String>,
}

#[derive(Deserialize)]
struct Attachment {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "names")]
    skills: Vec<Named>,
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

/// A message's `content`: the blocks of a list; a string (a user's typed text) is skipped unread.
fn blocks<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Block>, D::Error> {
    d.deserialize_any(SeqOrSkip::<Block>(std::marker::PhantomData))
}

fn names<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Named>, D::Error> {
    d.deserialize_any(SeqOrSkip::<Named>(std::marker::PhantomData))
}

/// A list of `T`, or anything else skipped: the visitor's `visit_str` takes the text and drops it.
struct SeqOrSkip<T>(std::marker::PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for SeqOrSkip<T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a list, or anything else to skip")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut out = Vec::new();
        while let Some(x) = seq.next_element()? {
            out.push(x);
        }
        Ok(out)
    }
    fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Vec::new())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
    fn visit_unit<E: de::Error>(self) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
    fn visit_none<E: de::Error>(self) -> Result<Vec<T>, E> {
        Ok(Vec::new())
    }
}

/// Default built-in agent when the `Agent` tool is called with no `subagent_type`.
const DEFAULT_AGENT: &str = "general-purpose";

pub(crate) fn not_counted() -> Vec<String> {
    vec![
        "skills and commands typed as /name: they are in message text, which is never read (a skill shows as invoked only when the model called it, or when the log lists it as invoked for the session)".into(),
        "hooks: their runs are not in the parts of the log this reads".into(),
        "built-in tools (Bash, Read, Edit and the rest): they are not part of a setup".into(),
        "sessions started outside the repo's folder (a worktree elsewhere), and logs Claude Code has already deleted (cleanupPeriodDays, 30 days by default)".into(),
        "the log format is not documented: the shapes read are inferred from the files Claude Code writes, and other lines are skipped".into(),
    ]
}

/// Claude Code's project-folder name for a path: every character that is not a letter or a digit becomes `-`.
fn project_folder_name(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// The project folders Claude Code would have written for sessions in `repo` or a folder inside it, so a folder
/// inside the repo shares the repo's name as a prefix. A sibling with a longer name shares it too; the `cwd`
/// lines each file carries settle it.
fn project_folders(ctx: &Ctx, projects: &Path) -> Vec<PathBuf> {
    let base = project_folder_name(&ctx.repo);
    let Ok(entries) = ctx.fs.read_dir(projects) else { return Vec::new() };
    entries
        .into_iter()
        .filter(|e| e.kind == EntryKind::Dir && (e.name == base || e.name.starts_with(&format!("{base}-"))))
        .map(|e| projects.join(e.name))
        .collect()
}

/// The session logs in one project folder: `*.jsonl`, and each session's `subagents/*.jsonl`.
fn logs(ctx: &Ctx, dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = ctx.fs.read_dir(dir) else { return out };
    for e in entries {
        let p = dir.join(&e.name);
        match e.kind {
            EntryKind::File if e.name.ends_with(".jsonl") => out.push(p),
            EntryKind::Dir if e.name != "memory" => {
                if let Ok(subs) = ctx.fs.read_dir(&p.join("subagents")) {
                    out.extend(
                        subs.into_iter()
                            .filter(|s| s.kind == EntryKind::File && s.name.ends_with(".jsonl"))
                            .map(|s| p.join("subagents").join(s.name)),
                    );
                }
            }
            _ => {}
        }
    }
    out
}

fn within(cwd: &str, repo: &Path) -> bool {
    Path::new(cwd).starts_with(repo)
}

pub(crate) fn usage(ctx: &Ctx, since: Option<&str>) -> Result<Usage, String> {
    let projects = Paths::new(ctx).cfg.join("projects");
    if !ctx.fs.exists(&projects) {
        return Err(format!("no Claude Code session logs: {} does not exist", projects.display()));
    }
    let own = project_folder_name(&ctx.repo);
    let mut t = Tally::default();
    for dir in project_folders(ctx, &projects) {
        for file in logs(ctx, &dir) {
            let mut lines: Vec<Line> = Vec::new();
            let mut skipped = 0;
            let read = ctx.fs.for_each_line(&file, &mut |raw| match raw.map(serde_json::from_str::<Line>) {
                Some(Ok(l)) => lines.push(l),
                Some(Err(_)) | None => skipped += 1,
            });
            if read.is_err() {
                t.files_unread += 1;
                continue;
            }
            t.files += 1;
            t.skipped_lines += skipped;
            // What the file settles for its own lines. Its `cwd` lines say whose log it is: a file whose
            // `cwd` lines all sit outside is a sibling's whole file, even where some lines carry no `cwd`,
            // and a file with no `cwd` at all can only be the repo's in the repo's own projects folder. Its
            // timestamped lines say whether the session is inside the window.
            let any_cwd = lines.iter().any(|l| l.cwd.is_some());
            let cwd_inside = lines.iter().any(|l| l.cwd.as_deref().is_some_and(|c| within(c, &ctx.repo)));
            let this_repo = if cwd_inside {
                true
            } else if any_cwd {
                false
            } else {
                dir.file_name().is_some_and(|n| n.to_string_lossy() == own)
            };
            if !this_repo {
                continue;
            }
            let in_window =
                since.is_none_or(|s| lines.iter().any(|l| l.timestamp.as_deref().is_some_and(|time| time >= s)));
            for line in lines {
                read_line(ctx, since, &mut t, line, in_window);
            }
        }
    }
    let repo = ctx.repo.file_name().map_or_else(|| ctx.repo.to_string_lossy(), |n| n.to_string_lossy()).into_owned();
    Ok(t.finish(super::ID, &repo, since, not_counted()))
}

fn read_line(ctx: &Ctx, since: Option<&str>, t: &mut Tally, line: Line, in_window: bool) {
    let Some(session) = line.session_id.clone() else { return };
    let session = session.as_str();
    let Some(time) = line.timestamp.as_deref() else {
        // A line with no `timestamp` — Claude Code repeats some responses without one. It follows its
        // file: inside the window the session counts, and its calls and tokens with it (an id a
        // timestamped line already counted counts once either way); outside it, the line cannot be
        // placed, so it is counted and never kept.
        if in_window {
            t.saw_untimed(session);
            figures(t, session, line);
        } else {
            t.skipped_lines += 1;
        }
        return;
    };
    if since.is_some_and(|s| time < s) {
        return;
    }
    // A line with no `cwd` follows its file, which the caller has settled.
    if line.cwd.as_deref().is_some_and(|c| !within(c, &ctx.repo)) {
        return;
    }
    t.saw(session, time);
    figures(t, session, line);
}

/// What a counted line shows: the response's tokens, the calls it carries, and what its log lists as invoked.
fn figures(t: &mut Tally, session: &str, line: Line) {
    if let Some(m) = line.message {
        if let (Some(id), Some(u)) = (m.id.as_deref(), m.usage) {
            let tokens = Tokens {
                input: u.input_tokens.unwrap_or(0),
                output: u.output_tokens.unwrap_or(0),
                cache_read: u.cache_read_input_tokens.unwrap_or(0),
                cache_write: u.cache_creation_input_tokens.unwrap_or(0),
            };
            t.response(id, m.model.as_deref(), tokens);
        }
        for b in m.content.into_iter().filter(|b| b.kind.as_deref() == Some("tool_use")) {
            let (Some(call), Some(name)) = (b.id.as_deref(), b.name.as_deref()) else { continue };
            let input = b.input.unwrap_or(Input { skill: None, subagent_type: None });
            match name {
                "Skill" => {
                    if let Some(s) = input.skill.as_deref() {
                        t.invoke(call, session, "skill", s);
                    }
                }
                "Agent" | "Task" => {
                    t.invoke(call, session, "agent", input.subagent_type.as_deref().unwrap_or(DEFAULT_AGENT));
                }
                _ => {
                    if let Some(server) = name.strip_prefix("mcp__").and_then(|r| r.split("__").next()) {
                        t.invoke(call, session, "mcp", server);
                    }
                }
            }
        }
    }
    if let Some(a) = line.attachment.filter(|a| a.kind.as_deref() == Some("invoked_skills")) {
        for name in a.skills.into_iter().filter_map(|s| s.name) {
            t.present(session, "skill", &name);
        }
    }
}
