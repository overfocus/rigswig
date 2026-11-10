//! The Cognition Devin CLI adapter. Every rule it models is in `docs/adapters/devin.md`, with the docs it comes
//! from (all read 2026-11-06).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len, unknown};
use crate::adapters::shared::hooks_of;
use crate::model::*;
use crate::read::frontmatter::{self, FmValue};
use crate::read::json::{obj_at, str_at};
use crate::read::redact::{printable, safe_base, sha256_hex, url_origin};

pub const ID: &str = "devin";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The CLI release the docs this adapter follows describe (v3000.3, "Local 3.6"; read 2026-11-06).
const TOOL_VERSION: &str = "3000.3";

pub struct Devin;

/// The project rule files Devin's own standard set covers (the `agents_standard` import).
const STANDARD_RULES: &[&str] = &["AGENTS.md", "AGENTS.local.md", "AGENT.md", ".windsurfrules"];
/// The Windsurf channel directories; the CLI reads the one matching its update channel.
const CODEIUM_CHANNELS: &[&str] = &["windsurf", "windsurf-next", "windsurf-insiders"];
/// Heavy directories the lazy walk does not enter.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "vendor", "build", "coverage", ".venv", "venv"];

struct Paths {
    user: PathBuf,
    home_rules: PathBuf,
    project: PathBuf,
    system: PathBuf,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let user = match ctx.os {
            Os::Windows => ctx
                .var("APPDATA")
                .map(|d| PathBuf::from(d).join("devin"))
                .unwrap_or_else(|| ctx.home.join("AppData/Roaming/devin")),
            _ => ctx.home.join(".config/devin"),
        };
        let system = match ctx.os {
            Os::Macos => PathBuf::from("/Library/Application Support/Devin/system.json"),
            Os::Windows => PathBuf::from("C:\\ProgramData\\Devin\\system.json"),
            _ => PathBuf::from("/etc/devin/system.json"),
        };
        Self {
            user,
            system,
            home_rules: ctx.home.join(".devin"),
            project: ctx.repo.join(".devin"),
            repo: ctx.repo.clone(),
        }
    }

    fn codeium_dir(&self, ctx: &Ctx) -> Option<PathBuf> {
        // The docs leave no Windows path for it; the documented locations are the home's (inferred).
        Some(ctx.home.join(".codeium"))
    }
}

/// The `read_config_from` switches: every import is on unless a nearer config turns it off.
#[derive(Clone, Copy)]
struct Switches {
    agents_standard: bool,
    cursor: bool,
    windsurf: bool,
    claude: bool,
    copilot: bool,
    opencode: bool,
    zed: bool,
}

const SWITCH_KEYS: &[&str] = &["agents_standard", "cursor", "windsurf", "claude", "copilot", "opencode", "zed"];

/// One switch resolved from the project and user configs; unset means on (the docs' default).
fn switch(project: &Map<String, Value>, user: &Map<String, Value>, key: &str) -> Option<(bool, bool, bool)> {
    let at = |m: &Map<String, Value>| obj_at(m, "read_config_from").and_then(|r| r.get(key)).and_then(Value::as_bool);
    match (at(project), at(user)) {
        (Some(p), Some(u)) => Some((p, u, true)),
        (Some(p), None) => Some((p, false, true)),
        (None, Some(u)) => Some((false, u, true)),
        (None, None) => None,
    }
}

impl Switches {
    fn from(project: &Map<String, Value>, user: &Map<String, Value>) -> Self {
        // Unset means on (the docs' default); a match keeps this shape so clippy cannot "simplify" the
        // unset arm into false.
        let r = |k: &str| match switch(project, user, k) {
            Some((v, _, _)) => v,
            None => true,
        };
        Self {
            agents_standard: r("agents_standard"),
            cursor: r("cursor"),
            windsurf: r("windsurf"),
            claude: r("claude"),
            copilot: r("copilot"),
            opencode: r("opencode"),
            zed: r("zed"),
        }
    }
}

/// The agent's schema conversions: an import's config shape differs from theirs.
enum Convert {
    None,
    OpenCode,
    Zed,
}

/// One MCP server as declared: name, layer, its spec, and the file shown.
struct ServerDecl {
    name: String,
    layer: String,
    spec: Map<String, Value>,
    file: Option<String>,
}

struct Run<'a, 'b> {
    b: &'b mut Build<'a>,
    p: Paths,
    switches: Switches,
    user_cfg: Map<String, Value>,
    project_cfg: Map<String, Value>,
    local_cfg: Map<String, Value>,
    system_cfg: Map<String, Value>,
}

