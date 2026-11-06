//! Readers more than one adapter uses: the hook event map that Claude Code and Codex share.

use serde_json::{Map, Value};

use crate::adapters::build::{Build, calc};
use crate::model::*;
use crate::read::redact::{script_of, url_origin};

/// A tool-neutral name for Claude Code's hook events.
pub fn neutral_event(event: &str) -> String {
    match event {
        "PreToolUse" => "before-tool".into(),
        "PostToolUse" => "after-tool".into(),
        "PostToolUseFailure" => "after-tool-failure".into(),
        // Cursor's camelCase spellings of the same loop stages.
        "preToolUse" => "before-tool".into(),
        "postToolUse" => "after-tool".into(),
        "postToolUseFailure" => "after-tool-failure".into(),
        "beforeSubmitPrompt" => "prompt-submit".into(),
        "preCompact" => "before-compact".into(),
        "sessionStart" => "session-start".into(),
        "sessionEnd" => "session-end".into(),
        "subagentStart" => "subagent-start".into(),
        "subagentStop" => "subagent-stop".into(),
        "stop" => "stop".into(),
        // Gemini CLI's compaction hook is the same lifecycle point.
        "PreCompress" => "before-compact".into(),
        "pre_tool_call" => "before-tool".into(),
        "post_tool_call" => "after-tool".into(),
        "subagent_start" => "subagent-start".into(),
        "subagent_stop" => "subagent-stop".into(),
        "UserPromptSubmit" => "prompt-submit".into(),
        "SessionStart" => "session-start".into(),
        "SessionEnd" => "session-end".into(),
        "Stop" => "stop".into(),
        "SubagentStart" => "subagent-start".into(),
        "SubagentStop" => "subagent-stop".into(),
        "PreCompact" => "before-compact".into(),
        "Notification" => "notification".into(),
        other => {
            // CamelCase to kebab-case for events not listed here.
            let mut out = String::new();
            for (i, c) in other.chars().enumerate() {
                if c.is_ascii_uppercase() && i > 0 {
                    out.push('-');
                }
                out.push(c.to_ascii_lowercase());
            }
            out
        }
    }
}

/// The hooks in one event map (`{ "PreToolUse": [ { matcher, hooks: [ ... ] } ] }`), returning their indices.
/// `prefix` keeps names unique when one layer has more than one source of hooks (e.g. `config.toml:`).
pub fn hooks_of(
    b: &mut Build,
    block: &Map<String, Value>,
    layer: &str,
    file: Option<String>,
    prefix: &str,
) -> Vec<usize> {
    let mut out = Vec::new();
    for (event, groups) in block {
        let Some(groups) = groups.as_array() else { continue };
        let mut n = 0;
        for g in groups {
            let matcher = g.get("matcher").and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from);
            for h in g.get("hooks").and_then(Value::as_array).into_iter().flatten() {
                n += 1;
                let kind = h.get("type").and_then(Value::as_str).unwrap_or("command");
                let (handler, script) = match kind {
                    "command" => (HookHandler::Command, h.get("command").and_then(Value::as_str).and_then(script_of)),
                    "http" => (HookHandler::Http, h.get("url").and_then(Value::as_str).and_then(url_origin)),
                    "prompt" => (HookHandler::Prompt, None),
                    "agent" => (HookHandler::Agent, None),
                    "mcp_tool" => (HookHandler::McpTool, None),
                    _ => (HookHandler::Unknown, None),
                };
                let attrs = HookAttrs {
                    event: neutral_event(event),
                    native_event: event.clone(),
                    matcher: matcher.clone(),
                    handler,
                    script,
                };
                let mut it =
                    b.item(layer, &format!("{prefix}{event}-{n}"), ItemDetail::Hook(attrs), LoadWhen::Never, calc(0));
                it.origin.file = file.clone();
                out.push(b.push(it));
            }
        }
    }
    out
}
