//! The Hermes Agent adapter. Every rule it models is in `docs/adapters/hermes.md`, with the docs it comes from.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection, SessionClaim};
use crate::adapters::build::{Build, calc, char_len, unknown};
use crate::fs::Fs;
use crate::model::*;
use crate::read::frontmatter;
use crate::read::redact::{printable, safe_base, sha256_hex, url_origin};
use crate::read::yaml;

pub const ID: &str = "hermes";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The Hermes Agent release whose documents this adapter follows (read 2026-11-06).
const TOOL_VERSION: &str = "0.22.5";
/// The context-file cap when `context_file_max_chars` does not pin one: the floor of the dynamic cap.
const CONTEXT_FILE_CAP: u64 = 20_000;
/// Folders the project walk never enters (build and dependency output; the Hermes docs name no list, so this list
/// is carried over from the other adapters and marked inferred).
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "coverage",
    "__pycache__",
    "venv",
    ".venv",
    "Pods",
    "DerivedData",
    ".git",
    ".hg",
    ".svn",
    "site-packages",
];
const MAX_WALK_DEPTH: usize = 12;
/// The walker gives up after this many folders, with a note.
const MAX_WALKED: usize = 20_000;

pub struct Hermes;

/// Which shape a settings value has, and how it is redacted.
#[derive(Clone, Copy)]
enum Kind {
    Text,
    Count,
    Url,
}

struct Setting {
    name: &'static str,
    path: &'static [&'static str],
    kind: Kind,
}

/// The settings rigswig resolves, each with its layer trail. Only non-secret keys: secrets live in `.env`, which
/// this adapter never opens, and `model.api_key` is not among the model keys read.
const SETTINGS: &[Setting] = &[
    Setting { name: "model", path: &["model", "default"], kind: Kind::Text },
    Setting { name: "provider", path: &["model", "provider"], kind: Kind::Text },
    Setting { name: "modelBaseUrl", path: &["model", "base_url"], kind: Kind::Url },
    Setting { name: "contextLength", path: &["model", "context_length"], kind: Kind::Count },
    Setting { name: "approvalMode", path: &["approvals", "mode"], kind: Kind::Text },
    Setting { name: "contextFileMaxChars", path: &["context_file_max_chars"], kind: Kind::Count },
    Setting { name: "memoryCharLimit", path: &["memory", "memory_char_limit"], kind: Kind::Count },
    Setting { name: "userCharLimit", path: &["memory", "user_char_limit"], kind: Kind::Count },
];

/// The value at a dotted path under a config mapping.
fn at<'a>(cfg: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    let mut node = cfg.get(*path.first()?)?;
    for key in &path[1..] {
        node = node.as_object()?.get(*key)?;
    }
    Some(node)
}

fn cfg_str(cfg: &Map<String, Value>, path: &[&str]) -> Option<String> {
    at(cfg, path).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn cfg_count(cfg: &Map<String, Value>, path: &[&str]) -> Option<u64> {
    at(cfg, path).and_then(Value::as_i64).filter(|n| *n > 0).map(|n| n as u64)
}

fn cfg_strs(cfg: &Map<String, Value>, path: &[&str]) -> Vec<String> {
    at(cfg, path)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

/// False only when the key is set to an explicit `false`.
fn cfg_on(cfg: &Map<String, Value>, path: &[&str]) -> bool {
    at(cfg, path).and_then(Value::as_bool).unwrap_or(true)
}

/// A JSON array of strings.
fn list_of(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).map(String::from).collect())
        .unwrap_or_default()
}

/// `~/x` becomes `<home>/x`; a bare relative value is taken against the home folder (marked inferred).
fn expand(home: &Path, value: &str) -> PathBuf {
    let v = value.trim();
    match v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None if v == "~" => home.to_path_buf(),
        None => {
            let p = PathBuf::from(v);
            if p.is_absolute() { p } else { home.join(p) }
        }
    }
}

/// The platform name a `platforms` frontmatter list uses, when one applies to this machine.
fn os_name(os: Os) -> Option<&'static str> {
    match os {
        Os::Macos => Some("macos"),
        Os::Linux => Some("linux"),
        Os::Windows => Some("windows"),
        Os::Other => None,
    }
}

/// The characters of `text` before `cap`, and whether anything was cut.
fn kept(text: &str, cap: u64) -> String {
    let end = text.char_indices().nth(cap as usize).map_or(text.len(), |(i, _)| i);
    text[..end].to_string()
}

/// A walking budget shared by one scan: 20,000 folders at most.
struct Walk {
    left: usize,
    exhausted: bool,
}

impl Walk {
    fn new() -> Self {
        Self { left: MAX_WALKED, exhausted: false }
    }

    /// One folder counted. False when the budget ran out (the walk stops there).
    fn tick(&mut self) -> bool {
        if self.left == 0 {
            self.exhausted = true;
            return false;
        }
        self.left -= 1;
        true
    }
}