impl Adapter for Devin {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        // The CLI's paths are the XDG location (or %APPDATA% on Windows); no Devin-specific override is documented.
        &["APPDATA"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.project, ".devin/", 100),
            (p.repo.join(".devin-plugin"), ".devin-plugin/ (plugin source)", 80),
            (p.repo.join(".windsurf"), ".windsurf/", 65),
            (p.repo.join(".agents/skills"), ".agents/skills/", 60),
            (p.repo.join("AGENTS.md"), "AGENTS.md", 40),
            (p.user, "Devin CLI folder", 45),
            (p.home_rules, "~/.devin (home rules)", 40),
            (p.system, "system policy", 20),
        ];
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
        let user_cfg = b.jsonc(&p.user.join("config.json")).object();
        let project_cfg = b.jsonc(&p.project.join("config.json")).object();
        let local_cfg = b.jsonc(&p.project.join("config.local.json")).object();
        let system_cfg = b.json(&p.system).object();
        let switches = Switches::from(&project_cfg, &user_cfg);
        let has = |f: &Path| ctx.fs.exists(f);

        b.add_layer(
            "managed",
            LayerKind::Managed,
            "system policy (administrator-written)",
            Some(b.display(&p.system)),
            !system_cfg.is_empty() && system_cfg.contains_key("enterprise_host"),
            LayerMode::Enforce,
        );
        b.add_layer(
            "user",
            LayerKind::User,
            "the user config directory",
            Some(b.display(&p.user)),
            user_cfg.contains_key("agent")
                || user_cfg.contains_key("permissions")
                || user_cfg.contains_key("hooks")
                || user_cfg.contains_key("read_config_from")
                || has(&p.user.join("mcp_config.json"))
                || has(&p.user.join("AGENTS.md"))
                || has(&p.user.join("AGENT.md"))
                || has(&p.user.join("skills"))
                || has(&p.user.join("agents")),
            LayerMode::Default,
        );
        b.add_layer(
            "home_rules",
            LayerKind::User,
            "home-folder rules",
            Some(b.display(&p.home_rules)),
            has(&p.home_rules.join("global_rules.md"))
                || !b.files_in(&p.home_rules.join("rules"), ".md").is_empty()
                || has(&p.home_rules.join("AGENTS.md"))
                || has(&p.home_rules.join("AGENTS.local.md"))
                || has(&p.home_rules.join("AGENT.md")),
            LayerMode::Default,
        );
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .devin directory",
            Some(b.display(&p.project)),
            !project_cfg.is_empty()
                || has(&p.project.join("mcp_config.json"))
                || has(&p.project.join("hooks.v1.json"))
                || has(&p.project.join("global_rules.md"))
                || !b.files_in(&p.project.join("rules"), ".md").is_empty()
                || !b.dirs_in(&p.project.join("skills")).is_empty()
                || !b.dirs_in(&p.project.join("agents")).is_empty(),
            LayerMode::Default,
        );
        b.add_layer(
            "local",
            LayerKind::Local,
            "gitignored personal overrides",
            Some(b.display(&p.project.join("config.local.json"))),
            !local_cfg.is_empty() || has(&p.project.join("mcp_config.local.json")),
            LayerMode::Default,
        );
        let imports_project_present = has(&p.repo.join("CLAUDE.md"))
            || has(&p.repo.join(".windsurf"))
            || has(&p.repo.join(".cursor"))
            || has(&p.repo.join(".mcp.json"))
            || has(&p.repo.join(".claude"))
            || has(&p.repo.join("opencode.json"))
            || has(&p.repo.join(".zed"));
        b.add_layer(
            "imports_project",
            LayerKind::Project,
            "the other tools' project files the import reads (read_config_from, all on by default)",
            None,
            imports_project_present,
            LayerMode::Default,
        );
        b.add_layer(
            "imports_user",
            LayerKind::User,
            "the other tools' user-level files the import reads",
            None,
            has(&ctx.home.join(".claude.json"))
                || has(&ctx.home.join(".claude"))
                || has(&ctx.home.join(".agents/skills"))
                || has(&ctx.home.join(".copilot/skills"))
                || has(&ctx.home.join(".codeium"))
                || has(&ctx.home.join(".config/opencode/opencode.json"))
                || has(&ctx.home.join(".config/zed/settings.json")),
            LayerMode::Default,
        );

        let mut run = Run { b: &mut b, p, switches, user_cfg, project_cfg, local_cfg, system_cfg };
        if !run.switches.agents_standard {
            run.b.warn(
                "agents-standard-off",
                "read_config_from.agents_standard is off, so the standard rule files are not read",
                None,
            );
        }
        warn_unsupported_keys(run.b, &run.project_cfg);
        run.settings();
        run.instructions();
        run.skills();
        run.agents();
        run.hooks();
        run.mcp();
        run.permissions();
        run.b.link_packages();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt and the built-in tools".into(),
            why: "they come with Devin, not from any file".into(),
        });
        run.b.not_measured_common();

        for note in [
            "Devin Cloud: knowledge, playbooks, memories, cloud-configured MCP servers and personal plugins, which join sessions through read_config_from.cloud (on by default); rigswig reads local files only",
            "enterprise team settings enforced from the dashboard: model allowlists and the team default model (which applies until the user picks a model), MCP server allowances and toggles, web search and authoritative sandbox domain lists",
            "in-memory session grants (approvals saved for the session rank above every file, and nothing on disk writes them)",
            "the subagent router's model choice for subagent_explore, and Adaptive and Fusion model routing (server-side)",
            "plugin manifests fetched from the cloud at sign-in (enterprise, org and personal levels), and plugins installed locally: their disk location is not documented, so a checkout's requiredPlugins, optionalPlugins and forbiddenPlugins are modelled as governance declarations, and a required plugin's own rules, agents, hooks, MCP servers and skills are not measured",
            "plugin governance beyond what a checkout declares: higher-authority wins, deny-wins carve-outs, blocked plugins soft-failing at session start, and managed-manifest fetches failing open",
            "which of ~/.codeium/<channel> actually loads: the CLI's update channel picks one of windsurf, windsurf-next, windsurf-insiders; every existing one is listed and marked channel-dependent",
            "the same-name merge order among imported MCP sources of different tools (only 'higher-priority source wins' is documented, and only among Devin's own layers)",
            "the workspace root's definition: rules between the workspace root and the session's working directory load at startup; rigswig treats the repo as the workspace root (inferred)",
            "AGENTS.md-family and CLAUDE.md files below the workspace root load lazily, only when the agent first touches their directory; they are listed as load-on-invoke, not measured into the startup total",
            "glob-activated rules: which files a glob rule fires for is decided at runtime, and the pattern's documented form does not carry it, so such rules are listed as load-on-invoke",
            "subagent frontmatter beyond name, description, model and allowed-tools (max-nesting and other experimental fields are skipped)",
            "skills' allowed-tools and per-skill permission overrides, which apply only while the skill runs",
            "hooks that ask an LLM to decide or rewrite a tool's input (PermissionRequest decisions, updatedInput): listed, never evaluated",
            "the sandbox (--sandbox): OS-level isolation and its network filtering run per session flag",
            ".codeiumignore ignore patterns, which the docs record Devin CLI does not support",
            "Windows deployments beyond the documented %APPDATA% paths (the system policy file's documented path is used)",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        drop(run);
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

fn warn_unsupported_keys(b: &mut Build, project: &Map<String, Value>) {
    // Only permissions, read_config_from, hooks and (v3000.3+) mcpServers exist at the project level.
    for k in ["agent", "permission_mode", "theme_mode", "sandbox", "subagents_enabled", "attribution", "proxy"] {
        if project.contains_key(k) {
            b.warn("project-key-ignored", format!("{k} is set in the project config, where the CLI ignores it"), None);
        }
    }
}

/// A host or host:port without credentials, scheme or path.
fn host_only(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        return None;
    }
    let v = url_origin(value).unwrap_or_else(|| printable(value));
    let v = v.split_once("://").map_or_else(|| v.clone(), |(_, rest)| rest.to_string());
    let host = v.rsplit_once('@').map_or_else(|| v.clone(), |(_, h)| h.to_string());
    Some(host)
}

