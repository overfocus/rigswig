//! Settings layers, their merge, the model decision and permission rules.

use serde_json::{Map, Value};

use super::Paths;
use crate::adapters::build::{Build, calc};
use crate::model::*;
use crate::read::json::{obj_at, str_at, strs_at};

/// One settings file (or the merged managed set), in precedence order: managed, local, project, user.
pub(crate) struct Scope {
    pub id: &'static str,
    pub obj: Map<String, Value>,
}

/// Merge `from` into `into`: objects key by key, arrays combined without duplicates, anything else replaced.
fn merge(into: &mut Value, from: Value) {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        a.insert(k, v);
                    }
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for v in b {
                if !a.contains(&v) {
                    a.push(v);
                }
            }
        }
        (slot, v) => *slot = v,
    }
}

/// Add the five settings layers and read them.
pub(crate) fn layers(b: &mut Build, p: &Paths) -> Vec<Scope> {
    // Managed: the main file, then managed-settings.d/*.json alphabetically.
    let main = p.system.join("managed-settings.json");
    let dropins = p.system.join("managed-settings.d");
    let mut managed = Value::Object(Map::new());
    let mut managed_present = false;
    let mut files = vec![main.clone()];
    files.extend(b.files_in(&dropins, ".json"));
    for f in files {
        let j = b.json(&f);
        managed_present |= j.present();
        if let Some(v) = j.value().filter(|v| v.is_object()) {
            merge(&mut managed, v.clone());
        }
    }
    let read = |b: &mut Build, f: &std::path::Path| {
        let j = b.json(f);
        (j.present(), j.object())
    };
    let (local_present, local) = read(b, &p.repo.join(".claude/settings.local.json"));
    let (project_present, project) = read(b, &p.repo.join(".claude/settings.json"));
    let (user_present, user) = read(b, &p.cfg.join("settings.json"));

    let shell_model = b.ctx.var("ANTHROPIC_MODEL").is_some();
    let sys = b.display(&main);
    b.add_layer("managed", LayerKind::Managed, "managed settings", Some(sys), managed_present, LayerMode::Enforce);
    b.add_layer(
        "session",
        LayerKind::Session,
        "command line and shell (only ANTHROPIC_MODEL is read)",
        None,
        shell_model,
        LayerMode::Default,
    );
    b.add_layer(
        "local",
        LayerKind::Local,
        "local project settings",
        Some(".claude/settings.local.json".into()),
        local_present,
        LayerMode::Default,
    );
    b.add_layer(
        "project",
        LayerKind::Project,
        "project settings",
        Some(".claude/settings.json".into()),
        project_present,
        LayerMode::Default,
    );
    let user_path = b.display(&p.cfg.join("settings.json"));
    b.add_layer("user", LayerKind::User, "user settings", Some(user_path), user_present, LayerMode::Default);

    vec![
        Scope { id: "managed", obj: managed.as_object().cloned().unwrap_or_default() },
        Scope { id: "local", obj: local },
        Scope { id: "project", obj: project },
        Scope { id: "user", obj: user },
    ]
}

/// The model: settings `env.ANTHROPIC_MODEL` (inferred to beat the shell), then the shell's `ANTHROPIC_MODEL`, then
/// the `model` key by precedence. A managed `model` is a default, not a lock.
pub(crate) fn model(b: &mut Build, scopes: &[Scope]) {
    let mut trail = Vec::new();
    for s in scopes {
        if let Some(v) = obj_at(&s.obj, "env").and_then(|e| str_at(e, "ANTHROPIC_MODEL")) {
            trail.push(TrailStep { layer: s.id.into(), value: v.into(), note: Some("env.ANTHROPIC_MODEL".into()) });
        }
    }
    if let Some(v) = b.ctx.var("ANTHROPIC_MODEL") {
        trail.push(TrailStep {
            layer: "session".into(),
            value: v.into(),
            note: Some("ANTHROPIC_MODEL in the shell".into()),
        });
    }
    for s in scopes {
        if let Some(v) = str_at(&s.obj, "model") {
            trail.push(TrailStep { layer: s.id.into(), value: v.into(), note: Some("model".into()) });
        }
    }
    b.note_not_modelled("whether a settings env.ANTHROPIC_MODEL beats the shell's (assumed yes)");
    let decision = match trail.first() {
        Some(t) => Decision { value: t.value.clone(), source: Some(t.layer.clone()), trail: trail.clone() },
        None => Decision { value: Value::Null, source: None, trail },
    };
    b.settings.insert("model".into(), decision);
}

/// Single-value settings decided by precedence (`permissions.defaultMode` is reported as `defaultMode`).
pub(crate) fn scalars(b: &mut Build, scopes: &[Scope]) {
    let keys: &[(&str, &[&str])] = &[
        ("defaultMode", &["permissions", "defaultMode"]),
        ("disableAllHooks", &["disableAllHooks"]),
        ("allowManagedHooksOnly", &["allowManagedHooksOnly"]),
        ("enableAllProjectMcpServers", &["enableAllProjectMcpServers"]),
        ("autoMemoryEnabled", &["autoMemoryEnabled"]),
        ("autoMemoryDirectory", &["autoMemoryDirectory"]),
    ];
    for (name, path) in keys {
        let trail: Vec<TrailStep> = scopes
            .iter()
            .filter_map(|s| {
                let mut v = s.obj.get(path[0])?;
                for k in &path[1..] {
                    v = v.get(k)?;
                }
                (!v.is_null()).then(|| TrailStep { layer: s.id.into(), value: v.clone(), note: None })
            })
            .collect();
        if let Some(first) = trail.first() {
            b.settings.insert(
                (*name).into(),
                Decision { value: first.value.clone(), source: Some(first.layer.clone()), trail: trail.clone() },
            );
        }
    }
}

/// The value a scalar setting resolved to, if any layer set it.
pub(crate) fn resolved<'v>(b: &'v Build, name: &str) -> Option<&'v Value> {
    b.settings.get(name).map(|d| &d.value)
}

/// Every allow, ask and deny rule, one item each; the lists combine across layers, so every rule is effective.
pub(crate) fn permissions(b: &mut Build, scopes: &[Scope]) {
    for s in scopes {
        let Some(perms) = obj_at(&s.obj, "permissions") else { continue };
        for (key, effect) in
            [("deny", PermissionEffect::Deny), ("ask", PermissionEffect::Ask), ("allow", PermissionEffect::Allow)]
        {
            let mut seen = std::collections::BTreeSet::new();
            for rule in strs_at(perms, key) {
                if !seen.insert(rule.clone()) {
                    continue;
                }
                let (tool, pattern) = match rule.split_once('(') {
                    Some((t, rest)) => (t.to_string(), Some(rest.trim_end_matches(')').to_string())),
                    None => (rule.clone(), None),
                };
                let mut it = b.item(
                    s.id,
                    &format!("{key} {rule}"),
                    ItemDetail::Permission(PermissionAttrs { effect, tool, pattern }),
                    LoadWhen::Never,
                    calc(0),
                );
                it.origin.file = b.layers.iter().find(|l| l.id == s.id).and_then(|l| l.path.clone());
                b.push(it);
            }
        }
    }
}