/// All the file paths an adapter may look at, resolved once.
struct Paths {
    home: PathBuf,
    /// `HERMES_HOME`, or the default for the OS.
    hermes_home: PathBuf,
    /// Where a session actually stands: a profile's home when one is named, else the Hermes home.
    active: PathBuf,
    profile: Option<String>,
    managed: Option<PathBuf>,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let home = ctx.home.clone();
        let hermes_home = match ctx.var("HERMES_HOME") {
            Some(v) => expand(&home, v),
            None => match ctx.os {
                Os::Windows => {
                    ctx.var("LOCALAPPDATA").map_or_else(|| home.join(".hermes"), |d| expand(&home, d).join("hermes"))
                }
                _ => home.join(".hermes"),
            },
        };
        let profile = ctx.var("HERMES_PROFILE").map(String::from);
        let active = match &profile {
            Some(name) => {
                let dir = hermes_home.join("profiles").join(name);
                if ctx.fs.exists(&dir) { dir } else { hermes_home.clone() }
            }
            None => hermes_home.clone(),
        };
        let managed = match ctx.os {
            Os::Windows => None,
            _ => Some(ctx.var("HERMES_MANAGED_DIR").map_or_else(|| PathBuf::from("/etc/hermes"), |d| expand(&home, d))),
        };
        Self { home, hermes_home, active, profile, managed, repo: ctx.repo.clone() }
    }
}

