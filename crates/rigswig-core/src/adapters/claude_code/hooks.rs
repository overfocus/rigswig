//! Hooks from every settings layer and every enabled plugin, and the two switches that turn them off.

use serde_json::Value;

use super::plugins::Plugin;
use super::settings::{Scope, resolved};
use crate::adapters::build::Build;
use crate::adapters::shared::hooks_of;
use crate::read::json::obj_at;

pub(crate) fn all(b: &mut Build, scopes: &[Scope], plugins: &[Plugin]) {
    let mut by_layer: Vec<(String, Vec<usize>, bool)> = Vec::new(); // (layer, items, exempt from managed-only)
    for s in scopes {
        let file = b.layers.iter().find(|l| l.id == s.id).and_then(|l| l.path.clone());
        if let Some(block) = obj_at(&s.obj, "hooks").cloned() {
            let idx = hooks_of(b, &block, s.id, file, "");
            by_layer.push((s.id.into(), idx, s.id == "managed"));
        }
    }
    for pl in plugins {
        let mut idx = Vec::new();
        let default = pl.dir.join("hooks/hooks.json");
        let j = b.json(&default).object();
        if let Some(block) = obj_at(&j, "hooks").cloned() {
            let f = Some(b.display(&default));
            idx.extend(hooks_of(b, &block, &pl.layer, f, ""));
        }
        for v in pl.manifest_list("hooks").unwrap_or_default() {
            match v {
                Value::String(rel) => {
                    let Some(f) = pl.path(b, &rel, "hooks") else { continue };
                    let j = b.json(&f).object();
                    if let Some(block) = obj_at(&j, "hooks").cloned() {
                        let shown = Some(b.display(&f));
                        idx.extend(hooks_of(b, &block, &pl.layer, shown, ""));
                    }
                }
                Value::Object(block) => {
                    let block = obj_at(&block, "hooks").cloned().unwrap_or(block);
                    idx.extend(hooks_of(b, &block, &pl.layer, Some(".claude-plugin/plugin.json".into()), ""));
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
