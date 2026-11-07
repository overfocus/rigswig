//! Opt-in usage: what past sessions in a repo actually invoked, read from the harness's own session logs.
//!
//! This is the one place rigswig opens session logs, and only when a run asks for it (`rigswig usage`,
//! `rigswig audit --usage`). What it keeps is metadata, by construction: each adapter deserialises a log line into
//! structs that have fields for ids, timestamps, the working folder, the model, token counts, tool names, and the two
//! tool arguments that name what was invoked (a skill's name, an agent's type) — and nothing else. Every other field,
//! message text, thinking, tool inputs and tool results included, is skipped by the parser without being stored, so
//! no code path past the parser can print, serve or keep it. `tests/usage.rs` plants a canary in every one of those
//! fields and checks that it never comes out.
//!
//! The figures are counts of what the logs record, not judgements: a skill the user typed as `/name` is in message
//! text, which this never reads, so it is not counted (see [`Usage::not_counted`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Fewer sessions than this and the audit does not judge "never invoked": a handful of sessions says little.
pub const MIN_SESSIONS: u64 = 5;

/// What the sessions in one repo invoked, over a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub tool: String,
    pub repo: String,
    /// The window's start (an ISO 8601 date or time), or absent for every log there is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Distinct sessions with at least one line in the window. A subagent's log counts with its parent session.
    pub sessions: u64,
    /// The earliest and latest timestamps seen in the window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    /// Token totals the logs report for model responses, each response counted once.
    pub tokens: Tokens,
    /// Model responses per model id.
    pub models: BTreeMap<String, u64>,
    /// Every agent, skill and MCP server the logs show invoked, most calls first.
    pub invoked: Vec<Invoked>,
    /// Log files read, log files that could not be (counted, not named), and lines that were not JSON, not UTF-8,
    /// over the line limit, or not in a shape this reads (skipped, never printed).
    pub files: u64,
    #[serde(default)]
    pub files_unread: u64,
    pub skipped_lines: u64,
    /// What the logs hold that this does not count, in words.
    pub not_counted: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invoked {
    /// `agent`, `skill` or `mcp` (an MCP server: calls to any of its tools).
    pub kind: String,
    pub name: String,
    pub calls: u64,
    /// Sessions that invoked it at least once.
    pub sessions: u64,
}

impl Usage {
    /// The record for an invoked item of `kind` called `name`, if the logs show one.
    pub fn find(&self, kind: &str, name: &str) -> Option<&Invoked> {
        self.invoked.iter().find(|i| i.kind == kind && i.name == name)
    }

    /// Whether an MCP server called `name` was invoked. A plugin's server reaches the logs with the plugin's name in
    /// front (`plugin_<plugin>_<server>`), so a match on the last part counts too.
    pub fn mcp_calls(&self, name: &str) -> u64 {
        let tail = format!("_{name}");
        self.invoked
            .iter()
            .filter(|i| i.kind == "mcp" && (i.name == name || i.name.ends_with(&tail)))
            .map(|i| i.calls)
            .sum()
    }
}

/// Collects counts as an adapter walks its logs, and dedupes what the logs repeat.
#[derive(Debug, Default)]
pub struct Tally {
    sessions: BTreeSet<String>,
    first: Option<String>,
    last: Option<String>,
    tokens: Tokens,
    models: BTreeMap<String, u64>,
    responses: BTreeSet<String>,
    calls: BTreeSet<String>,
    invoked: BTreeMap<(String, String), (u64, BTreeSet<String>)>,
    pub files: u64,
    pub files_unread: u64,
    pub skipped_lines: u64,
}

impl Tally {
    /// A line of `session` at `time` was in the window.
    pub fn saw(&mut self, session: &str, time: &str) {
        self.sessions.insert(session.to_string());
        if self.first.as_deref().is_none_or(|f| time < f) {
            self.first = Some(time.to_string());
        }
        if self.last.as_deref().is_none_or(|l| time > l) {
            self.last = Some(time.to_string());
        }
    }

    /// A model response `id`; its tokens count once however many lines repeat them.
    pub fn response(&mut self, id: &str, model: Option<&str>, tokens: Tokens) {
        if !self.responses.insert(id.to_string()) {
            return;
        }
        if let Some(m) = model {
            *self.models.entry(m.to_string()).or_default() += 1;
        }
        self.tokens.input += tokens.input;
        self.tokens.output += tokens.output;
        self.tokens.cache_read += tokens.cache_read;
        self.tokens.cache_write += tokens.cache_write;
    }

    /// A tool call `call_id` in `session` invoked `name` of `kind`; a call id seen before is not counted again.
    pub fn invoke(&mut self, call_id: &str, session: &str, kind: &str, name: &str) {
        if !self.calls.insert(call_id.to_string()) {
            return;
        }
        let e = self.invoked.entry((kind.to_string(), name.to_string())).or_default();
        e.0 += 1;
        e.1.insert(session.to_string());
    }

    /// The log lists `name` of `kind` as invoked in `session`, with no call to count: the session counts, the calls
    /// do not.
    pub fn present(&mut self, session: &str, kind: &str, name: &str) {
        self.invoked.entry((kind.to_string(), name.to_string())).or_default().1.insert(session.to_string());
    }

    pub fn finish(self, tool: &str, repo: &str, since: Option<&str>, not_counted: Vec<String>) -> Usage {
        let mut invoked: Vec<Invoked> = self
            .invoked
            .into_iter()
            .map(|((kind, name), (calls, s))| Invoked { kind, name, calls, sessions: s.len() as u64 })
            .collect();
        invoked.sort_by(|a, b| b.calls.cmp(&a.calls).then(a.kind.cmp(&b.kind)).then(a.name.cmp(&b.name)));
        Usage {
            tool: tool.into(),
            repo: repo.into(),
            since: since.map(Into::into),
            sessions: self.sessions.len() as u64,
            first: self.first,
            last: self.last,
            tokens: self.tokens,
            models: self.models,
            invoked,
            files: self.files,
            files_unread: self.files_unread,
            skipped_lines: self.skipped_lines,
            not_counted,
        }
    }
}
