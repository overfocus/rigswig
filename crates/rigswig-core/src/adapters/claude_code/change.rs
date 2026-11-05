//! Planning a change to a Claude Code setup: one key in one of the three settings files a person owns (local, project,
//! user). Managed settings and `.claude.json` (which Claude Code itself rewrites) are never targets.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{ClaudeCode, Paths};
use crate::adapter::{Adapter, Ctx};
use crate::change::{Change, Plan, Scope, diff_lines};
use crate::fs::{EntryKind, OverlayFs};
use crate::model::{Harness, ItemDetail, McpState};
use crate::read::redact::sha256_hex;

fn set_list(obj: &mut Map<String, Value>, key: &str, name: &str, present: bool) {
    let list = obj.entry(key.to_string()).or_insert_with(|| Value::Array(Vec::new()));
    if !list.is_array() {
        *list = Value::Array(Vec::new());
    }
    let arr = list.as_array_mut().expect("an array");
    arr.retain(|v| v.as_str() != Some(name));
    if present {
        arr.push(Value::String(name.into()));
    }
    if arr.is_empty() {
        obj.remove(key);
    }
}

fn mcp_state(h: &Harness, name: &str) -> Option<McpState> {
    h.items.iter().find_map(|i| match &i.detail {
        ItemDetail::Mcp(m) if i.origin.layer == "project" && i.name == name => Some(m.state),
        _ => None,
    })
}

pub(crate) fn plan(ctx: &Ctx, change: &Change) -> Result<Plan, String> {
    let p = Paths::new(ctx);
    let scope = match change {
        Change::Plugin { scope, .. } | Change::McpJson { scope, .. } => *scope,
    };
    let path = match scope {
        Scope::Local => p.repo.join(".claude/settings.local.json"),
        Scope::Project => p.repo.join(".claude/settings.json"),
        Scope::User => p.cfg.join("settings.json"),
    };
    let before = ClaudeCode.resolve(ctx);

    // Checks that need the current setup.
    match change {
        Change::Plugin { id, .. } => {
            if !id.contains('@') || id.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err("a plugin id looks like name@marketplace".into());
            }
            let pkg = before.packages.iter().find(|x| &x.id == id).ok_or(format!("no plugin {id} is known here"))?;
            if pkg.scopes.contains_key("managed") {
                return Err(format!("managed settings decide {id}; rigswig never changes managed settings"));
            }
        }
        Change::McpJson { name, .. } => {
            if mcp_state(&before, name).is_none() {
                return Err(format!("{name} is not a server in this repo's .mcp.json"));
            }
        }
    }

    let fs = ctx.fs;
    if fs.kind(&path).ok() == Some(EntryKind::Symlink) {
        return Err("that settings file is a link; rigswig does not write through links".into());
    }
    let (text, creates_file) = match fs.read_to_string(&path) {
        Ok(t) => (Some(t), false),
        Err(_) if !fs.exists(&path) => (None, true),
        Err(_) => return Err("that settings file cannot be read".into()),
    };
    let mut obj: Map<String, Value> = match &text {
        None => Map::new(),
        Some(t) => match serde_json::from_str::<Value>(t.strip_prefix('\u{feff}').unwrap_or(t)) {
            Ok(Value::Object(o)) => o,
            Ok(_) => return Err("that settings file is not a JSON object; fix it by hand first".into()),
            Err(_) => return Err("that settings file is not valid JSON; fix it by hand first".into()),
        },
    };

    match change {
        Change::Plugin { id, on, .. } => {
            let en = obj.entry("enabledPlugins").or_insert_with(|| Value::Object(Map::new()));
            if !en.is_object() {
                return Err("enabledPlugins in that file is not an object; fix it by hand first".into());
            }
            en.as_object_mut().expect("an object").insert(id.clone(), Value::Bool(*on));
        }
        Change::McpJson { name, on, .. } => {
            set_list(&mut obj, "disabledMcpjsonServers", name, !on);
            set_list(&mut obj, "enabledMcpjsonServers", name, *on);
        }
    }
    let after = serde_json::to_string_pretty(&Value::Object(obj)).expect("JSON serialises") + "\n";
    if text.as_deref() == Some(after.as_str()) {
        return Err("that file already says so; nothing to change".into());
    }

    // The setup after the change, resolved exactly.
    let overlay = OverlayFs { base: fs, files: BTreeMap::from([(path.clone(), after.clone())]) };
    let after_ctx =
        Ctx { fs: &overlay, env: ctx.env.clone(), home: ctx.home.clone(), repo: ctx.repo.clone(), os: ctx.os };
    let later = ClaudeCode.resolve(&after_ctx);

    let mut notes = vec![
        "Takes effect in the next Claude Code session (or after /reload-plugins), not in running ones.".to_string(),
    ];
    match scope {
        Scope::Project => {
            notes.push("Project settings are committed with the repo, so this changes it for everyone.".into())
        }
        Scope::User => notes.push("User settings apply to every repo on this machine.".into()),
        Scope::Local => notes.push("Local settings apply to this repo, for you only.".into()),
    }
    let takes_effect = match change {
        Change::Plugin { id, on, .. } => {
            let pkg = later.packages.iter().find(|x| &x.id == id);
            let ok = pkg.is_some_and(|x| x.enabled == *on);
            if !ok {
                let by = pkg.and_then(|x| x.decided_by.clone()).unwrap_or_default();
                notes.push(format!("No effect: {by} settings, which rank higher, still decide this plugin."));
            }
            ok
        }
        Change::McpJson { name, on, .. } => {
            let state = mcp_state(&later, name);
            let ok = (state == Some(McpState::Active)) == *on;
            if !ok {
                notes.push(format!(
                    "No effect: the server stays {} (another settings file decides it, or policy denies it).",
                    state.map_or("unknown".into(), |s| format!("{s:?}").to_lowercase())
                ));
            }
            ok
        }
    };

    let b = crate::adapters::build::Build::new(ctx, "claude-code");
    Ok(Plan {
        change: change.clone(),
        file: b.display(&path),
        path,
        before_hash: text.as_deref().map(sha256_hex),
        after: after.clone(),
        creates_file,
        diff: diff_lines(text.as_deref().unwrap_or(""), &after),
        tokens_before: before.context.tokens.value,
        tokens_after: later.context.tokens.value,
        takes_effect,
        notes,
    })
}
