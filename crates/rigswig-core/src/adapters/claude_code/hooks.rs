//! Hooks from every settings layer and every enabled plugin, and the two switches that turn them off.

use serde_json::{Map, Value};

use super::plugins::Plugin;
use super::settings::{Scope, resolved};
use crate::adapters::build::{Build, calc};
use crate::model::*;
use crate::read::json::obj_at;
use crate::read::redact::{script_of, url_origin};

/// A tool-neutral name for Claude Code's hook events.
fn neutral(event: &str) -> String {
    match event {
        "PreToolUse" => "before-tool".into(),
        "PostToolUse" => "after-tool".into(),
        "PostToolUseFailure" => "after-tool-failure".into(),
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
fn hooks_of(b: &mut Build, block: &Map<String, Value>, layer: &str, file: Option<String>) -> Vec<usize> {
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
                    event: neutral(event),
                    native_event: event.clone(),
                    matcher: matcher.clone(),
                    handler,
                    script,
                };
                let mut it = b.item(layer, &format!("{event}-{n}"), ItemDetail::Hook(attrs), LoadWhen::Never, calc(0));
                it.origin.file = file.clone();
                out.push(b.push(it));
            }
        }
    }
    out
}

pub(crate) fn all(b: &mut Build, scopes: &[Scope], plugins: &[Plugin]) {
    let mut by_layer: Vec<(String, Vec<usize>, bool)> = Vec::new(); // (layer, items, exempt from managed-only)
    for s in scopes {
        let file = b.layers.iter().find(|l| l.id == s.id).and_then(|l| l.path.clone());
        if let Some(block) = obj_at(&s.obj, "hooks").cloned() {
            let idx = hooks_of(b, &block, s.id, file);
            by_layer.push((s.id.into(), idx, s.id == "managed"));
        }
    }
    for pl in plugins {
        let mut idx = Vec::new();
        let default = pl.dir.join("hooks/hooks.json");
        let j = b.json(&default).object();
        if let Some(block) = obj_at(&j, "hooks").cloned() {
            let f = Some(b.display(&default));
            idx.extend(hooks_of(b, &block, &pl.layer, f));
        }
        for v in pl.manifest_list("hooks").unwrap_or_default() {
            match v {
                Value::String(rel) => {
                    let Some(f) = pl.path(b, &rel, "hooks") else { continue };
                    let j = b.json(&f).object();
                    if let Some(block) = obj_at(&j, "hooks").cloned() {
                        let shown = Some(b.display(&f));
                        idx.extend(hooks_of(b, &block, &pl.layer, shown));
                    }
                }
                Value::Object(block) => {
                    let block = obj_at(&block, "hooks").cloned().unwrap_or(block);
                    idx.extend(hooks_of(b, &block, &pl.layer, Some(".claude-plugin/plugin.json".into())));
                }
                _ => {}
            }
        }
        by_layer.push((pl.layer.clone(), idx, pl.forced_by_managed));
    }

    // disableAllHooks: the value after precedence turns off every non-managed hook; only a managed `true` turns off
    // managed hooks too. allowManagedHooksOnly (managed) turns off the rest, except plugins managed force-enables.
    let disable_all = resolved(b, "disableAllHooks").and_then(Value::as_bool).unwrap_or(false);
    let managed_disables =
        scopes.iter().any(|s| s.id == "managed" && s.obj.get("disableAllHooks") == Some(&Value::Bool(true)));
    let managed_only =
        scopes.iter().any(|s| s.id == "managed" && s.obj.get("allowManagedHooksOnly") == Some(&Value::Bool(true)));
    let mut off = 0;
    for (layer, idx, exempt) in by_layer {
        let is_managed = layer == "managed";
        for i in idx {
            let blocked =
                (disable_all && (!is_managed || managed_disables)) || (managed_only && !is_managed && !exempt);
            if blocked && b.items[i].effective {
                Build::disable(&mut b.items[i]);
                off += 1;
            }
        }
    }
    if off > 0 {
        b.warn(
            "hooks-off",
            format!(
                "{off} hook{} turned off by disableAllHooks or allowManagedHooksOnly",
                if off == 1 { " is" } else { "s are" }
            ),
            None,
        );
    }
}
