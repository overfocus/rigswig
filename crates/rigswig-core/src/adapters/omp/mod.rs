//! The oh-my-pi adapter (`omp`, github.com/can1357/oh-my-pi, a fork of Pi). Every rule it models is in
//! `docs/adapters/omp.md`, with the source files it comes from (all read 2026-11-07, at release 18.18.0).
//!
//! OMP reads its own `.omp` folders and, through discovery providers, other harnesses' files: a repo's `.claude/`,
//! `.cursor/`, `.github/` and so on load by default, and those harnesses' home folders when the user opts in. Each
//! provider's files get a layer of their own (`claude:project`, `cursor:user`, ...), so every item says where OMP
//! found it and nothing is counted as OMP's own.

mod context;
mod items;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::Build;
use crate::model::*;
use crate::read::redact::printable;
use crate::read::yaml::read_yaml;

pub const ID: &str = "omp";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release whose source this adapter follows (`@oh-my-pi/pi-coding-agent` 18.18.0, commit e95a8e7).
const TOOL_VERSION: &str = "18.18.0";

/// Discovery providers: id, priority (higher wins), and how a layer names them. Ties keep this order, which is
/// OMP's registration order (`discovery/index.ts`).
const PROVIDERS: &[(&str, u32, &str)] = &[
    ("native", 100, "OMP"),
    ("claude", 80, "Claude Code"),
    ("agents", 70, ".agent/.agents"),
    ("codex", 70, "Codex"),
    ("gemini", 60, "Gemini CLI"),
    ("opencode", 55, "OpenCode"),
    ("cursor", 50, "Cursor"),
    ("windsurf", 50, "Windsurf"),
    ("cline", 40, "Cline"),
    ("github", 30, "GitHub Copilot"),
    ("vscode", 20, "VS Code"),
    ("agents-md", 10, "standalone AGENTS.md"),
    ("claude-md", 10, "standalone CLAUDE.md"),
    ("custom-context", 9, "contextFiles.extra"),
    ("mcp-json", 5, "root mcp.json"),
    ("managed-skills", 5, "auto-learned skills"),
];

/// Providers whose user-level files load only when `enabledProviders` names them (`capability/index.ts`).
const FOREIGN_USER: &[&str] =
    &["cursor", "codex", "claude", "claude-plugins", "gemini", "opencode", "windsurf", "github"];

pub(super) fn priority(provider: &str) -> u32 {
    PROVIDERS.iter().find(|p| p.0 == provider).map_or(0, |p| p.1)
}

fn order(provider: &str) -> usize {
    PROVIDERS.iter().position(|p| p.0 == provider).unwrap_or(usize::MAX)
}

fn display_name(provider: &str) -> &'static str {
    PROVIDERS.iter().find(|p| p.0 == provider).map_or("another tool", |p| p.2)
}

pub(super) struct Paths {
    pub home: PathBuf,
    pub repo: PathBuf,
    /// The active agent directory: `~/<cfg>/agent`, a profile's, or `$PI_CODING_AGENT_DIR`.
    pub agent: PathBuf,
    /// The agent directory as task agents and APPEND_SYSTEM.md see it: profile-aware, but blind to
    /// `$PI_CODING_AGENT_DIR` (`config.ts`).
    pub agent_plain: PathBuf,
    /// The user root's folder name: `.omp`, or `$PI_CONFIG_DIR`. Project folders are always `.omp`.
    pub config_dir_name: String,
    pub claude_user: PathBuf,
    pub claude_json: PathBuf,
    pub codex_user: PathBuf,
    pub copilot_user: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let cfg = ctx.var("PI_CONFIG_DIR").unwrap_or(".omp").to_string();
        // OMP_PROFILE wins whenever it is defined, even empty; PI_PROFILE is the legacy fallback.
        let profile = match ctx.env.get("OMP_PROFILE") {
            Some(p) => Some(p.trim().to_string()),
            None => ctx.var("PI_PROFILE").map(|p| p.trim().to_string()),
        }
        .filter(|p| !p.is_empty() && p != "default");
        let root = ctx.home.join(&cfg);
        let agent_plain = match &profile {
            Some(p) => root.join("profiles").join(p).join("agent"),
            None => root.join("agent"),
        };
        let agent = match (&profile, ctx.var("PI_CODING_AGENT_DIR")) {
            (None, Some(d)) => expand(&ctx.home, d),
            _ => agent_plain.clone(),
        };
        let (claude_user, claude_json) = match ctx.var("CLAUDE_CONFIG_DIR") {
            Some(d) => {
                let d = expand(&ctx.home, d.trim());
                (d.clone(), d.join(".claude.json"))
            }
            None => (ctx.home.join(".claude"), ctx.home.join(".claude.json")),
        };
        let copilot_user = ctx.var("COPILOT_HOME").map_or_else(|| ctx.home.join(".copilot"), |d| expand(&ctx.home, d));
        Self {
            home: ctx.home.clone(),
            repo: ctx.repo.clone(),
            agent,
            agent_plain,
            config_dir_name: cfg,
            claude_user,
            claude_json,
            codex_user: ctx.home.join(".codex"),
            copilot_user,
        }
    }
}

