//! MCP servers: where each is declared, whether it is approved, allowed and not denied, and which wins a name clash.
//! Commands, arguments, env, headers and URL paths are read only to match policy entries; none reach the output.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Map, Value};

use super::Paths;
use super::plugins::Plugin;
use super::settings::Scope;
use crate::adapters::build::{Build, unknown};
use crate::model::*;
use crate::read::json::{obj_at, strs_at};
use crate::read::redact::{safe_base, url_origin};

/// A server as declared, kept private to this module.
struct Decl {
    name: String,
    layer: String,
    file: Option<String>,
    url: Option<String>,
    command: Option<Vec<String>>,
    transport: McpTransport,
    target: Option<String>,
}

fn decls(map: &Map<String, Value>, layer: &str, file: Option<String>) -> Vec<Decl> {
    map.iter()
        .filter_map(|(name, s)| {
            let s = s.as_object()?;
            let ty = s.get("type").and_then(Value::as_str);
            let cmd = s.get("command").and_then(Value::as_str);
            let url = s.get("url").and_then(Value::as_str);
            let transport = match ty {
                Some("stdio") => McpTransport::Stdio,
                Some("http") | Some("streamable-http") => McpTransport::Http,
                Some("sse") => McpTransport::Sse,
                _ if cmd.is_some() => McpTransport::Stdio,
                _ if url.is_some() => McpTransport::Http,
                _ => McpTransport::Unknown,
            };
            let target = match transport {
                McpTransport::Stdio => cmd.and_then(|c| c.split_whitespace().next()).map(safe_base),
                _ => url.map(|u| url_origin(u).unwrap_or_else(|| "(unparsed url)".into())),
            };
            let command = cmd.map(|c| {
                let mut v = vec![c.to_string()];
                v.extend(strs_at(s, "args"));
                v
            });
            Some(Decl {
                name: name.clone(),
                layer: layer.into(),
                file: file.clone(),
                url: url.map(String::from),
                command,
                transport,
                target,
            })
        })
        .collect()
}

/// `*` matches any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(k) => rest = &rest[k + part.len()..],
                None => return false,
            }
        }
    }
    true
}

fn matches(entry: &Value, d: &Decl) -> bool {
    if let Some(n) = entry.get("serverName").and_then(Value::as_str) {
        return n == d.name;
    }
    if let Some(u) = entry.get("serverUrl").and_then(Value::as_str) {
        return d.url.as_deref().is_some_and(|url| glob(u, url));
    }
    if let Some(c) = entry.get("serverCommand").and_then(Value::as_array) {
        let want: Vec<&str> = c.iter().filter_map(Value::as_str).collect();
        return d.command.as_ref().is_some_and(|have| have.iter().map(String::as_str).eq(want.iter().copied()));
    }
    false
}

fn map_of(j: &Map<String, Value>) -> Map<String, Value> {
    obj_at(j, "mcpServers").cloned().unwrap_or_else(|| j.clone())
}