/// Merge two config layers: the managed value wins wherever the managed file names the key (leaf-level).
fn overlay(user: Map<String, Value>, managed: Map<String, Value>) -> Map<String, Value> {
    fn merge(into: &mut Value, from: &Value) {
        match (into, from) {
            (Value::Object(a), Value::Object(b)) => {
                for (k, v) in b {
                    match a.get_mut(k) {
                        Some(slot) => merge(slot, v),
                        None => {
                            a.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            (slot, v) => *slot = v.clone(),
        }
    }
    let mut merged = Value::Object(user);
    merge(&mut merged, &Value::Object(managed));
    merged.as_object().cloned().unwrap_or_default()
}

struct Run<'a, 'b> {
    b: &'b mut Build<'a>,
    p: Paths,
    cfg_user: Map<String, Value>,
    cfg_managed: Map<String, Value>,
    cfg_merged: Map<String, Value>,
    cap: u64,
}

impl Adapter for Hermes {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &[
            "HERMES_HOME",
            "HERMES_PROFILE",
            "HERMES_MANAGED_DIR",
            "HERMES_INFERENCE_MODEL",
            "HERMES_MODEL",
            "LOCALAPPDATA",
        ]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let repo = &ctx.repo;
        let in_repo = |name: &str| ctx.fs.exists(&repo.join(name));
        let checks = [
            // The home folder alone scores at most 50 (see `Detection`); the repo's own Hermes files score above it.
            (ctx.fs.exists(&p.active.join("config.yaml")), "config.yaml", 50),
            (ctx.fs.exists(&p.active.join("skills")), "skills/", 45),
            (ctx.fs.exists(&p.active.join("SOUL.md")), "SOUL.md", 40),
            (ctx.fs.exists(&p.active), "Hermes home", 45),
            (in_repo(".hermes.md") || in_repo("HERMES.md"), ".hermes.md", 100),
            (in_repo(".hermes/skills"), ".hermes/skills/", 60),
            // `.agents/skills` is shared with Codex, so it is a hint, not a claim.
            (in_repo(".agents/skills"), ".agents/skills/", 25),
            (in_repo("AGENTS.md"), "AGENTS.md", 15),
            (p.profile.is_some(), "profile selected", 20),
        ];
        let mut score = 0;
        let mut evidence = Vec::new();
        for (found, label, s) in checks {
            if found {
                score = score.max(s);
                evidence.push(label.to_string());
            }
        }
        Detection { score, evidence }
    }

    /// The session look: the Hermes home's own activity files (`docs/adapters/hermes.md`, "Live sessions").
    /// Metadata only — the one place a session's repo is named (`state.db`) is never opened, so the claim is
    /// machine-scoped: it cannot say which repo a live session sits in.
    fn session(&self, ctx: &Ctx) -> Option<SessionClaim> {
        let p = Paths::new(ctx);
        let mut latest = None;
        let mut seen: Vec<String> = Vec::new();
        let mut look = |dir: &Path, tag: &str| {
            let mut names = Vec::new();
            for name in ["state.db-wal", "state.db", "gateway_state.json", ".hermes_history"] {
                let f = dir.join(name);
                if !ctx.fs.exists(&f) {
                    continue;
                }
                names.push(name);
                if let Ok(t) = ctx.fs.modified(&f) {
                    latest = latest.max(Some(t));
                }
            }
            if !names.is_empty() {
                seen.push(format!("{tag}: {}", names.join(", ")));
            }
        };
        look(p.hermes_home.as_path(), "Hermes home");
        if p.active != p.hermes_home {
            look(p.active.as_path(), "Hermes home (profile)");
        }
        if seen.is_empty() {
            return None;
        }
        Some(SessionClaim { scope: SessionScope::Machine, latest, evidence: seen })
    }

    fn resolve(&self, ctx: &Ctx) -> Harness {
        let mut b = Build::new(ctx, ID);
        b.detected_by = self.detect(ctx).evidence;
        let p = Paths::new(ctx);
        let user_yaml = yaml::read_yaml(ctx.fs, &p.active.join("config.yaml"));
        b.warn_yaml(&user_yaml, "config.yaml");
        let managed_yaml = match &p.managed {
            Some(d) => yaml::read_yaml(ctx.fs, &d.join("config.yaml")),
            None => yaml::YamlFile { value: None, notes: Vec::new() },
        };
        b.warn_yaml(&managed_yaml, "managed config");
        if let Some(name) = &p.profile {
            if p.active == p.hermes_home {
                b.warn(
                    "profile-unresolved",
                    format!("the profile {name} has no home folder, so the main Hermes home is used"),
                    None,
                );
            }
        }
        let cfg_merged = overlay(user_yaml.object(), managed_yaml.object());
        let mut run = Run {
            b: &mut b,
            p,
            cfg_user: user_yaml.object(),
            cfg_managed: managed_yaml.object(),
            cfg_merged,
            cap: CONTEXT_FILE_CAP,
        };
        if let Some(pin) = cfg_count(&run.cfg_merged, &["context_file_max_chars"]) {
            run.cap = pin;
        } else {
            run.b.note_not_modelled(
                "the context-file cap scales with the model's context window (20K..500K chars) when context_file_max_chars is unset; rigswig models the 20K floor",
            );
        }
        run.layers();
        run.settings();
        run.instructions();
        run.soul();
        run.memory();
        run.skills();
        run.plugins();
        run.mcp();

        run.b.not_measured.push(NotMeasured {
            what: "the built-in system prompt and the built-in tools' schemas".into(),
            why: "they come with Hermes, not from any file".into(),
        });
        run.b.not_measured.push(NotMeasured {
            what: "the header and `§` separators the memory block wraps your entries in".into(),
            why: "rigswig measures the entry files, not their rendering".into(),
        });
        run.b.not_measured_common();
        for note in [
            "other flags and their variables: -p/--profile as a command flag, --ignore-rules and HERMES_IGNORE_RULES (which also skips SOUL.md, plugins and MCP servers)",
            "profiles are resolved from $HERMES_PROFILE as profiles/<name> when it exists (inferred; the docs document -p, not the variable)",
            "per-skill conditional activation (requires_toolsets, fallback_for_tools), Skills Hub quarantine verdicts and curator locks",
            "skill settings (skills.config) injected when a skill loads, and ${VAR} entries in skills.external_dirs",
            "plugin tools, slash commands, skills and hooks registered in Python (plugin.yaml is the only manifest rigswig reads)",
            "built-in plugins that ship inside the Hermes install and run automatically (they are not in the plugins folder)",
            "MCP tool filtering (tools.include/exclude), sampling, elicitation, events, spend limits, OAuth tokens (mcp-tokens/) and trust tiers",
            "cron jobs, loops, heartbeats, kanban and webhooks (they drive runs; they load nothing before your first message)",
            "context-file read timeouts: a file too slow to read is reported absent, exactly as Hermes would show nothing for it",
            "memory providers other than the built-in files (when memory.provider names one)",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn Fs {
        self.b.fs()
    }

    fn ctx_var(&self, name: &str) -> Option<String> {
        self.b.ctx.env.get(name).map(|v| printable(v))
    }

    fn layers(&mut self) {
        let managed_path = self
            .p
            .managed
            .as_ref()
            .filter(|d| self.fs().exists(&d.join("config.yaml")))
            .map(|d| self.b.display(&d.join("config.yaml")));
        let managed_present =
            self.p.managed.as_ref().map(|d| self.fs().exists(&d.join("config.yaml"))).unwrap_or(false);
        if let Some(d) = &self.p.managed {
            if !managed_present {
                self.b.note_not_modelled(
                    "there is no managed scope here (/etc/hermes, or $HERMES_MANAGED_DIR, carries no managed config.yaml); everything resolves as an unmanaged install",
                );
                let _ = d;
            }
        }
        self.b.add_layer(
            "managed",
            LayerKind::Managed,
            "managed scope (admin-pinned)",
            managed_path,
            managed_present,
            LayerMode::Enforce,
        );
        self.b.add_layer(
            "session",
            LayerKind::Session,
            "command line and shell environment (rigswig reads only the variables Hermes documents)",
            None,
            false,
            LayerMode::Default,
        );
        let project = ["AGENTS.md", "CLAUDE.md", ".cursorrules", ".hermes.md", "HERMES.md", ".cursor"]
            .iter()
            .any(|n| self.fs().exists(&self.p.repo.join(n)));
        self.b.add_layer(
            "project",
            LayerKind::Project,
            "project context files and project skills (skills need a trusted repo)",
            Some(".hermes.md / AGENTS.md / CLAUDE.md / .cursorrules".into()),
            project,
            LayerMode::Default,
        );
        let label = match &self.p.profile {
            Some(name) => format!("Hermes home (profile {name})"),
            None => "Hermes home".into(),
        };
        self.b.add_layer(
            "user",
            LayerKind::User,
            &label,
            Some(self.b.display(&self.p.active)),
            true,
            LayerMode::Default,
        );
    }

    fn settings(&mut self) {
        for set in SETTINGS {
            let mut trail: Vec<TrailStep> = Vec::new();
            if let Some(v) = at(&self.cfg_managed, set.path) {
                if !v.is_null() {
                    trail.push(TrailStep {
                        layer: "managed".into(),
                        value: v.clone(),
                        note: Some("pinned by the managed scope; a user value for it is refused".into()),
                    });
                }
            }
            if set.name == "model" {
                for (env, note) in [
                    ("HERMES_INFERENCE_MODEL", "process-level model override"),
                    ("HERMES_MODEL", "process-level model override (older name)"),
                ] {
                    if let Some(v) = self.ctx_var(env) {
                        trail.push(TrailStep {
                            layer: "session".into(),
                            value: Value::String(v),
                            note: Some(format!("{note} ({env})")),
                        });
                    }
                }
            }
            if let Some(v) = at(&self.cfg_user, set.path) {
                if !v.is_null() {
                    trail.push(TrailStep { layer: "user".into(), value: v.clone(), note: None });
                }
            }
            let Some(first) = trail.first().cloned() else { continue };
            let value = match set.kind {
                Kind::Text => first.value.as_str().map(|s| Value::String(printable(s))),
                Kind::Count => first.value.as_i64().map(|n| Value::Number(n.into())),
                Kind::Url => first.value.as_str().and_then(url_origin).map(Value::String),
            };
            let Some(value) = value else {
                self.b.warn(
                    "setting-unreadable",
                    format!("{} is set to a value rigswig cannot show; it is left out", set.name),
                    None,
                );
                continue;
            };
            self.b.settings.insert(set.name.into(), Decision { value, source: Some(first.layer.clone()), trail });
        }
    }

    /// The context chain: every folder from the repo up to the git root, root first.
    fn chain(&self) -> Vec<PathBuf> {
        let root = self.git_root();
        let mut chain: Vec<PathBuf> =
            self.p.repo.ancestors().take_while(|d| d.starts_with(&root)).map(Path::to_path_buf).collect();
        chain.reverse();
        chain
    }

    fn git_root(&self) -> PathBuf {
        self.p.repo.ancestors().find(|d| self.fs().exists(&d.join(".git"))).unwrap_or(&self.p.repo).to_path_buf()
    }

    fn non_empty(&self, file: &Path) -> Option<String> {
        let text = self.fs().read_to_string(file).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        Some(text)
    }

    /// One context file: cap-trimmed, hashed, counted. `None` when the file is absent or empty.
    fn add_doc(&mut self, file: &Path, layer: &str, when: LoadWhen, applies_to: Vec<String>) -> Option<usize> {
        let text = self.non_empty(file)?;
        let body = if text.chars().count() as u64 > self.cap { kept(&text, self.cap) } else { text.clone() };
        let cut_cap = body.chars().count() as u64;
        let whole = text.chars().count() as u64;
        if cut_cap < whole {
            self.b.warn(
                "context-file-cap",
                format!("cut to the context-file cap ({} chars); the middle is dropped with a marker", self.cap),
                Some(file),
            );
        }
        let name = self.b.display(file);
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to }),
            when,
            calc(char_len(&body)),
        );
        it.origin.file = Some(name.clone());
        it.load.content_hash = Some(sha256_hex(&text));
        Some(self.b.push(it))
    }

    /// A file that exists but lost to its type's winner (or another type's winner): it does not load.
    fn shadow_doc(&mut self, file: &Path, winner_id: &str) {
        let Some(text) = self.non_empty(file) else { return };
        let name = self.b.display(file);
        let mut it = self.b.item(
            "project",
            &name,
            ItemDetail::Instruction(InstructionAttrs::default()),
            LoadWhen::Startup,
            calc(char_len(&text)),
        );
        it.origin.file = Some(name);
        it.load.content_hash = Some(sha256_hex(&text));
        let idx = self.b.push(it);
        Build::disable(&mut self.b.items[idx]);
        self.b.items[idx].shadowed_by = Some(winner_id.to_string());
    }

    /// The project context files: only one type loads, first match wins (`.hermes.md` → `AGENTS.md` →
    /// `CLAUDE.md` → `.cursorrules`/`.cursor/rules`). SOUL.md is independent (see `memory`-adjacent user scope).
    fn instructions(&mut self) {
        enum Pick {
            Hermes(PathBuf),
            Agents(PathBuf),
            Claude(PathBuf),
            Cursor { main: PathBuf, extra: Vec<PathBuf> },
        }

        let chain = self.chain();
        let mut hermes_files: Vec<PathBuf> = Vec::new();
        for d in chain.iter().rev() {
            for n in [".hermes.md", "HERMES.md"] {
                let f = d.join(n);
                if self.non_empty(&f).is_some() && !hermes_files.contains(&f) {
                    hermes_files.push(f);
                }
            }
        }
        let agents_file = self.p.repo.join("AGENTS.md");
        let claude_file = self.p.repo.join("CLAUDE.md");
        let cursorrules = self.p.repo.join(".cursorrules");
        let mdc_files = self.b.files_in(&self.p.repo.join(".cursor/rules"), ".mdc");
        let cursor_main: Option<(PathBuf, Vec<PathBuf>)> = if self.non_empty(&cursorrules).is_some() {
            Some((cursorrules.clone(), mdc_files.clone()))
        } else {
            mdc_files.first().map(|m| (m.clone(), mdc_files.iter().skip(1).cloned().collect()))
        };

        let pick = if let Some(f) = hermes_files.first().cloned() {
            Pick::Hermes(f)
        } else if self.non_empty(&agents_file).is_some() {
            Pick::Agents(agents_file.clone())
        } else if self.non_empty(&claude_file).is_some() {
            Pick::Claude(claude_file.clone())
        } else if let Some((main, extra)) = cursor_main.clone() {
            Pick::Cursor { main, extra }
        } else {
            return;
        };

        let winner_path = match &pick {
            Pick::Hermes(f) | Pick::Agents(f) | Pick::Claude(f) => f.clone(),
            Pick::Cursor { main, .. } => main.clone(),
        };
        let Some(win_idx) = self.add_doc(&winner_path, "project", LoadWhen::Startup, Vec::new()) else { return };
        let winner_id = self.b.items[win_idx].id.clone();

        let cursor_all: Vec<PathBuf> =
            cursor_main.map(|(m, e)| std::iter::once(m).chain(e).collect()).unwrap_or_default();
        match pick {
            Pick::Hermes(w) => {
                if hermes_files.len() > 1 {
                    self.b.note_not_modelled(
                        "when several .hermes.md copies sit between the working folder and the git root, the nearest one wins and the rest are shadowed (inferred)",
                    );
                }
                for f in hermes_files.iter().filter(|f| **f != w) {
                    self.shadow_doc(f, &winner_id);
                }
                self.shadow_doc(&agents_file, &winner_id);
                self.shadow_doc(&claude_file, &winner_id);
                for f in &cursor_all {
                    self.shadow_doc(f, &winner_id);
                }
            }
            Pick::Agents(_) => {
                self.shadow_doc(&claude_file, &winner_id);
                for f in &cursor_all {
                    self.shadow_doc(f, &winner_id);
                }
                self.nested_agents();
            }
            Pick::Claude(_) => {
                self.shadow_doc(&agents_file, &winner_id);
                for f in cursor_all {
                    self.shadow_doc(&f, &winner_id);
                }
                self.b.note_not_modelled(
                    "nested AGENTS.md copies below the working folder (they load during navigation only when their type wins the session, which AGENTS.md did not here)",
                );
            }
            Pick::Cursor { extra, .. } => {
                self.shadow_doc(&agents_file, &winner_id);
                self.shadow_doc(&claude_file, &winner_id);
                for f in extra {
                    self.shadow_doc(&f, &winner_id);
                }
                self.b.note_not_modelled(
                    "each Cursor rule file's own frontmatter globs; every loaded rule file is counted at startup (inferred)",
                );
            }
        }
        let _ = mdc_files;
    }

    /// Nested AGENTS.md copies below the working folder: they load when the agent first works in that subfolder.
    fn nested_agents(&mut self) {
        let mut found: Vec<(PathBuf, String)> = Vec::new();
        let mut walk = Walk::new();
        self.walk_files(&self.p.repo, &self.p.repo, 0, "AGENTS.md", &mut walk, &mut found);
        if walk.exhausted {
            self.b.warn(
                "walk-stopped",
                "the subfolder walk stopped after 20,000 folders; nested AGENTS.md copies may be missing",
                None,
            );
        }
        for (f, rel) in found {
            if rel == "AGENTS.md" {
                continue; // the working folder's own copy, already added
            }
            let applies = rel.rsplit_once('/').map(|(dir, _)| vec![format!("{dir}/**")]).unwrap_or_default();
            self.add_doc(&f, "project", LoadWhen::OnMatch, applies);
        }
    }

    /// Every file ending with `ext` under `dir`: real folders only (dot folders, build and dependency output
    /// skipped), 12 folders deep, 20,000 folders at most.
    fn walk_files(
        &self,
        root: &Path,
        dir: &Path,
        depth: usize,
        ext: &str,
        walk: &mut Walk,
        out: &mut Vec<(PathBuf, String)>,
    ) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            if !walk.tick() {
                return;
            }
            let sub = dir.join(&e.name);
            match e.kind {
                crate::fs::EntryKind::Dir => {
                    if e.name.starts_with('.') || SKIPPED_DIRS.contains(&e.name.as_str()) {
                        continue;
                    }
                    self.walk_files(root, &sub, depth + 1, ext, walk, out);
                }
                _ if e.name.ends_with(ext) => {
                    let rel = sub.strip_prefix(root).unwrap_or(&sub).to_string_lossy().replace('\\', "/");
                    out.push((sub, rel));
                }
                _ => {}
            }
        }
    }

    /// SOUL.md: the agent's identity, slot #1 of the system prompt, always loaded when present.
    fn soul(&mut self) {
        let file = self.p.active.join("SOUL.md");
        let Some(text) = self.non_empty(&file) else { return };
        let body = if text.chars().count() as u64 > self.cap { kept(&text, self.cap) } else { text.clone() };
        let cut_to = body.chars().count() as u64;
        let whole_is = text.chars().count() as u64;
        if cut_to < whole_is {
            self.b.warn(
                "context-file-cap",
                format!(
                    "SOUL.md is cut to the context-file cap ({} chars); the middle is dropped with a marker",
                    self.cap
                ),
                Some(&file),
            );
        }
        let mut it = self.b.item(
            "user",
            "SOUL.md",
            ItemDetail::Instruction(InstructionAttrs::default()),
            LoadWhen::Startup,
            calc(char_len(&body)),
        );
        it.origin.file = Some(self.b.display(&file));
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// The memory files, injected at startup as a frozen snapshot.
    fn memory(&mut self) {
        if let Some(provider) = cfg_str(&self.cfg_merged, &["memory", "provider"]) {
            self.b.note_not_modelled(format!(
                "memory is handled by the configured provider ({provider}); rigswig models the built-in on-disk memory only"
            ));
            return;
        }
        let dir = self.p.active.join("memories");
        if cfg_on(&self.cfg_merged, &["memory", "memory_enabled"]) {
            self.memory_file(&dir.join("MEMORY.md"), cfg_count(&self.cfg_merged, &["memory", "memory_char_limit"]));
        }
        if cfg_on(&self.cfg_merged, &["memory", "user_profile_enabled"]) {
            self.memory_file(&dir.join("USER.md"), cfg_count(&self.cfg_merged, &["memory", "user_char_limit"]));
        }
    }

    fn memory_file(&mut self, file: &Path, limit: Option<u64>) {
        let Some(text) = self.non_empty(file) else { return };
        let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if let Some(limit) = limit {
            if char_len(&text) > limit {
                self.b.warn(
                    "memory-over-limit",
                    format!("{name} holds more than its {limit}-character limit (hand-edited? the memory tool refuses writes past it)"),
                    Some(file),
                );
            }
        }
        let mut it = self.b.item(
            "user",
            &name,
            ItemDetail::Instruction(InstructionAttrs::default()),
            LoadWhen::Startup,
            calc(char_len(&text)),
        );
        it.origin.file = Some(self.b.display(file));
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// Skills from every tier; the higher tier hides the same name lower down, and a repo's own skills need trust.
    fn skills(&mut self) {
        let discovery = cfg_on(&self.cfg_merged, &["skills", "project_discovery"]);
        let root = self.git_root();
        let trusted = self.project_trusted(&root);
        if discovery && !trusted {
            for sub in [".hermes/skills", ".agents/skills"] {
                if self.fs().exists(&root.join(sub)) {
                    self.b.warn(
                        "project-skills-untrusted",
                        "this repo carries project skills but is not in skills.trusted_project_dirs, so Hermes does not load them",
                        None,
                    );
                    break;
                }
            }
        }
        let mut tiers: Vec<(PathBuf, String)> = Vec::new();
        if discovery && trusted {
            for sub in [".hermes/skills", ".agents/skills"] {
                let d = root.join(sub);
                if self.fs().exists(&d) {
                    tiers.push((d, "project".into()));
                }
            }
        }
        let local = self.p.active.join("skills");
        if self.fs().exists(&local) {
            tiers.push((local, "user".into()));
        }
        if let Some(cd) = cfg_str(&self.cfg_merged, &["skills", "create_dir"]) {
            let d = expand(&self.p.active, &cd);
            if self.fs().exists(&d) {
                self.b.add_layer(
                    "create-dir",
                    LayerKind::Local,
                    "agent-created skills (skills.create_dir)",
                    Some(self.b.display(&d)),
                    true,
                    LayerMode::Default,
                );
                tiers.push((d, "create-dir".into()));
            }
        }
        for (i, e) in cfg_strs(&self.cfg_merged, &["skills", "external_dirs"]).into_iter().enumerate() {
            if e.contains("${") {
                self.b.warn(
                    "external-dir-unresolvable",
                    "an external skills dir names a ${VAR}; rigswig reads only Hermes's own variable list, so it is skipped",
                    None,
                );
                continue;
            }
            let d = expand(&self.p.active, &e);
            if self.fs().exists(&d) {
                let base = d.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let id = format!("external:{i}");
                self.b.add_layer(
                    &id,
                    LayerKind::Local,
                    &format!("external skills ({base})"),
                    Some(self.b.display(&d)),
                    true,
                    LayerMode::Default,
                );
                tiers.push((d, id));
            }
        }

        let mut walk = Walk::new();
        let mut found: Vec<(PathBuf /*skill dir*/, String /*rel*/, String /*layer*/)> = Vec::new();
        for (base, layer) in tiers {
            let mut here: Vec<(PathBuf, String)> = Vec::new();
            self.walk_skill_dirs(&base, &base, 0, &mut walk, &mut here);
            found.extend(here.into_iter().map(|(d, rel)| (d, rel, layer.clone())));
        }
        if walk.exhausted {
            self.b.warn(
                "walk-stopped",
                "the skills walk stopped after 20,000 folders; some skills may be missing",
                None,
            );
        }

        struct Entry {
            name: String,
            listing: u64,
            body: u64,
            file: PathBuf,
            rel: String,
            layer: String,
            platforms: Vec<String>,
        }
        let mut entries: Vec<Entry> = Vec::new();
        for (d, rel, layer) in found {
            let file = d.join("SKILL.md");
            let Some(text) = self.non_empty(&file) else { continue };
            let fm = frontmatter::parse(&text);
            let name = fm
                .str("name")
                .map(String::from)
                .unwrap_or_else(|| d.file_name().unwrap_or_default().to_string_lossy().into_owned());
            let platforms = fm.fields.get("platforms").map(|v| v.as_list()).unwrap_or_default();
            let listing = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
            entries.push(Entry { name, listing, body: char_len(&text[fm.body_start..]), file, rel, layer, platforms });
        }
        // Two same-named skills in one tier are never guessed between: both are listed by relative path.
        // Renaming pass: entries in one tier sharing a name get their relative skill path as the name.
        let mut renamed: Vec<(usize, String)> = Vec::new(); // (index in entries, new name)
        {
            // Group by tier and name. Cross-tier same names follow the tier precedence instead.
            let mut groups: std::collections::BTreeMap<String, Vec<usize>> = std::collections::BTreeMap::new();
            for (n, e) in entries.iter().enumerate() {
                let key = format!("{}|{}", e.layer, e.name);
                groups.entry(key).or_default().push(n);
            }
            for group in groups.values() {
                if group.len() > 1 {
                    let name = entries[group[0]].name.clone();
                    self.b.warn(
                        "ambiguous-skill",
                        format!("two skills named {name} in one tier; Hermes requires the exact relative path, so rigswig lists both by path"),
                        None,
                    );
                    renamed.extend(group.iter().map(|&n| (n, entries[n].rel.clone())));
                }
            }
        }
        for (n, new) in renamed.iter() {
            entries[*n].name = new.clone();
        }

        // The platform filter: a skill restricted to other operating systems is hidden here.
        let this_os = os_name(self.b.ctx.os);
        let mut idxs: Vec<usize> = Vec::new();
        for e in &entries {
            let platform_ok = e.platforms.is_empty() || this_os.is_some_and(|o| e.platforms.iter().any(|p| p == o));
            let file_label = self.b.display(&e.file);
            let mut it = self.b.item(
                &e.layer,
                &e.name,
                ItemDetail::Skill(SkillAttrs { body_chars: Some(e.body) }),
                LoadWhen::Startup,
                calc(e.listing),
            );
            it.origin.file = Some(file_label);
            it.load.content_hash = self.fs().read_to_string(&e.file).ok().map(|t| sha256_hex(&t));
            if !platform_ok {
                Build::disable(&mut it);
            }
            idxs.push(self.b.push(it));
        }
        self.b.resolve_clashes(&idxs, |i| i.name.clone(), tier_prio);
        for &i in &idxs {
            if self.b.items[i].shadowed_by.is_some() {
                let name = self.b.items[i].name.clone();
                self.b.warn(
                    "skill-shadowed",
                    format!("the skill {name} is shadowed by a higher tier here; the hidden copy is not loaded"),
                    None,
                );
            }
        }
        // Pinned skills: their full body loads at the start of every session.
        for want in cfg_strs(&self.cfg_merged, &["skills", "auto_load"]) {
            let found_idx = self.b.items.iter().position(|i| i.detail.kind() == "skill" && i.name == want).map(|idx| {
                (
                    idx,
                    self.b.items[idx].effective,
                    match &self.b.items[idx].detail {
                        ItemDetail::Skill(a) => a.body_chars.unwrap_or(0),
                        _ => 0,
                    },
                )
            });
            let Some((idx, effective, body)) = found_idx else {
                if self.b.items.iter().any(|i| i.name == want && i.detail.kind() == "skill") {
                    self.b.warn(
                        "auto-load-hidden",
                        format!("skills.auto_load names {want:?}, which is hidden or shadowed here; no fully loaded copy exists"),
                        None,
                    );
                } else {
                    self.b.warn(
                        "auto-load-missing",
                        format!(
                            "skills.auto_load names {want:?}, which no loaded skill provides; Hermes warns and skips it"
                        ),
                        None,
                    );
                }
                continue;
            };
            if !effective {
                self.b.warn(
                    "auto-load-hidden",
                    format!(
                        "skills.auto_load names {want:?}, which is hidden or shadowed here; no fully loaded copy exists"
                    ),
                    None,
                );
                continue;
            }
            let listing = self.b.items[idx].load.chars.value;
            self.b.items[idx].load.chars = calc(listing + body);
        }
    }

    fn project_trusted(&self, root: &Path) -> bool {
        let listed = cfg_strs(&self.cfg_merged, &["skills", "trusted_project_dirs"]);
        if listed.is_empty() {
            return false;
        }
        let expanded: Vec<PathBuf> = listed.iter().map(|s| expand(&self.p.home, s)).collect();
        if expanded.iter().any(|p| p == root) {
            return true;
        }
        if let Ok(real) = self.fs().canonicalize(root) {
            return expanded.iter().any(|p| p == &real);
        }
        false
    }

    /// Skill folders under `base`: real folders carrying a SKILL.md (dot folders and dependencies skipped).
    fn walk_skill_dirs(
        &self,
        base: &Path,
        dir: &Path,
        depth: usize,
        walk: &mut Walk,
        out: &mut Vec<(PathBuf, String)>,
    ) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            if !walk.tick() {
                return;
            }
            if e.kind != crate::fs::EntryKind::Dir || e.name.starts_with('.') {
                continue;
            }
            let sub = dir.join(&e.name);
            if self.fs().exists(&sub.join("SKILL.md")) {
                let rel = sub.strip_prefix(base).unwrap_or(&sub).to_string_lossy().replace('\\', "/");
                out.push((sub.clone(), rel));
            }
            self.walk_skill_dirs(base, &sub, depth + 1, walk, out);
        }
    }

    /// Folder plugins: present in the plugins folder, switched off only by `plugins.disabled`; their plugin.yaml
    /// hooks are counted, their Python-registered tools are not.
    fn plugins(&mut self) {
        let dir = self.p.active.join("plugins");
        if !self.fs().exists(&dir) {
            return;
        }
        let disabled = cfg_strs(&self.cfg_merged, &["plugins", "disabled"]);
        let allowed = at(&self.cfg_merged, &["plugins", "enabled"]).map(list_of);
        let mut enabled: usize = 0;
        for d in self.b.dirs_in(&dir) {
            let manifest = d.join("plugin.yaml");
            if !self.fs().exists(&manifest) {
                continue;
            }
            let yf = yaml::read_yaml(self.fs(), &manifest);
            self.b.warn_yaml(&yf, "plugin.yaml");
            let m = yf.object();
            let name = m
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map_or_else(|| d.file_name().unwrap_or_default().to_string_lossy().into_owned(), printable);
            let is_disabled = disabled.contains(&name);
            let in_allowed = allowed.as_ref().is_some_and(|a| a.contains(&name));
            if allowed.is_some() && !is_disabled && !in_allowed {
                self.b.warn(
                    "plugin-not-listed",
                    format!(
                        "the plugin {name} sits in the plugins folder but is not in plugins.enabled; Hermes runs a dropped-in plugin unless plugins.disabled names it, so rigswig counts it as active"
                    ),
                    None,
                );
            }
            let id = printable(&name);
            let mut scopes: std::collections::BTreeMap<String, bool> = std::collections::BTreeMap::new();
            scopes.insert("user".into(), !is_disabled);
            self.b.packages.push(Package {
                id: id.clone(),
                kind: PackageKind::Plugin,
                source: "plugins folder".into(),
                enabled: !is_disabled,
                decided_by: is_disabled.then(|| "user".into()),
                scopes,
                contributes: Vec::new(),
            });
            if is_disabled {
                continue;
            }
            enabled += 1;
            let layer = format!("plugin:{id}");
            self.b.add_layer(
                &layer,
                LayerKind::Package,
                &format!("plugin {id}"),
                Some(self.b.display(&d)),
                true,
                LayerMode::Default,
            );
            for hook in at(&m, &["provides_hooks"]).map(list_of).unwrap_or_default() {
                let attrs = HookAttrs {
                    event: crate::adapters::shared::neutral_event(&hook),
                    native_event: hook.clone(),
                    matcher: None,
                    handler: HookHandler::Unknown,
                    script: None,
                };
                let it = self.b.item(
                    &layer,
                    &format!("plugin.yaml:{hook}"),
                    ItemDetail::Hook(attrs),
                    LoadWhen::Never,
                    calc(0),
                );
                self.b.push(it);
            }
        }
        if enabled > 0 {
            self.b.not_measured.push(NotMeasured {
                what: format!(
                    "the tools, slash commands and skills {enabled} enabled plugin(s) register in Python (their schema text joins the prompt)"
                ),
                why: "plugin code is not read; plugin.yaml is the only manifest".into(),
            });
        }
    }

    /// MCP servers from the user and managed configs; on a name clash the managed server wins.
    fn mcp(&mut self) {
        let user_file = self.b.display(&self.p.active.join("config.yaml"));
        let managed_file = self.p.managed.as_ref().map(|d| self.b.display(&d.join("config.yaml")));
        let mut idxs: Vec<usize> = Vec::new();
        let layers: [(&str, &Map<String, Value>, Option<String>); 2] =
            [("managed", &self.cfg_managed, managed_file), ("user", &self.cfg_user, Some(user_file))];
        for (layer, cfg, file) in layers {
            let Some(servers) = at(cfg, &["mcp_servers"]).and_then(Value::as_object) else { continue };
            for (name, spec) in servers {
                let Some(spec) = spec.as_object() else { continue };
                let cmd = spec.get("command").and_then(Value::as_str);
                let url = spec.get("url").and_then(Value::as_str);
                let sse = spec.get("transport").and_then(Value::as_str) == Some("sse");
                let (transport, target) = match (cmd, url) {
                    (Some(c), _) => (McpTransport::Stdio, c.split_whitespace().next().map(safe_base)),
                    (None, Some(u)) => (
                        if sse { McpTransport::Sse } else { McpTransport::Http },
                        Some(url_origin(u).unwrap_or_else(|| "(unparsed url)".into())),
                    ),
                    _ => {
                        self.b.warn(
                            "mcp-server-transport",
                            format!(
                                "the MCP server {name} has neither command nor url; Hermes would fail to connect it"
                            ),
                            None,
                        );
                        continue;
                    }
                };
                let is_disabled = spec.get("enabled") == Some(&Value::Bool(false));
                let state = if is_disabled { McpState::Disabled } else { McpState::Active };
                let mut it = self.b.item(
                    layer,
                    name,
                    ItemDetail::Mcp(McpAttrs { transport, target, state }),
                    LoadWhen::Startup,
                    unknown(),
                );
                it.origin.file = file.clone();
                if is_disabled {
                    Build::disable(&mut it);
                }
                idxs.push(self.b.push(it));
            }
        }
        self.b.resolve_clashes(&idxs, |i| i.name.clone(), |i| if i.origin.layer == "managed" { 0 } else { 1 });
    }
}

/// The skill tiers in precedence order: a project skill beats the profile's (same name, lower tier hidden).
fn tier_prio(i: &Item) -> u32 {
    match i.origin.layer.as_str() {
        "project" => 0,
        "user" => 1,
        "create-dir" => 2,
        l if l.starts_with("external:") => {
            let n: u32 = l.strip_prefix("external:").and_then(|n| n.parse().ok()).unwrap_or(0);
            3 + n
        }
        _ => u32::MAX,
    }
}