fn expand(home: &Path, p: &str) -> PathBuf {
    if p == "~" {
        home.to_path_buf()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// The settings layers that decide what loads: the global `config.yml` and the merged project layer.
pub(super) struct Settings {
    global: Map<String, Value>,
    project: Map<String, Value>,
}

impl Settings {
    fn leaf<'s>(m: &'s Map<String, Value>, path: &[&str]) -> Option<&'s Value> {
        let (last, head) = path.split_last()?;
        let mut at = m;
        for k in head {
            at = at.get(*k)?.as_object()?;
        }
        at.get(*last).filter(|v| !v.is_null())
    }

    /// A value and the layer it comes from: project over global. Objects merge key by key (a leaf lookup is that
    /// merge); arrays and scalars are replaced whole by the higher layer.
    pub(super) fn get(&self, path: &[&str]) -> Option<(&Value, &'static str)> {
        Self::leaf(&self.project, path)
            .map(|v| (v, "project"))
            .or_else(|| Self::leaf(&self.global, path).map(|v| (v, "user")))
    }

    pub(super) fn strs_with_layer(&self, path: &[&str]) -> Option<(Vec<String>, &'static str)> {
        let (v, layer) = self.get(path)?;
        let list = v.as_array()?.iter().filter_map(Value::as_str).map(str::to_string).collect();
        Some((list, layer))
    }

    pub(super) fn strs(&self, path: &[&str]) -> Vec<String> {
        self.strs_with_layer(path).map(|(l, _)| l).unwrap_or_default()
    }

    pub(super) fn bool_or(&self, path: &[&str], default: bool) -> bool {
        self.get(path).and_then(|(v, _)| v.as_bool()).unwrap_or(default)
    }
}