/// A managed object with its strings redacted (URLs keep their origin, other non-empty strings stay printable)
fn safe_object(v: &Value) -> Value {
    match v {
        Value::String(s) => url_origin(s).map(Value::String).unwrap_or_else(|| Value::String(printable(s))),
        Value::Array(a) => Value::Array(a.iter().map(safe_object).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), safe_object(v))).collect()),
        other => other.clone(),
    }
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    fn home(&self) -> &Path {
        &self.b.ctx.home
    }

    /// A whole instruction file as one item, measured against its text.
    fn add_doc(&mut self, file: &Path, layer: &str, when: LoadWhen) {
        let Ok(text) = self.fs().read_to_string(file) else { return };
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let name = self.b.display(file);
        let mut it =
            self.b.item(layer, &name, ItemDetail::Instruction(InstructionAttrs::default()), when, calc(char_len(body)));
        it.origin.file = Some(name);
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// One rule file with the Windsurf-style `trigger` frontmatter. Below the workspace root every rule file
    /// loads lazily (on access), so the trigger only matters at the root.
    fn add_rule(&mut self, file: &Path, layer: &str, at_root: bool) {
        let Ok(text) = self.fs().read_to_string(file) else { return };
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let fm = frontmatter::parse(&text);
        let trigger = fm.str("trigger").unwrap_or("always_on");
        let when = match (trigger, at_root) {
            (_, false) => LoadWhen::OnInvoke,
            ("always_on", true) | ("manual", true) => LoadWhen::Startup,
            ("model_decision", true) | ("agent", true) | ("glob", true) => LoadWhen::OnInvoke,
            (other, true) => {
                self.b.warn(
                    "unknown-trigger",
                    format!("trigger {other:?} is not a documented value; treated as always-on"),
                    Some(file),
                );
                LoadWhen::Startup
            }
        };
        let name = self.b.display(file);
        let mut it =
            self.b.item(layer, &name, ItemDetail::Instruction(InstructionAttrs::default()), when, calc(char_len(body)));
        it.origin.file = Some(name);
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// `.cursor/rules/*.md(c)`: `alwaysApply` starts the session, `globs` fire on match, a description alone is
    /// the agent's call, and nothing at all waits for the user.
    fn add_cursor_rule(&mut self, file: &Path, layer: &str) {
        let Ok(text) = self.fs().read_to_string(file) else { return };
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let fm = frontmatter::parse(&text);
        let always = fm.str("alwaysApply").is_some_and(|a| a == "true");
        let globs = fm.fields.get("globs").map(FmValue::as_list).unwrap_or_default();
        let (when, applies) = match (globs.is_empty(), always) {
            (false, _) => (LoadWhen::OnMatch, globs),
            (true, true) => (LoadWhen::Startup, Vec::new()),
            (true, false) => (LoadWhen::OnInvoke, Vec::new()),
        };
        let name = self.b.display(file);
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to: applies }),
            when,
            calc(char_len(body)),
        );
        it.origin.file = Some(name);
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// The rules: the user's global file, the home-folder rules, the root's standard family and `.devin/`
    /// rules, every import surface, and everything below the root as lazily loaded.
    fn instructions(&mut self) {
        for name in ["AGENTS.md", "AGENT.md"] {
            let f = self.p.user.join(name);
            if let Ok(text) = self.fs().read_to_string(&f) {
                if !text.trim().is_empty() {
                    self.add_doc(&f, "user", LoadWhen::Startup);
                    break;
                }
            }
        }
        if let Some(f) = ["global_rules.md"].iter().map(|n| self.p.home_rules.join(n)).find(|f| self.fs().exists(f)) {
            self.add_doc(&f, "home_rules", LoadWhen::Startup);
        }
        for f in self.b.files_in(&self.p.home_rules.join("rules"), ".md") {
            self.add_doc(&f, "home_rules", LoadWhen::Startup);
        }

        if self.switches.agents_standard {
            for name in STANDARD_RULES {
                let f = self.p.repo.join(name);
                if self.fs().exists(&f) {
                    self.add_doc(&f, "project", LoadWhen::Startup);
                }
            }
        }
        if self.switches.claude && self.fs().exists(&self.p.repo.join("CLAUDE.md")) {
            self.add_doc(&self.p.repo.join("CLAUDE.md"), "imports_project", LoadWhen::Startup);
        }

        self.devin_rules_at_root();
        if self.switches.windsurf {
            let windsurf_global = self.p.repo.join(".windsurf/global_rules.md");
            if self.fs().exists(&windsurf_global) {
                if self.fs().exists(&self.p.project.join("global_rules.md")) {
                    self.b.warn(
                        "windsurf-global-rules-shadowed",
                        ".devin/global_rules.md exists, so .windsurf/global_rules.md is not read",
                        Some(&windsurf_global),
                    );
                } else {
                    self.add_doc(&windsurf_global, "imports_project", LoadWhen::Startup);
                }
            }
            for f in self.b.files_in(&self.p.repo.join(".windsurf/rules"), ".md") {
                self.add_rule(&f, "imports_project", true);
            }
        }
        if self.switches.cursor {
            for f in self.b.files_in(&self.p.repo.join(".cursor/rules"), ".md") {
                self.add_cursor_rule(&f, "imports_project");
            }
            for f in self.b.files_in(&self.p.repo.join(".cursor/rules"), ".mdc") {
                self.add_cursor_rule(&f, "imports_project");
            }
        }

        self.lazy_rules();
    }

    fn devin_rules_at_root(&mut self) {
        if let Some(f) = ["global_rules.md"].iter().map(|n| self.p.project.join(n)).find(|f| self.fs().exists(f)) {
            self.add_doc(&f, "project", LoadWhen::Startup);
        }
        for f in self.b.files_in(&self.p.project.join("rules"), ".md") {
            self.add_rule(&f, "project", true);
        }
    }

    /// Every rule file below the repo root is discovered lazily, when the agent first touches its directory.
    fn lazy_rules(&mut self) {
        let mut found: Vec<(PathBuf, Loader)> = Vec::new();
        self.walk_lazy(&self.p.repo, 0, &mut found);
        for (file, loader) in found {
            match loader {
                Loader::Doc(l) => self.add_doc(&file, &l, LoadWhen::OnInvoke),
                Loader::Rule(l) => self.add_rule(&file, &l, false),
                Loader::Cursor(l) => self.add_cursor_rule(&file, &l),
            }
        }
    }

    fn walk_lazy(&self, dir: &Path, depth: usize, out: &mut Vec<(PathBuf, Loader)>) {
        if depth > 6 {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            // The repo root's .devin/.windsurf/.cursor directories and its standard rule files are handled
            // above; deeper ones join the lazy list here.
            if SKIP_DIRS.contains(&e.name.as_str())
                || (e.name.starts_with('.')
                    && !matches!(e.name.as_str(), ".devin" | ".windsurf" | ".cursor")
                    && depth > 0)
                || (depth == 0 && matches!(e.name.as_str(), ".devin" | ".windsurf" | ".cursor"))
                || (depth == 0 && (STANDARD_RULES.contains(&e.name.as_str()) || e.name == "CLAUDE.md"))
            {
                continue;
            }
            let p = dir.join(&e.name);
            match e.kind {
                crate::fs::EntryKind::Dir => {
                    if e.name == ".devin" {
                        if self.fs().exists(&p.join("global_rules.md")) {
                            out.push((p.join("global_rules.md"), Loader::Rule("project".into())));
                        }
                        for f in self.b.files_in(&p.join("rules"), ".md") {
                            out.push((f, Loader::Rule("project".into())));
                        }
                        continue;
                    }
                    if e.name == ".windsurf" && self.switches.windsurf {
                        // .devin takes precedence over .windsurf for the always-on file, at every level.
                        let devin_twin = dir.join(".devin/global_rules.md");
                        if self.fs().exists(&p.join("global_rules.md")) && !self.fs().exists(&devin_twin) {
                            out.push((p.join("global_rules.md"), Loader::Rule("imports_project".into())));
                        }
                        for f in self.b.files_in(&p.join("rules"), ".md") {
                            out.push((f, Loader::Rule("imports_project".into())));
                        }
                        continue;
                    }
                    if e.name == ".cursor" && self.switches.cursor {
                        for f in self
                            .b
                            .files_in(&p.join("rules"), ".md")
                            .into_iter()
                            .chain(self.b.files_in(&p.join("rules"), ".mdc"))
                        {
                            out.push((f, Loader::Cursor("imports_project".into())));
                        }
                        continue;
                    }
                    self.walk_lazy(&p, depth + 1, out);
                }
                _ => {
                    let standard =
                        ["AGENTS.md", "AGENT.md", "AGENTS.local.md", ".windsurfrules"].contains(&e.name.as_str());
                    if (standard && self.switches.agents_standard) || (e.name == "CLAUDE.md" && self.switches.claude) {
                        out.push((p, Loader::Doc("project".into())));
                    }
                }
            }
        }
    }

    /// Push a skill, keeping same-named skills apart (nothing documents which one a name resolves to).
    fn skill(&mut self, dir: &Path, layer: &str) -> Option<usize> {
        let file = dir.join("SKILL.md");
        let text = self.fs().read_to_string(&file).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        let fm = frontmatter::parse(&text);
        let base = fm
            .str("name")
            .map(String::from)
            .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
        let chars = char_len(&base) + char_len(fm.str("description").unwrap_or(""));
        let body = char_len(&text[fm.body_start..]);
        let mut it = self.b.item(
            layer,
            &base,
            ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }),
            LoadWhen::Startup,
            calc(chars),
        );
        it.origin.file = Some(self.b.display(&file));
        it.load.content_hash = Some(sha256_hex(&text));
        let base_id = it.id.clone();
        let mut n = 2;
        while self.b.items.iter().any(|i| i.id == it.id) {
            it.id = format!("{base_id}-{n}");
            n += 1;
        }
        Some(self.b.push(it))
    }

    /// The skills, one location per source family: Devin's own, the `.agents` standard, the imports.
    fn skills(&mut self) {
        let project: Vec<(PathBuf, &str)> = [
            (self.p.project.join("skills"), "project"),
            (self.p.repo.join(".agents/skills"), "project"),
            (self.p.repo.join(".windsurf/skills"), "imports_project"),
            (self.p.repo.join(".claude/skills"), "imports_project"),
            (self.p.repo.join(".github/skills"), "imports_project"),
        ]
        .into_iter()
        .filter(|(dir, layer)| match *layer {
            "project" => true,
            "imports_project" => self.switches.any_import_skills_at(dir),
            _ => true,
        })
        .collect();
        for (dir, layer) in project {
            for d in self.b.dirs_in(&dir) {
                let _ = self.skill(&d, layer);
            }
        }
        // Claude's commands import as skills.
        if self.switches.claude {
            let rel = self.p.repo.join(".claude/commands");
            for (f, rel_path) in self.b.walk(&rel, ".md") {
                let Ok(text) = self.fs().read_to_string(&f) else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                let fm = frontmatter::parse(&text);
                let stem = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let chars = char_len(&stem) + char_len(fm.str("description").unwrap_or(""));
                let body = char_len(&text[fm.body_start..]);
                let mut it = self.b.item(
                    "imports_project",
                    &stem,
                    ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }),
                    LoadWhen::OnInvoke,
                    calc(chars),
                );
                it.origin.file = Some(format!("{}/{}", self.b.display(&rel), rel_path));
                it.load.content_hash = Some(sha256_hex(&text));
                self.b.push(it);
            }
        }
        for d in self.b.dirs_in(&self.p.user.join("skills")) {
            let _ = self.skill(&d, "user");
        }
        for d in self.b.dirs_in(&self.home().join(".agents/skills")) {
            let _ = self.skill(&d, "user");
        }
        if self.switches.copilot {
            for d in self.b.dirs_in(&self.home().join(".copilot/skills")) {
                let _ = self.skill(&d, "imports_user");
            }
        }
        if self.switches.windsurf {
            if let Some(root) = self.p.codeium_dir(self.b.ctx) {
                let mut present: Vec<String> = Vec::new();
                for channel in CODEIUM_CHANNELS {
                    let d = root.join(*channel);
                    if self.fs().exists(&d) {
                        present.push((*channel).to_string());
                    }
                    for s in self.b.dirs_in(&d.join("skills")) {
                        let _ = self.skill(&s, "imports_user");
                    }
                }
                if present.len() > 1 {
                    self.b.warn(
                        "channel-ambiguous",
                        format!(
                            "several Windsurf channel folders exist ({}); the CLI reads only its own update channel",
                            present.join(", ")
                        ),
                        None,
                    );
                }
            }
        }
    }

    /// Custom subagent profiles: `agents/<name>.md` or `agents/<name>/AGENT.md` (AGENT.md before its
    /// alternatives). Project and user compete on the name; project is assumed to win.
    fn agents(&mut self) {
        let mut idx = Vec::new();
        for (dir, layer) in [(self.p.project.join("agents"), "project"), (self.p.user.join("agents"), "user")] {
            for d in self.b.dirs_in(&dir) {
                let candidates = ["AGENT.md", "AGENTS.md", "agent.md", "agents.md"].map(|n| d.join(n));
                if let Some(file) = candidates.into_iter().find(|f| self.fs().exists(f)) {
                    idx.extend(self.agent_item(&file, &d, layer));
                }
            }
            for f in self.b.files_in(&dir, ".md") {
                idx.extend(self.agent_item(&f, &dir, layer));
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |i| if i.origin.layer == "project" { 0 } else { 1 });
        self.b.note_not_modelled("whether a project agent beats a user agent of the same name (assumed yes)");
    }

    fn agent_item(&mut self, file: &Path, dir: &Path, layer: &str) -> Option<usize> {
        let text = self.fs().read_to_string(file).ok()?;
        if text.trim().is_empty() {
            return None;
        }
        let fm = frontmatter::parse(&text);
        let fallback = match file.file_name().and_then(|n| n.to_str())? {
            "AGENT.md" | "AGENTS.md" | "agent.md" | "agents.md" => {
                dir.file_name().unwrap_or_default().to_string_lossy().into_owned()
            }
            stem => stem.trim_end_matches(".md").to_string(),
        };
        let name = fm.str("name").map(String::from).unwrap_or(fallback);
        let chars = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
        let tools = match &fm.fields.get("allowed-tools").map(FmValue::as_list) {
            Some(list) if !list.is_empty() => ToolAccess::Only(list.clone()),
            _ => ToolAccess::All,
        };
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Agent(AgentAttrs { tools, model: fm.str("model").map(String::from) }),
            LoadWhen::Startup,
            calc(chars),
        );
        it.origin.file = Some(self.b.display(file));
        it.load.content_hash = Some(sha256_hex(&text));
        Some(self.b.push(it))
    }

    /// Lifecycle hooks: `.devin/hooks.v1.json` is the map itself, `hooks` keys sit in the config files, and
    /// Claude's files join when that import is on.
    fn hooks(&mut self) {
        let hooksv1 = self.p.project.join("hooks.v1.json");
        if self.fs().exists(&hooksv1) {
            let j = self.b.jsonc(&hooksv1).object();
            if !j.is_empty() {
                let shown = Some(self.b.display(&hooksv1));
                hooks_of(self.b, &j, "project", shown, "");
            }
        }
        for (cfg, layer, fallback) in [
            (&self.project_cfg, "project", self.p.project.join("config.json")),
            (&self.local_cfg, "local", self.p.project.join("config.local.json")),
            (&self.user_cfg, "user", self.p.user.join("config.json")),
        ] {
            if let Some(block) = obj_at(cfg, "hooks").cloned() {
                let shown = Some(self.b.display(&fallback));
                hooks_of(self.b, &block, layer, shown, "");
            }
        }
        if self.switches.claude {
            for (f, layer) in [
                (self.p.repo.join(".claude/settings.json"), "imports_project"),
                (self.p.repo.join(".claude/settings.local.json"), "imports_project"),
                (self.home().join(".claude.json"), "imports_user"),
                (self.home().join(".claude/settings.json"), "imports_user"),
                (self.home().join(".claude/settings.local.json"), "imports_user"),
            ] {
                if self.fs().exists(&f) {
                    let j = self.b.jsonc(&f).object();
                    if let Some(block) = obj_at(&j, "hooks").cloned() {
                        let shown = Some(self.b.display(&f));
                        hooks_of(self.b, &block, layer, shown, "");
                    }
                }
            }
        }
    }

    /// The MCP servers: Devin's own files, the legacy `mcpServers` keys, and the imports. Same-named
    /// servers are taken whole from the higher layer.
    fn mcp(&mut self) {
        let mut decls: Vec<ServerDecl> = Vec::new();
        let mut sources: Vec<(PathBuf, &str, u32, Option<&str>, Convert)> = vec![
            (self.p.project.join("mcp_config.local.json"), "local", 1, Some("mcpServers"), Convert::None),
            (self.p.project.join("mcp_config.json"), "project", 2, Some("mcpServers"), Convert::None),
            (self.p.user.join("mcp_config.json"), "user", 3, Some("mcpServers"), Convert::None),
        ];
        if self.project_cfg.contains_key("mcpServers") || self.user_cfg.contains_key("mcpServers") {
            self.b.warn(
                "legacy-mcp-location",
                "mcpServers inside the main config is the pre-3000.3 layout; the CLI migrates it to mcp_config.json",
                None,
            );
            sources.push((self.p.project.join("config.json"), "project", 2, Some("mcpServers"), Convert::None));
            sources.push((self.p.user.join("config.json"), "user", 3, Some("mcpServers"), Convert::None));
        }
        if self.switches.cursor {
            sources.push((
                self.p.repo.join(".cursor/mcp.json"),
                "imports_project",
                10,
                Some("mcpServers"),
                Convert::None,
            ));
        }
        if self.switches.opencode {
            sources.push((self.p.repo.join("opencode.json"), "imports_project", 11, Some("mcp"), Convert::OpenCode));
            sources.push((
                self.home().join(".config/opencode/opencode.json"),
                "imports_user",
                20,
                Some("mcp"),
                Convert::OpenCode,
            ));
        }
        if self.switches.zed {
            sources.push((
                self.p.repo.join(".zed/settings.json"),
                "imports_project",
                12,
                Some("context_servers"),
                Convert::Zed,
            ));
            sources.push((
                self.home().join(".config/zed/settings.json"),
                "imports_user",
                21,
                Some("context_servers"),
                Convert::Zed,
            ));
        }
        if self.switches.claude {
            sources.push((self.p.repo.join(".mcp.json"), "imports_project", 13, None, Convert::None));
            sources.push((
                self.p.repo.join(".claude/settings.json"),
                "imports_project",
                14,
                Some("mcpServers"),
                Convert::None,
            ));
            sources.push((
                self.p.repo.join(".claude/settings.local.json"),
                "imports_project",
                15,
                Some("mcpServers"),
                Convert::None,
            ));
            sources.push((self.home().join(".claude.json"), "imports_user", 22, Some("mcpServers"), Convert::None));
            sources.push((
                self.home().join(".claude/settings.json"),
                "imports_user",
                23,
                Some("mcpServers"),
                Convert::None,
            ));
            sources.push((
                self.home().join(".claude/settings.local.json"),
                "imports_user",
                24,
                Some("mcpServers"),
                Convert::None,
            ));
            sources.push((self.home().join(".claude/mcp_servers.json"), "imports_user", 25, None, Convert::None));
        }
        if self.switches.windsurf {
            if let Some(root) = self.p.codeium_dir(self.b.ctx) {
                for channel in CODEIUM_CHANNELS {
                    let f = root.join(*channel).join("mcp_config.json");
                    if self.fs().exists(&f) {
                        sources.push((f, "imports_user", 26, Some("mcpServers"), Convert::None));
                        break;
                    }
                }
            }
        }

        for (file, layer, priority, key, convert) in sources {
            if !self.fs().exists(&file) {
                continue;
            }
            let wrapper: Option<&str> = key;
            let j = self.b.jsonc(&file).object();
            let map = match wrapper {
                Some(k) => obj_at(&j, k).cloned(),
                None => obj_at(&j, "mcpServers").cloned().or_else(|| Some(j.clone())),
            }
            .unwrap_or_default();
            let shown = Some(self.b.display(&file));
            for (name, s) in map {
                let Some(spec) = convert.run(s, &name) else { continue };
                let _ = priority;
                decls.push(ServerDecl { name: printable(&name), layer: layer.into(), spec, file: shown.clone() });
            }
        }

        let idx: Vec<usize> = decls
            .into_iter()
            .map(|d| {
                let cmd = str_at(&d.spec, "command");
                let url = str_at(&d.spec, "url");
                let kind = str_at(&d.spec, "transport").or_else(|| str_at(&d.spec, "type"));
                let (transport, target) = match (cmd, url) {
                    (Some(c), _) => (McpTransport::Stdio, c.split_whitespace().next().map(safe_base)),
                    (None, Some(u)) => (
                        match kind {
                            Some("sse") => McpTransport::Sse,
                            Some("http") | Some("streamable-http") | None => McpTransport::Http,
                            _ => McpTransport::Unknown,
                        },
                        url_origin(u).or_else(|| Some("(unparsed url)".into())),
                    ),
                    _ => (McpTransport::Unknown, None),
                };
                let disabled = d.spec.get("disabled") == Some(&Value::Bool(true))
                    || d.spec.get("enabled") == Some(&Value::Bool(false));
                let mut it = self.b.item(
                    &d.layer,
                    &d.name,
                    ItemDetail::Mcp(McpAttrs {
                        transport,
                        target,
                        state: if disabled { McpState::Disabled } else { McpState::Active },
                    }),
                    LoadWhen::Startup,
                    unknown(),
                );
                it.origin.file = d.file;
                if disabled {
                    Build::disable(&mut it);
                }
                self.b.push(it)
            })
            .collect();
        self.b.resolve_clashes(
            &idx,
            |i| i.name.clone(),
            |i| match i.origin.layer.as_str() {
                "managed" => 0,
                "local" => 1,
                "project" => 2,
                "user" => 3,
                "imports_project" => 4,
                "imports_user" => 5,
                _ => 9,
            },
        );
    }

    /// Permission rules merge across every layer (the docs say "merged across levels"): all rules are
    /// listed, none shadowed.
    fn permissions(&mut self) {
        for (layer, cfg, fallback) in [
            ("user", self.user_cfg.clone(), self.p.user.join("config.json")),
            ("project", self.project_cfg.clone(), self.p.project.join("config.json")),
            ("local", self.local_cfg.clone(), self.p.project.join("config.local.json")),
        ] {
            let Some(perms) = obj_at(&cfg, "permissions") else { continue };
            let mut seen = std::collections::BTreeSet::new();
            for (key, effect) in
                [("deny", PermissionEffect::Deny), ("ask", PermissionEffect::Ask), ("allow", PermissionEffect::Allow)]
            {
                for rule in
                    perms.get(key).and_then(Value::as_array).unwrap_or(&Vec::new()).iter().filter_map(Value::as_str)
                {
                    if !seen.insert(format!("{key}{rule}")) {
                        continue;
                    }
                    let (tool, pattern) = match rule.split_once('(') {
                        Some((t, rest)) => (t.to_string(), Some(rest.trim_end_matches(')').to_string())),
                        None => (rule.to_string(), None),
                    };
                    let name = format!("{key} {rule}");
                    let mut it = self.b.item(
                        layer,
                        &name,
                        ItemDetail::Permission(PermissionAttrs { effect, tool, pattern }),
                        LoadWhen::Never,
                        calc(0),
                    );
                    it.origin.file = Some(self.b.display(&fallback));
                    self.b.push(it);
                }
            }
        }
    }

    /// The resolved settings, each with its trail across the layers that had a say.
    fn settings(&mut self) {
        let user = &self.user_cfg;
        let agent = obj_at(user, "agent").cloned().unwrap_or_default();
        for (key, value) in [
            ("model", agent.get("model").or(None).cloned()),
            ("subagentModel", agent.get("subagent_model").cloned()),
            ("subagentsEnabled", user.get("subagents_enabled").cloned()),
            ("permissionMode", user.get("permission_mode").cloned()),
            ("attribution", user.get("attribution").cloned()),
            ("respectGitignore", user.get("respect_gitignore").cloned()),
        ] {
            let Some(v) = value else { continue };
            let name = key.to_string();
            let note = if key == "model" || key == "subagentModel" { format!("agent.{key}") } else { name.clone() };
            self.b.settings.insert(
                name,
                Decision {
                    value: v.clone(),
                    source: Some("user".into()),
                    trail: vec![TrailStep { layer: "user".into(), value: v, note: Some(note) }],
                },
            );
        }

        // Sandbox: the user's own lists (domain names, not secrets) beside the mode.
        if let Some(s) = obj_at(user, "sandbox").cloned() {
            self.b.settings.insert(
                "sandbox".into(),
                Decision {
                    value: Value::Object(s.clone()),
                    source: Some("user".into()),
                    trail: vec![TrailStep { layer: "user".into(), value: Value::Object(s), note: None }],
                },
            );
        }

        // The import switches: each key's nearest say-so, and the trail of everything that had one.
        let keys: Vec<String> = {
            let mut ks: Vec<String> = SWITCH_KEYS.iter().map(|s| s.to_string()).collect();
            for src in [&self.project_cfg, &self.user_cfg] {
                for k in
                    obj_at(src, "read_config_from").map(|r| r.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()
                {
                    if !ks.contains(&k) {
                        ks.push(k);
                    }
                }
            }
            ks
        };
        for key in keys {
            let steps: Vec<(String, Value)> = [("project", &self.project_cfg), ("user", &self.user_cfg)]
                .into_iter()
                .filter_map(|(l, m)| {
                    obj_at(m, "read_config_from")
                        .and_then(|r| r.get(&key))
                        .and_then(Value::as_bool)
                        .map(|v| (l.to_string(), Value::Bool(v)))
                })
                .collect();
            if steps.len() == 1 && steps[0].1 == Value::Bool(true) {
                continue; // set to the default at one layer: nothing resolves against it
            }
            if steps.is_empty() {
                continue;
            }
            let Some((_, value)) = steps.first().cloned() else { continue };
            let trail: Vec<TrailStep> = steps
                .iter()
                .map(|(l, v)| TrailStep { layer: l.clone(), value: v.clone(), note: Some(key.clone()) })
                .collect();
            let name = if key == "cloud" { "cloud".into() } else { format!("readConfigFrom.{key}") };
            let src = trail.last().map(|t| t.layer.clone());
            self.b.settings.insert(name, Decision { value, source: src, trail });
        }

        // The managed policy is the system file's whole point; its strings are redacted.
        if !self.system_cfg.is_empty() {
            let mut trail = Vec::new();
            for (key, v) in &self.system_cfg {
                let shown = match (key.as_str(), v.as_str()) {
                    ("enterprise_host", Some(s)) => host_only(s).map(Value::String).unwrap_or(Value::Null),
                    ("proxy", _) => safe_object(v),
                    (_, Some(s)) => Value::String(printable(s)),
                    _ => safe_object(v),
                };
                trail.push(TrailStep { layer: "managed".into(), value: shown, note: Some(key.clone()) });
            }
            let host = self
                .system_cfg
                .get("enterprise_host")
                .and_then(Value::as_str)
                .and_then(host_only)
                .map(Value::String)
                .unwrap_or(Value::Null);
            self.b.settings.insert("policy".into(), Decision { value: host, source: Some("managed".into()), trail });
        }
    }
}