pub(crate) fn all(b: &mut Build, p: &Paths, scopes: &[Scope], plugins: &[Plugin]) {
    let mut all: Vec<Decl> = Vec::new();

    // managed-mcp.json takes exclusive control.
    let managed_file = p.system.join("managed-mcp.json");
    let managed = b.json(&managed_file);
    let exclusive = managed.present();
    if let Some(m) = managed.value().and_then(Value::as_object) {
        let shown = Some(b.display(&managed_file));
        all.extend(decls(&map_of(m), "managed", shown));
    }

    // .claude.json: top-level servers are user scope; the project's entry (by path and real path) is local scope.
    let cj = b.json(&p.claude_json).object();
    let cj_label = Some(b.display(&p.claude_json));
    let mut keys = vec![p.repo.to_string_lossy().into_owned()];
    if let Ok(real) = b.fs().canonicalize(&p.repo) {
        keys.push(real.to_string_lossy().into_owned());
    }
    keys.dedup();
    let projects: Vec<Map<String, Value>> =
        keys.iter().filter_map(|k| obj_at(&cj, "projects").and_then(|ps| obj_at(ps, k)).cloned()).collect();
    for pj in &projects {
        if let Some(m) = obj_at(pj, "mcpServers") {
            all.extend(decls(m, "local", cj_label.clone()));
        }
    }
    let mcp_json = b.json(&p.repo.join(".mcp.json")).object();
    if let Some(m) = obj_at(&mcp_json, "mcpServers") {
        all.extend(decls(m, "project", Some(".mcp.json".into())));
    }
    if let Some(m) = obj_at(&cj, "mcpServers") {
        all.extend(decls(m, "user", cj_label.clone()));
    }
    for pl in plugins {
        let f = pl.dir.join(".mcp.json");
        let mut servers = map_of(&b.json(&f).object());
        for v in pl.manifest_list("mcpServers").unwrap_or_default() {
            match v {
                Value::String(s) => {
                    let lower = s.to_ascii_lowercase();
                    if lower.ends_with(".mcpb")
                        || lower.ends_with(".dxt")
                        || lower.starts_with("http:")
                        || lower.starts_with("https:")
                    {
                        b.warn(
                            "mcp-bundle",
                            format!("plugin {}: an MCP bundle or URL is declared and was not read", pl.id),
                            None,
                        );
                        continue;
                    }
                    let Some(file) = pl.path(b, &s, "mcpServers") else { continue };
                    servers.extend(map_of(&b.json(&file).object()));
                }
                Value::Object(o) => servers.extend(o),
                _ => {}
            }
        }
        let shown = Some(b.display(&f));
        all.extend(decls(&servers, &pl.layer, shown));
    }
    if exclusive {
        b.note_not_modelled("managedMcpServers entries alongside managed-mcp.json");
    }

    // Approval of .mcp.json servers.
    let mut off: BTreeSet<String> = scopes.iter().flat_map(|s| strs_at(&s.obj, "disabledMcpjsonServers")).collect();
    let mut on: BTreeSet<String> = scopes.iter().flat_map(|s| strs_at(&s.obj, "enabledMcpjsonServers")).collect();
    for pj in &projects {
        off.extend(strs_at(pj, "disabledMcpjsonServers"));
        on.extend(strs_at(pj, "enabledMcpjsonServers"));
    }
    let enable_all = scopes
        .iter()
        .find_map(|s| s.obj.get("enableAllProjectMcpServers").and_then(Value::as_bool))
        .or_else(|| projects.iter().rev().find_map(|pj| pj.get("enableAllProjectMcpServers").and_then(Value::as_bool)))
        .unwrap_or(false);

    // Policy lists: denials merge from every layer; allowances too, unless managed keeps them to itself.
    let denied: Vec<Value> = scopes
        .iter()
        .flat_map(|s| s.obj.get("deniedMcpServers").and_then(Value::as_array).cloned().unwrap_or_default())
        .collect();
    let managed_only =
        scopes.iter().any(|s| s.id == "managed" && s.obj.get("allowManagedMcpServersOnly") == Some(&Value::Bool(true)));
    let allow_lists: Vec<&Value> = scopes
        .iter()
        .filter(|s| !managed_only || s.id == "managed")
        .filter_map(|s| s.obj.get("allowedMcpServers"))
        .collect();
    let allowed: Option<Vec<Value>> = if allow_lists.is_empty() {
        None
    } else {
        Some(allow_lists.iter().filter_map(|v| v.as_array()).flatten().cloned().collect())
    };

    let mut idx = Vec::new();
    for d in &all {
        let refused = (exclusive && d.layer != "managed")
            || denied.iter().any(|e| matches(e, d))
            || (d.layer != "managed" && allowed.as_ref().is_some_and(|a| !a.iter().any(|e| matches(e, d))));
        let state = if refused {
            McpState::Denied
        } else if d.layer == "project" {
            if off.contains(&d.name) {
                McpState::Disabled
            } else if enable_all || on.contains(&d.name) {
                McpState::Active
            } else {
                McpState::Pending
            }
        } else {
            McpState::Active
        };
        let attrs = McpAttrs { transport: d.transport, target: d.target.clone(), state };
        let mut it = b.item(&d.layer, &d.name, ItemDetail::Mcp(attrs), LoadWhen::Startup, unknown());
        it.origin.file = d.file.clone();
        if state != McpState::Active {
            Build::disable(&mut it);
        }
        idx.push(b.push(it));
    }
    let pending = all
        .iter()
        .zip(&idx)
        .filter(|(_, i)| matches!(&b.items[**i].detail, ItemDetail::Mcp(m) if m.state == McpState::Pending))
        .count();
    if pending > 0 {
        b.warn(
            "mcp-pending",
            format!("{pending} project MCP server{} waiting for approval", if pending == 1 { " is" } else { "s are" }),
            Some(Path::new(".mcp.json")),
        );
    }
    let order = |l: &str| match l {
        "managed" => 0,
        "local" => 1,
        "project" => 2,
        "user" => 3,
        _ => 4,
    };
    b.resolve_clashes(&idx, |i| i.name.clone(), |i| order(&i.origin.layer));
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("https://*.example.com/*", "https://api.example.com/mcp"));
        assert!(!glob("https://*.example.com/*", "https://example.org/mcp"));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
    }
}