/// Deep merge: objects recurse, everything else is replaced (`config/settings.ts`).
fn merge(dst: &mut Map<String, Value>, src: &Map<String, Value>) {
    for (k, v) in src {
        match (dst.get_mut(k), v) {
            (Some(Value::Object(d)), Value::Object(s)) => merge(d, s),
            _ => {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
}

pub struct Omp;

impl Adapter for Omp {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["PI_CODING_AGENT_DIR", "PI_CONFIG_DIR", "OMP_PROFILE", "PI_PROFILE", "CLAUDE_CONFIG_DIR", "COPILOT_HOME"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [(p.repo.join(".omp"), ".omp/", 100), (p.home.join(&p.config_dir_name), "OMP folder", 45)];
        let mut score = 0;
        let mut evidence = Vec::new();
        for (path, label, s) in checks {
            if ctx.fs.exists(&path) {
                score = score.max(s);
                evidence.push(label.to_string());
            }
        }
        Detection { score, evidence }
    }

    fn resolve(&self, ctx: &Ctx) -> Harness {
        let mut b = Build::new(ctx, ID);
        b.detected_by = self.detect(ctx).evidence;
        let p = Paths::new(ctx);
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .omp directory (OMP's own project files)",
            Some(b.display(&p.repo.join(".omp"))),
            false,
            LayerMode::Default,
        );
        b.add_layer(
            "user",
            LayerKind::User,
            "the agent directory (OMP's own user files)",
            Some(b.display(&p.agent)),
            ctx.fs.exists(&p.agent),
            LayerMode::Default,
        );
        let repo_root = ctx.repo.ancestors().find(|d| ctx.fs.exists(&d.join(".git"))).map(Path::to_path_buf);
        let mut run = Run { b: &mut b, p, settings: Settings { global: Map::new(), project: Map::new() }, repo_root };
        run.load_settings();
        run.decisions();
        run.context_files();
        run.sticky_rules();
        run.system_prompt();
        run.rules();
        run.skills();
        run.commands();
        run.agents();
        run.mcp();
        run.hooks();
        run.permissions();
        run.finish_layers();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the built-in tools and their descriptions".into(),
            why: "they come with OMP, not from any file".into(),
        });
        run.b.not_measured.push(NotMeasured {
            what: "the bundled task agents' lines in the task tool (scout, reviewer, security-reviewer, seance, task, sonic) and the bundled TTSR rules".into(),
            why: "they ship inside OMP".into(),
        });
        run.b.not_measured_common();
        for note in [
            "plugins and packages: omp-plugins (~/.omp/plugins), the Claude-compatible marketplace (installed_plugins.json), Agent Plugins (plugin.json), Skillshare lock files, and Claude Code's own plugins (claude-plugins), with every skill, command, rule, hook, tool, agent and MCP server they bring",
            "extensions and custom tools (.omp/extensions, tools/, -e): executable TypeScript and scripts rigswig lists by nobody but OMP itself and never runs",
            "PI_CONFIG_FILES and --config overlays, runtime overrides, the settings environment variables, .env files (never opened: they hold secrets), and command-line flags",
            "path-scoped enabledProviders and disabledProviders entries (lists of maps), which the YAML reader skips with a warning",
            "MCP servers shadowed by an equivalent connection under another name, the Exa and browser servers OMP drops, servers' instructions, and tool schemas",
            "the session-dependent parts: rules matched to subagents by their agents globs, TTSR rules fired by a condition mid-session, skills a subagent autoloads, and the model-compressed skill descriptions OMP may cache (rigswig counts the 100-character preview)",
            "gitignore filtering of command, rule, hook and tool folders, the discovery ceilings for temporary folders, WSL's second home, and COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
            "the content-containment check counts paragraphs before @import expansion",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        drop(run);
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

pub(super) struct Run<'b, 'a> {
    pub b: &'b mut Build<'a>,
    pub p: Paths,
    pub settings: Settings,
    pub repo_root: Option<PathBuf>,
}

impl Run<'_, '_> {
    pub(super) fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// The layer for one provider's files at one scope, created on first use.
    pub(super) fn layer(&mut self, provider: &str, scope: &str) -> String {
        if provider == "native" {
            self.b.set_present(scope);
            return scope.to_string();
        }
        let id = format!("{provider}:{scope}");
        if !self.b.layers.iter().any(|l| l.id == id) {
            let (kind, label) = if scope == "user" {
                (LayerKind::User, format!("{}'s user files, which OMP imports", display_name(provider)))
            } else {
                (LayerKind::Project, format!("{}'s files in the repo, which OMP imports", display_name(provider)))
            };
            self.b.add_layer(&id, kind, &label, None, true, LayerMode::Default);
        }
        id
    }

    /// Ranks follow provider priority, then project before user (the order most capabilities emit in).
    fn finish_layers(&mut self) {
        let key = |id: &str| -> (std::cmp::Reverse<u32>, usize, bool) {
            let (provider, scope) = match id {
                "project" | "user" => ("native", id),
                other => other.split_once(':').unwrap_or((other, "")),
            };
            (std::cmp::Reverse(priority(provider)), order(provider), scope == "user")
        };
        self.b.layers.sort_by_key(|l| key(&l.id));
        for (i, l) in self.b.layers.iter_mut().enumerate() {
            l.rank = i as u32;
        }
    }

    /// Whether a provider's files at a scope load: not when `disabledProviders` names it; at user scope a foreign
    /// provider needs `enabledProviders` (or `*`/`all`; Claude also by `CLAUDE_CONFIG_DIR`).
    pub(super) fn source_on(&self, provider: &str, scope: &str) -> bool {
        if self.settings.strs(&["disabledProviders"]).iter().any(|p| p == provider) {
            return false;
        }
        if scope != "user" || !FOREIGN_USER.contains(&provider) {
            return true;
        }
        let on = self.settings.strs(&["enabledProviders"]);
        on.iter().any(|p| p == provider || p == "*" || p == "all")
            || (provider == "claude" && self.b.ctx.var("CLAUDE_CONFIG_DIR").is_some_and(|v| !v.trim().is_empty()))
    }

    /// The repo's `.omp/` when it is a native project root: non-empty, and the repo is not the home.
    pub(super) fn native_project_dir(&self) -> Option<PathBuf> {
        let d = self.p.repo.join(".omp");
        (self.p.repo != self.p.home && self.fs().read_dir(&d).is_ok_and(|e| !e.is_empty())).then_some(d)
    }

    /// The global `config.yml` (or `config.yaml`) and the project layer: `.omp/settings.json` then
    /// `.omp/config.yml`, then every foreign project settings file in provider order, the later file winning a key,
    /// with `.omp/config.yml`'s model roles put back on top (`Settings.#loadProjectSettings`).
    fn load_settings(&mut self) {
        let yml = self.p.agent.join("config.yml");
        let global_file = if self.fs().exists(&yml) { yml } else { self.p.agent.join("config.yaml") };
        if self.fs().exists(&global_file) {
            let f = read_yaml(self.fs(), &global_file);
            self.b.warn_yaml(&f, "the global config.yml");
            self.settings.global = f.object();
        }
        let mut project = Map::new();
        let mut native_roles: Option<Value> = None;
        if let Some(d) = self.native_project_dir() {
            if self.fs().exists(&d.join("settings.json")) {
                merge(&mut project, &self.b.json(&d.join("settings.json")).object());
            }
            if self.fs().exists(&d.join("config.yml")) {
                let f = read_yaml(self.fs(), &d.join("config.yml"));
                self.b.warn_yaml(&f, "the project's .omp/config.yml");
                let m = f.object();
                native_roles = m.get("modelRoles").cloned();
                merge(&mut project, &m);
            }
        }
        let r = self.p.repo.clone();
        let foreign: Vec<(&str, PathBuf)> = vec![
            ("claude", r.join(".claude/settings.json")),
            ("codex", r.join(".codex/config.toml")),
            ("gemini", r.join(".gemini/settings.json")),
            ("opencode", r.join("opencode.json")),
            ("opencode", r.join("opencode.jsonc")),
            ("opencode", r.join(".opencode/opencode.json")),
            ("opencode", r.join(".opencode/opencode.jsonc")),
            ("cursor", r.join(".cursor/settings.json")),
        ];
        for (_provider, f) in foreign {
            if !self.fs().exists(&f) {
                continue;
            }
            let ext = f.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
            let m = match ext.as_str() {
                "toml" => self.b.toml(&f).object(),
                "jsonc" => self.b.jsonc(&f).object(),
                _ => self.b.jsonc(&f).object(),
            };
            merge(&mut project, &m);
        }
        if let Some(Value::Object(roles)) = native_roles {
            let mut top = Map::new();
            top.insert("modelRoles".into(), Value::Object(roles));
            merge(&mut project, &top);
        }
        // A provider turned off in disabledProviders contributes no settings; rigswig reads the list after the
        // merge (it decides from the same layers), which differs only if a foreign file sets disabledProviders.
        self.settings.project = project;
    }

    /// The settings that decide behaviour, each with the layer that set it.
    fn decisions(&mut self) {
        for (path, name) in [
            (&["modelRoles", "default"][..], "model"),
            (&["tools", "approvalMode"][..], "approvalMode"),
            (&["enabledProviders"][..], "enabledProviders"),
            (&["disabledProviders"][..], "disabledProviders"),
            (&["contextFiles", "extra"][..], "contextFiles.extra"),
            (&["skills", "enabled"][..], "skills.enabled"),
            (&["mcp", "enableProjectConfig"][..], "mcp.enableProjectConfig"),
        ] {
            let mut trail = Vec::new();
            for (layer, m) in [("project", &self.settings.project), ("user", &self.settings.global)] {
                if let Some(v) = Settings::leaf(m, path) {
                    let value = match v {
                        Value::String(s) => Value::String(printable(s)),
                        other => other.clone(),
                    };
                    trail.push(TrailStep { layer: layer.into(), value, note: None });
                }
            }
            let Some(first) = trail.first() else { continue };
            let (value, source) = (first.value.clone(), first.layer.clone());
            self.b.settings.insert(name.into(), Decision { value, source: Some(source), trail });
        }
        if !self.b.settings.contains_key("approvalMode") {
            self.b.settings.insert(
                "approvalMode".into(),
                Decision { value: Value::String("yolo".into()), source: None, trail: Vec::new() },
            );
        }
    }

    /// Permission rules: `tools.approval.<tool>` (allow, deny or prompt; merged per tool across layers) and
    /// `tools.restrictedFolders` (every call into one prompts, even in yolo mode).
    fn permissions(&mut self) {
        let mut rules: BTreeMap<String, (Value, &'static str)> = BTreeMap::new();
        for (layer, m) in [("user", &self.settings.global), ("project", &self.settings.project)] {
            if let Some(o) = Settings::leaf(m, &["tools", "approval"]).and_then(Value::as_object) {
                for (k, v) in o {
                    rules.insert(k.clone(), (v.clone(), layer));
                }
            }
        }
        for (tool, (v, layer)) in rules {
            let effect = match v.as_str() {
                Some("allow") => PermissionEffect::Allow,
                Some("deny") => PermissionEffect::Deny,
                Some("prompt") => PermissionEffect::Ask,
                _ => continue,
            };
            let attrs = PermissionAttrs { effect, tool: printable(&tool), pattern: None };
            let it = self.b.item(
                layer,
                &format!("approval:{tool}"),
                ItemDetail::Permission(attrs),
                LoadWhen::Never,
                crate::adapters::build::calc(0),
            );
            self.b.push(it);
        }
        if let Some((folders, layer)) = self.settings.strs_with_layer(&["tools", "restrictedFolders"]) {
            for f in folders {
                let attrs =
                    PermissionAttrs { effect: PermissionEffect::Ask, tool: "*".into(), pattern: Some(printable(&f)) };
                let it = self.b.item(
                    layer,
                    &format!("restricted:{f}"),
                    ItemDetail::Permission(attrs),
                    LoadWhen::Never,
                    crate::adapters::build::calc(0),
                );
                self.b.push(it);
            }
        }
    }
}
