//! What every adapter does the same way: path labels, item ids, warnings, shadowing and the context total.

use std::collections::BTreeMap;
use std::path::Path;

use crate::adapter::Ctx;
use crate::fs::{EntryKind, Fs};
use crate::model::*;
use crate::read::json::{JsonFile, read_json, read_jsonc, read_toml};
use crate::read::redact::printable;

pub struct Build<'a> {
    pub ctx: &'a Ctx<'a>,
    pub tool: &'static str,
    pub layers: Vec<Layer>,
    pub packages: Vec<Package>,
    pub items: Vec<Item>,
    pub settings: BTreeMap<String, Decision>,
    pub warnings: Vec<Warning>,
    pub not_measured: Vec<NotMeasured>,
    pub not_modelled: Vec<String>,
    pub detected_by: Vec<String>,
}

impl<'a> Build<'a> {
    pub fn new(ctx: &'a Ctx<'a>, tool: &'static str) -> Self {
        Self {
            ctx,
            tool,
            layers: Vec::new(),
            packages: Vec::new(),
            items: Vec::new(),
            settings: BTreeMap::new(),
            warnings: Vec::new(),
            not_measured: Vec::new(),
            not_modelled: Vec::new(),
            detected_by: Vec::new(),
        }
    }

    pub fn fs(&self) -> &'a dyn Fs {
        self.ctx.fs
    }

    /// A path as shown to people: repo-relative, `~/...`, or absolute (system files).
    pub fn display(&self, p: &Path) -> String {
        let s = if let Ok(r) = p.strip_prefix(&self.ctx.repo) {
            let r = r.to_string_lossy().replace('\\', "/");
            if r.is_empty() { ".".into() } else { r }
        } else if let Ok(r) = p.strip_prefix(&self.ctx.home) {
            format!("~/{}", r.to_string_lossy().replace('\\', "/"))
        } else {
            p.to_string_lossy().into_owned()
        };
        printable(&s)
    }

    pub fn warn(&mut self, code: &str, message: impl Into<String>, file: Option<&Path>) {
        let file = file.map(|f| self.display(f));
        let w = Warning { code: code.into(), message: printable(&message.into()), item: None, file };
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    pub fn note_not_modelled(&mut self, what: impl Into<String>) {
        let w = what.into();
        if !self.not_modelled.contains(&w) {
            self.not_modelled.push(w);
        }
    }

    /// Read JSON, warning (without quoting content) when it is malformed.
    pub fn json(&mut self, path: &Path) -> JsonFile {
        let j = read_json(self.fs(), path);
        if matches!(j, JsonFile::Malformed) {
            self.warn("malformed-json", "not valid JSON, skipped", Some(path));
        }
        j
    }

    /// Read JSON *with comments* (Devin CLI's config format), warning when malformed after stripping.
    pub fn jsonc(&mut self, path: &Path) -> JsonFile {
        let j = read_jsonc(self.fs(), path);
        if matches!(j, JsonFile::Malformed) {
            self.warn("malformed-jsonc", "not valid JSON (comments allowed), skipped", Some(path));
        }
        j
    }

    /// Read TOML, warning (without quoting content) when it is malformed.
    pub fn toml(&mut self, path: &Path) -> JsonFile {
        let j = read_toml(self.fs(), path);
        if matches!(j, JsonFile::Malformed) {
            self.warn("malformed-toml", "not valid TOML, skipped", Some(path));
        }
        j
    }

    /// Warn per YAML feature a [`crate::read::yaml::YamlFile`] dropped, and say when the file itself is broken.
    pub fn warn_yaml(&mut self, f: &crate::read::yaml::YamlFile, what: &str) {
        if f.value.is_none() && f.notes.is_empty() {
            return;
        }
        for n in &f.notes {
            self.warn("yaml-feature-skipped", format!("{what}: {n}; that entry is skipped"), None);
        }
        if f.value.is_none() {
            self.warn("malformed-yaml", format!("{what} is not valid config YAML, skipped"), None);
        }
    }

    pub fn add_layer(
        &mut self,
        id: &str,
        kind: LayerKind,
        label: &str,
        path: Option<String>,
        present: bool,
        mode: LayerMode,
    ) {
        let rank = self.layers.len() as u32;
        self.layers.push(Layer { id: id.into(), kind, label: label.into(), path, present, rank, mode });
    }

    pub fn set_present(&mut self, layer: &str) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == layer) {
            l.present = true;
        }
    }

    pub fn rank_of(&self, layer: &str) -> u32 {
        self.layers.iter().find(|l| l.id == layer).map_or(u32::MAX, |l| l.rank)
    }

    /// A new, effective item. Names are made printable; the id is `<tool>:<kind>:<layer>:<name>`.
    pub fn item(&self, layer: &str, name: &str, detail: ItemDetail, when: LoadWhen, chars: Measure) -> Item {
        let name = printable(name);
        Item {
            id: format!("{}:{}:{}:{}", self.tool, detail.kind(), layer, name),
            name,
            origin: Origin { layer: layer.into(), package: None, file: None, line: None },
            effective: true,
            shadowed_by: None,
            load: Load { when, chars, content_hash: None },
            detail,
        }
    }

    /// Add an item, returning its index. A second item with the same id in one layer gets `#2`, `#3`... and is
    /// shadowed by the first (which of two same-named files a tool picks is not documented), with a warning.
    pub fn push(&mut self, mut item: Item) -> usize {
        if let Some(first) = self.items.iter().position(|i| i.id == item.id) {
            let base = item.id.clone();
            let mut n = 2;
            while self.items.iter().any(|i| i.id == format!("{base}#{n}")) {
                n += 1;
            }
            item.id = format!("{base}#{n}");
            let winner = self.items[first].id.clone();
            self.warn(
                "duplicate-name",
                format!("two {}s named {:?} in one place; the second is ignored", item.detail.kind(), item.name),
                None,
            );
            Self::shadow(&mut item, &winner);
        }
        self.items.push(item);
        self.items.len() - 1
    }

    pub fn shadow(item: &mut Item, by: &str) {
        item.effective = false;
        item.shadowed_by = Some(by.into());
        item.load.when = LoadWhen::Never;
    }

    pub fn disable(item: &mut Item) {
        item.effective = false;
        item.load.when = LoadWhen::Never;
    }

    /// Among `idx`, items with the same `key` compete: the effective one with the lowest `priority` wins and the rest
    /// are shadowed by it. Ties keep the first.
    pub fn resolve_clashes(&mut self, idx: &[usize], key: impl Fn(&Item) -> String, priority: impl Fn(&Item) -> u32) {
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &i in idx {
            groups.entry(key(&self.items[i])).or_default().push(i);
        }
        for (_, g) in groups {
            let Some(&win) =
                g.iter().filter(|&&i| self.items[i].effective).min_by_key(|&&i| (priority(&self.items[i]), i))
            else {
                continue;
            };
            let win_id = self.items[win].id.clone();
            for &i in &g {
                if i != win && self.items[i].effective {
                    Self::shadow(&mut self.items[i], &win_id);
                }
            }
        }
    }

    /// Files (and links to files) in `dir` whose name ends with `ext`, sorted.
    pub fn files_in(&self, dir: &Path, ext: &str) -> Vec<std::path::PathBuf> {
        self.fs()
            .read_dir(dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.kind != EntryKind::Dir && e.name.ends_with(ext) && !e.name.starts_with('.'))
            .map(|e| dir.join(e.name))
            .collect()
    }

    /// Folders (and links to folders) in `dir`, sorted.
    pub fn dirs_in(&self, dir: &Path) -> Vec<std::path::PathBuf> {
        self.fs()
            .read_dir(dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.name.starts_with('.'))
            .map(|e| dir.join(e.name))
            .filter(|p| self.fs().read_dir(p).is_ok())
            .collect()
    }

    /// Every file ending with `ext` under `dir`, recursively (real folders only, so link loops cannot recur), with
    /// its path relative to `dir`.
    pub fn walk(&self, dir: &Path, ext: &str) -> Vec<(std::path::PathBuf, String)> {
        fn go(
            b: &Build,
            root: &Path,
            dir: &Path,
            ext: &str,
            depth: usize,
            out: &mut Vec<(std::path::PathBuf, String)>,
        ) {
            if depth > 8 {
                return;
            }
            for e in b.fs().read_dir(dir).unwrap_or_default() {
                if e.name.starts_with('.') {
                    continue;
                }
                let p = dir.join(&e.name);
                match e.kind {
                    EntryKind::Dir => go(b, root, &p, ext, depth + 1, out),
                    _ if e.name.ends_with(ext) => {
                        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                        out.push((p, rel));
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        go(self, dir, dir, ext, 0, &mut out);
        out
    }

    /// Fill each package's `contributes` from the items in its layer (`plugin:<id>`), and set their `origin.package`.
    pub fn link_packages(&mut self) {
        for pkg in &mut self.packages {
            let layer = format!("plugin:{}", pkg.id);
            pkg.contributes = self
                .items
                .iter_mut()
                .filter(|i| i.origin.layer == layer)
                .map(|i| {
                    i.origin.package = Some(pkg.id.clone());
                    i.id.clone()
                })
                .collect();
        }
    }

    /// The unmeasured costs every harness shares: the tool listings of each effective MCP server, named (a server with
    /// hundreds of tools adds a long listing even when its tools load on demand), and the text SessionStart hooks print
    /// into the context, which rigswig cannot know without running them.
    pub fn not_measured_common(&mut self) {
        let mut servers: Vec<&str> =
            self.items.iter().filter(|i| i.effective && i.detail.kind() == "mcp").map(|i| i.name.as_str()).collect();
        servers.sort_unstable();
        servers.dedup();
        if !servers.is_empty() {
            const SHOWN: usize = 6;
            let mut names = servers.iter().take(SHOWN).copied().collect::<Vec<_>>().join(", ");
            if servers.len() > SHOWN {
                names.push_str(&format!(" and {} more", servers.len() - SHOWN));
            }
            let n = servers.len();
            self.not_measured.push(NotMeasured {
                what: format!("tool listings of {n} MCP server{} ({names})", if n == 1 { "" } else { "s" }),
                why: "rigswig reads server names, never connects to them".into(),
            });
        }
        let starts = self
            .items
            .iter()
            .filter(|i| i.effective && matches!(&i.detail, ItemDetail::Hook(h) if h.event == "session-start"))
            .count();
        if starts > 0 {
            self.not_measured.push(NotMeasured {
                what: format!(
                    "text printed by {starts} SessionStart hook{}, which joins the context",
                    if starts == 1 { "" } else { "s" }
                ),
                why: "rigswig never runs hooks, so their output is unknown".into(),
            });
        }
    }

    /// Assemble the harness: the context total from the effective startup items, and the coverage list.
    pub fn finish(self, adapter_version: &str, tool_version_assumed: Option<&str>, repo_label: String) -> Harness {
        let chars: u64 = self
            .items
            .iter()
            .filter(|i| i.effective && i.load.when == LoadWhen::Startup)
            .map(|i| i.load.chars.value)
            .sum();
        Harness {
            schema_version: SCHEMA_VERSION.into(),
            tool: Tool {
                id: self.tool.into(),
                adapter_version: adapter_version.into(),
                tool_version_assumed: tool_version_assumed.map(String::from),
                detected_by: self.detected_by,
            },
            target: Target { repo: repo_label, os: self.ctx.os },
            layers: self.layers,
            packages: self.packages,
            items: self.items,
            settings: self.settings,
            context: ContextEstimate {
                chars: Measure { value: chars, confidence: Confidence::Calculated },
                tokens: estimate_tokens(chars),
                not_measured: self.not_measured,
            },
            coverage: Coverage { not_modelled: self.not_modelled },
            warnings: self.warnings,
        }
    }
}

/// Characters as people count them (not bytes).
pub fn char_len(s: &str) -> u64 {
    s.chars().count() as u64
}

pub fn calc(n: u64) -> Measure {
    Measure { value: n, confidence: Confidence::Calculated }
}

pub fn unknown() -> Measure {
    Measure { value: 0, confidence: Confidence::Unknown }
}