impl Convert {
    fn run(&self, s: Value, _name: &str) -> Option<Map<String, Value>> {
        match self {
            Self::None => s.as_object().cloned(),
            Self::OpenCode => {
                // OpenCode's schema: `command` may be an array, `environment` is env, and `enabled` is the
                // inverse of the standard `disabled`.
                let mut out = Map::new();
                match s.get("command") {
                    Some(Value::Array(a)) => {
                        out.insert("command".into(), a.first().cloned().unwrap_or(Value::String(String::new())));
                    }
                    Some(Value::String(c)) => {
                        out.insert("command".into(), Value::String(c.clone()));
                    }
                    _ => {}
                }
                if let Some(u) = s.get("url").and_then(Value::as_str) {
                    out.insert("url".into(), Value::String(u.to_string()));
                }
                if let Some(env) = s.get("environment") {
                    out.insert("env".into(), env.clone());
                }
                if s.get("enabled") == Some(&Value::Bool(false)) {
                    out.insert("disabled".into(), Value::Bool(true));
                }
                (out.contains_key("command") || out.contains_key("url")).then_some(out)
            }
            Self::Zed => {
                // Zed keeps servers under `context_servers`; a command or a URL is what rigswig can show.
                let Value::Object(mut out) = s else { return None };
                if out.get("command").is_none() && out.get("url").is_none() {
                    return None;
                }
                out.remove("source");
                Some(out)
            }
        }
    }
}

/// Which reader a lazily discovered rule file uses.
enum Loader {
    Doc(String),
    Rule(String),
    Cursor(String),
}

impl Switches {
    /// Whether the import that owns `dir` is on.
    fn any_import_skills_at(&self, dir: &Path) -> bool {
        match dir.file_name().and_then(|n| n.to_str()) {
            Some(".windsurf") => self.windsurf,
            Some(".claude") => self.claude,
            Some(".github") => self.copilot,
            _ => true,
        }
    }
}
