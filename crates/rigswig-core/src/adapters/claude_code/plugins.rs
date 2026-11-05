//! Plugins: which are enabled and by which layer, where each one's folder is, and confinement of manifest paths.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use super::Paths;
use super::settings::Scope;
use crate::adapters::build::Build;
use crate::fs::is_within;
use crate::model::*;
use crate::read::json::{obj_at, str_at};
use crate::read::redact::printable;

/// An enabled plugin whose folder was found.
pub(crate) struct Plugin {
    pub id: String,
    /// The plugin's name, used as the namespace of everything it contributes.
    pub ns: String,
    pub layer: String,
    pub dir: PathBuf,
    canon: PathBuf,
    pub manifest: Map<String, Value>,
    /// Managed `enabledPlugins` sets it to true: its hooks survive `allowManagedHooksOnly`.
    pub forced_by_managed: bool,
}

fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            o => out.push(o.as_os_str()),
        }
    }
    out
}

impl Plugin {
    /// `rel` inside the plugin folder, lexically and through symlinks; `None` if it escapes.
    pub fn confine(&self, b: &Build, rel: &Path) -> Option<PathBuf> {
        let p = lexical(&self.dir.join(rel));
        if !p.starts_with(&self.dir) {
            return None;
        }
        if b.fs().exists(&p) {
            let real = b.fs().canonicalize(&p).ok()?;
            if !is_within(&real, &self.canon) {
                return None;
            }
        }
        Some(p)
    }

    /// Like `confine`, warning (without quoting the path) when it escapes.
    pub fn path(&self, b: &mut Build, rel: &str, what: &str) -> Option<PathBuf> {
        let p = self.confine(b, Path::new(rel));
        if p.is_none() {
            b.warn(
                "plugin-path-escape",
                format!("plugin {}: a {what} path leaves the plugin folder, ignored", self.id),
                None,
            );
        }
        p
    }

    /// The manifest's value for `key` as a list (a single value becomes one entry).
    pub fn manifest_list(&self, key: &str) -> Option<Vec<Value>> {
        match self.manifest.get(key)? {
            Value::Array(a) => Some(a.clone()),
            Value::Null => None,
            v => Some(vec![v.clone()]),
        }
    }
}

fn names_in(b: &Build, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for k in ["agents", "skills"] {
        for e in b.fs().read_dir(&dir.join(k)).unwrap_or_default() {
            out.push(format!("{k}/{}", e.name));
        }
    }
    out
}

pub(crate) fn discover(b: &mut Build, p: &Paths, scopes: &[Scope]) -> Vec<Plugin> {
    // Marketplaces: the known list, then extraKnownMarketplaces from settings.
    let known = b.json(&p.cfg.join("plugins/known_marketplaces.json")).object();
    let installed = b.json(&p.cfg.join("plugins/installed_plugins.json")).object();
    let installed = obj_at(&installed, "plugins").cloned().unwrap_or_default();
    let mut markets: BTreeMap<String, Map<String, Value>> =
        known.into_iter().filter_map(|(k, v)| v.as_object().cloned().map(|o| (k, o))).collect();
    for s in scopes.iter().rev() {
        if let Some(extra) = obj_at(&s.obj, "extraKnownMarketplaces") {
            for (k, v) in extra {
                if let Some(o) = v.as_object() {
                    markets.entry(k.clone()).or_insert_with(|| o.clone());
                }
            }
        }
    }

    // enabledPlugins: the highest layer that mentions an id decides.
    let mut states: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    let mut decided: BTreeMap<String, String> = BTreeMap::new();
    for s in scopes {
        if let Some(en) = obj_at(&s.obj, "enabledPlugins") {
            for (id, v) in en {
                let on = v.as_bool().unwrap_or(false);
                states.entry(id.clone()).or_default().insert(s.id.into(), on);
                decided.entry(id.clone()).or_insert_with(|| s.id.to_string());
            }
        }
    }

    let mut out = Vec::new();
    for (id, scopes_said) in states {
        let decider = decided[&id].clone();
        let enabled = scopes_said[&decider];
        let (name, market) = id.split_once('@').unwrap_or((id.as_str(), ""));

        // The folder: a directory marketplace is read from its own folder; otherwise the install cache.
        let entries: Vec<Map<String, Value>> = installed
            .get(&id)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|e| e.as_object().cloned()).collect())
            .unwrap_or_default();
        let repo_s = p.repo.to_string_lossy();
        let entry = entries
            .iter()
            .find(|e| str_at(e, "projectPath").is_some_and(|pp| pp == repo_s))
            .or_else(|| entries.iter().find(|e| str_at(e, "scope") == Some("user")))
            .or(entries.first());
        let cache = entry.and_then(|e| str_at(e, "installPath")).map(PathBuf::from).filter(|d| b.fs().exists(d));
        let m = markets.get(market);
        let src = m.and_then(|m| obj_at(m, "source"));
        let is_dir_market = src.and_then(|s| str_at(s, "source")) == Some("directory");
        let root = if is_dir_market {
            src.and_then(|s| str_at(s, "path")).map(PathBuf::from)
        } else {
            m.and_then(|m| str_at(m, "installLocation")).map(PathBuf::from)
        };
        let mut from_market = None;
        if let Some(root) = root.filter(|r| b.fs().exists(r)) {
            let mj = b.json(&root.join(".claude-plugin/marketplace.json")).object();
            let entry = mj
                .get("plugins")
                .and_then(Value::as_array)
                .and_then(|ps| ps.iter().find(|x| x.get("name").and_then(Value::as_str) == Some(name)));
            if let Some(source) = entry.and_then(|e| e.get("source")).and_then(Value::as_str) {
                let d = lexical(&root.join(source));
                if b.fs().exists(&d) {
                    from_market = Some(d);
                }
            }
        }
        let dir = match (&from_market, &cache) {
            (Some(d), c) if is_dir_market => {
                if let Some(c) = c {
                    if names_in(b, c) != names_in(b, d) {
                        b.warn(
                            "stale-plugin-cache",
                            format!(
                                "plugin {id}: the install cache differs from the marketplace folder and is ignored"
                            ),
                            None,
                        );
                    }
                }
                Some(d.clone())
            }
            (_, Some(c)) => Some(c.clone()),
            (Some(d), None) => Some(d.clone()),
            (None, None) => None,
        };

        let layer = format!("plugin:{id}");
        b.packages.push(Package {
            id: printable(&id),
            kind: PackageKind::Plugin,
            source: printable(if market.is_empty() { "(no marketplace)" } else { market }),
            enabled,
            decided_by: Some(decider.clone()),
            scopes: scopes_said.clone(),
            contributes: Vec::new(),
        });
        if !enabled {
            continue;
        }
        let Some(dir) = dir else {
            b.warn("plugin-missing", format!("plugin {id} is enabled but its folder was not found"), None);
            continue;
        };
        let canon = b.fs().canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        let shown = b.display(&dir);
        b.add_layer(&layer, LayerKind::Package, &format!("plugin {id}"), Some(shown), true, LayerMode::Default);
        let manifest = b.json(&dir.join(".claude-plugin/plugin.json")).object();
        let ns = name.to_string();
        out.push(Plugin {
            forced_by_managed: scopes_said.get("managed") == Some(&true),
            id: id.clone(),
            ns,
            layer,
            dir,
            canon,
            manifest,
        });
    }
    out
}

/// After all items are in: fill each package's `contributes` and set `origin.package` on its items.
pub(crate) fn link_contributions(b: &mut Build, plugins: &[Plugin]) {
    for pl in plugins {
        let pid = printable(&pl.id);
        let ids: Vec<String> = b
            .items
            .iter_mut()
            .filter(|i| i.origin.layer == pl.layer)
            .map(|i| {
                i.origin.package = Some(pid.clone());
                i.id.clone()
            })
            .collect();
        if let Some(pkg) = b.packages.iter_mut().find(|p| p.id == pid) {
            pkg.contributes = ids;
        }
    }
}
