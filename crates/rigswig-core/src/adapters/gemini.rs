//! The Gemini CLI adapter. Every rule it models is in `docs/adapters/gemini.md`, with the docs it comes
//! from (all read 2026-11-06).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::adapters::shared::hooks_of;
use crate::fs::EntryKind;
use crate::model::*;
use crate::read::frontmatter::{self, FmValue};
use crate::read::imports::imports_of;
use crate::read::redact::{printable, sha256_hex, url_origin};

pub const ID: &str = "gemini";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The settings format the docs this adapter follows describe (the nested v0.3+ structure; read 2026-11-06).
const TOOL_VERSION: &str = "0.3";

pub struct Gemini;

/// A same-name clash's winner: workspace files over user files over extensions (**inferred** for the
/// extension tie-breaks; the docs pin only settings-over-extension and workspace-over-extension).
fn ext_rank(i: &Item) -> u32 {
    match i.origin.layer.as_str() {
        "system" => 0,
        "project" => 1,
        "user" => 2,
        "extensions" => 3,
        _ => 9,
    }
}

/// Heavy directories the lazy context walk does not enter.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "vendor", "build", "coverage", ".venv", "venv"];
/// The context discovery budget (`context.discoveryMaxDirs`, default 200).
const DISCOVERY_MAX_DIRS: usize = 200;
/// The @-import depth the memory import processor follows (its documented default is 5 levels).
const IMPORT_MAX_HOPS: usize = 5;
/// The context file the docs name, when nothing configures another.
const DEFAULT_CONTEXT_FILES: &[&str] = &["GEMINI.md"];

struct Paths {
    user: PathBuf,
    project: PathBuf,
    repo: PathBuf,
    system: PathBuf,
    system_defaults: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let system = match ctx.os {
            Os::Windows => PathBuf::from("C:\\ProgramData\\gemini-cli\\settings.json"),
            Os::Macos => PathBuf::from("/Library/Application Support/GeminiCli/settings.json"),
            _ => PathBuf::from("/etc/gemini-cli/settings.json"),
        };
        let system_defaults = match ctx.os {
            Os::Windows => PathBuf::from("C:\\ProgramData\\gemini-cli\\system-defaults.json"),
            Os::Macos => PathBuf::from("/Library/Application Support/GeminiCli/system-defaults.json"),
            _ => PathBuf::from("/etc/gemini-cli/system-defaults.json"),
        };
        let system = ctx.var("GEMINI_CLI_SYSTEM_SETTINGS_PATH").map_or_else(|| system, PathBuf::from);
        let defaults =
            ctx.var("GEMINI_CLI_SYSTEM_DEFAULTS_PATH").map_or_else(|| system_defaults.clone(), PathBuf::from);
        Self {
            user: ctx.home.join(".gemini"),
            project: ctx.repo.join(".gemini"),
            repo: ctx.repo.clone(),
            system,
            system_defaults: defaults,
        }
    }
}

impl Adapter for Gemini {
    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["GEMINI_CLI_SYSTEM_SETTINGS_PATH", "GEMINI_CLI_SYSTEM_DEFAULTS_PATH"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.project, ".gemini/", 100),
            (p.repo.join("GEMINI.md"), "GEMINI.md", 60),
            (p.repo.join(".geminiignore"), ".geminiignore", 30),
            (p.user, "Gemini CLI folder", 45),
            (p.system, "system settings", 20),
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
        let has = |f: &Path| ctx.fs.exists(f);

        let user_cfg = b.jsonc(&p.user.join("settings.json")).object();
        let project_cfg = b.jsonc(&p.project.join("settings.json")).object();
        let system_cfg = b.json(&p.system).object();
        let defaults_cfg = b.json(&p.system_defaults).object();

        // The configuration docs' precedence: the system settings file overrides the project and user
        // files; the system defaults file is the base every other file overrides. Hooks are the documented
        // exception: every layer's hooks run, none shadow another.
        b.add_layer(
            "system",
            LayerKind::Managed,
            "the system settings file (overrides the project and user ones)",
            Some(b.display(&p.system)),
            !system_cfg.is_empty(),
            LayerMode::Enforce,
        );
        b.add_layer(
            "project",
            LayerKind::Project,
            "the workspace's .gemini directory and its root context file",
            Some(b.display(&p.project)),
            !project_cfg.is_empty()
                || !b.files_in(&p.project.join("commands"), ".toml").is_empty()
                || !b.files_in(&p.project.join("agents"), ".md").is_empty()
                || context_names(&project_cfg, &user_cfg).iter().any(|n| has(&p.repo.join(n))),
            LayerMode::Default,
        );
        b.add_layer(
            "user",
            LayerKind::User,
            "the user config directory",
            Some(b.display(&p.user)),
            !user_cfg.is_empty()
                || !b.files_in(&p.user.join("commands"), ".toml").is_empty()
                || !b.files_in(&p.user.join("agents"), ".md").is_empty()
                || context_names(&user_cfg, &user_cfg).iter().any(|n| has(&p.user.join(n)))
                || has(&p.user.join("policies")),
            LayerMode::Default,
        );
        let extensions_present = has(&p.user.join("extensions")) && !b.dirs_in(&p.user.join("extensions")).is_empty();
        b.add_layer(
            "extensions",
            LayerKind::Local,
            "installed extensions and what they declare",
            Some(b.display(&p.user.join("extensions"))),
            extensions_present,
            LayerMode::Default,
        );
        b.add_layer(
            "system_defaults",
            LayerKind::Managed,
            "the system defaults file (the base layer)",
            Some(b.display(&p.system_defaults)),
            !defaults_cfg.is_empty(),
            LayerMode::Default,
        );

        let mut run = Run { b: &mut b, p, home: ctx.home.clone(), user_cfg, project_cfg, system_cfg, defaults_cfg };
        run.settings();
        run.context_files();
        run.commands();
        run.agents();
        run.hooks();
        run.mcp();
        run.policies();
        run.b.link_packages();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the built-in tools and the model".into(),
            why: "they come with Gemini CLI, not from any file".into(),
        });
        run.b.not_measured_common();

        for note in [
            "the @-import expansion's file-boundary comments separators: the trail lists each imported file as its own item, and the CLI also wraps files with separators when it concatenates them",
            "GEMINI.md files in directories above the repo root (the CLI also searches ancestors up to a trusted root); rigswig reads only inside the repo",
            "extensions installed from the cloud and their per-workspace enable/disable state (`gemini extensions disable`), which is settings state, not a file",
            "extension `excludeTools` and the per-approval-mode tool policies the policy engine matches at runtime (argsPattern regular expressions against live tool arguments)",
            "the workspace policy tier, which the documentation marks as currently disabled",
            "the `.geminiignore` and `.gitignore` filtering of the context (@-file) system: permission- and search-scope control, not load",
            "MCP tool listings, extensions' `contextFileName` discovery beyond the manifest, and remote (Agent2Agent) subagents",
            "the sub-agent router's model choice and the Antigravity CLI migration of June 2026: the installed CLI's files are this adapter's subject",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        drop(run);
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

/// The context file names in effect (`context.fileName`, a string or an array), GEMINI.md by default.
fn context_names(project: &Map<String, Value>, user: &Map<String, Value>) -> Vec<String> {
    fn at(c: &Map<String, Value>) -> Option<&Value> {
        c.get("context").and_then(|c| c.as_object()).and_then(|c| c.get("fileName"))
    }
    let v = at(project).or_else(|| at(user));
    match v {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(a)) => {
            a.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).collect()
        }
        _ => DEFAULT_CONTEXT_FILES.iter().map(|s| (*s).to_string()).collect(),
    }
}

struct Run<'b, 'a> {
    b: &'b mut Build<'a>,
    p: Paths,
    home: PathBuf,
    user_cfg: Map<String, Value>,
    project_cfg: Map<String, Value>,
    system_cfg: Map<String, Value>,
    defaults_cfg: Map<String, Value>,
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// The settings that decide behaviour, each with its trail. A layer's key wins in the configuration
    /// docs' order: system over project over user over the defaults file.
    fn settings(&mut self) {
        let layers = [
            ("system", self.system_cfg.clone()),
            ("project", self.project_cfg.clone()),
            ("user", self.user_cfg.clone()),
            ("system_defaults", self.defaults_cfg.clone()),
        ];
        for (key, target) in [
            (vec!["model", "name"], "model"),
            (vec!["tools", "sandbox"], "sandbox"),
            (vec!["model", "maxSessionTurns"], "maxSessionTurns"),
            (vec!["context", "fileName"], "contextFileName"),
            (vec!["context", "discoveryMaxDirs"], "discoveryMaxDirs"),
            (vec!["context", "includeDirectories"], "includeDirectories"),
        ] {
            self.decide(target, &layers, &key);
        }
    }

    /// One decision: the value from the highest-ranked layer that sets this dotted path, with the full trail.
    fn decide(&mut self, name: &str, layers: &[(&str, Map<String, Value>)], key: &[&str]) {
        let mut trail = Vec::new();
        for (layer, cfg) in layers {
            if let Some(v) = self.at(cfg, key) {
                trail.push(((*layer).to_string(), v.clone()));
            }
        }
        if trail.is_empty() {
            return;
        }
        let (source, value) = trail.first().cloned().unwrap_or_default();
        let trail = trail
            .iter()
            .map(|(l, v)| crate::model::TrailStep {
                layer: l.clone(),
                // Strings pass through the printable filter; other values load as they are.
                value: match v {
                    Value::String(s) => Value::String(printable(s)),
                    other => other.clone(),
                },
                note: None,
            })
            .collect();
        self.b.settings.insert(name.to_string(), crate::model::Decision { value, source: Some(source), trail });
    }

    fn at(&self, cfg: &Map<String, Value>, key: &[&str]) -> Option<Value> {
        let mut at = cfg;
        for (i, k) in key.iter().enumerate() {
            let next = at.get(*k)?;
            if i + 1 == key.len() {
                if next.is_null() {
                    return None;
                }
                return Some(next.clone());
            }
            at = next.as_object()?;
        }
        None
    }

    /// The hierarchical context files: one at the user level every session, the workspace root every session,
    /// and any in subdirectories only when the agent touches them (just-in-time). Each file's `@path` imports
    /// follow up to five hops into the repo and the user folder.
    fn context_files(&mut self) {
        for name in context_names(&self.project_cfg, &self.user_cfg) {
            // The user's global context file: every session, every project.
            let u = self.p.user.join(&name);
            let mut seen = BTreeSet::new();
            self.add_context(&u, "user", LoadWhen::Startup, None, 0, &mut seen);

            // The repo root and every ancestor-less subdirectory the agent may open files in.
            let r = self.p.repo.join(&name);
            self.add_context(&r, "project", LoadWhen::Startup, None, 0, &mut seen);
            let mut budget = DISCOVERY_MAX_DIRS;
            let repo = self.p.repo.clone();
            self.walk_context(repo.as_path(), &name, 0, &mut budget, &mut seen);
        }
    }

    /// Subdirectory context files: the docs' just-in-time discovery, out of the startup total.
    fn walk_context(&mut self, dir: &Path, name: &str, depth: usize, budget: &mut usize, seen: &mut BTreeSet<PathBuf>) {
        if depth > 6 || *budget == 0 {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            let child = dir.join(&e.name);
            if e.kind != EntryKind::Dir || e.name.starts_with('.') || SKIP_DIRS.contains(&e.name.as_str()) {
                continue;
            }
            *budget = budget.saturating_sub(1);
            let f = child.join(name);
            if self.fs().exists(&f) {
                let f = f.clone();
                self.add_context(&f, "project", LoadWhen::OnInvoke, None, 0, seen);
            }
            self.walk_context(&child, name, depth + 1, budget, seen);
            if *budget == 0 {
                return;
            }
        }
    }

    /// One context file and the files it imports (`@path`, at most five hops, repo and user folder only).
    fn add_context(
        &mut self,
        file: &Path,
        layer: &str,
        when: LoadWhen,
        imported_by: Option<String>,
        hops: usize,
        seen: &mut BTreeSet<PathBuf>,
    ) {
        let Ok(real) = self.fs().canonicalize(file) else { return };
        if !seen.insert(real.clone()) {
            return;
        }
        let Ok(text) = self.fs().read_to_string(&real) else { return };
        if text.trim().is_empty() {
            return;
        }
        let name = self.b.display(file);
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Instruction(InstructionAttrs { imported_by: imported_by.clone(), applies_to: Vec::new() }),
            when,
            calc(char_len(text.trim())),
        );
        it.origin.file = Some(name.clone());
        it.load.content_hash = Some(sha256_hex(&text));
        let idx = self.b.push(it);
        let id = self.b.items[idx].id.clone();
        if hops >= IMPORT_MAX_HOPS {
            return;
        }
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        for imp in imports_of(&text) {
            let target = match imp.strip_prefix("~/") {
                Some(rest) => self.home.join(rest),
                None if imp.starts_with('/') => PathBuf::from(&imp),
                None => dir.join(&imp),
            };
            let Ok(t) = self.fs().canonicalize(&target) else { continue };
            if !crate::fs::is_within(&t, &self.p.repo) && !crate::fs::is_within(&t, &self.p.user) {
                self.b.warn("import-outside", "an @import outside the repo and ~/.gemini was not followed", Some(file));
                continue;
            }
            self.add_context(&t, layer, when, Some(id.clone()), hops + 1, seen);
        }
    }

    /// Custom commands: TOML prompts, named by their path under `commands/` with subdirectories as the
    /// namespace. Invoked from chat, so they are skills in the model.
    fn commands(&mut self) {
        let mut idx = Vec::new();
        let layers: [(&str, PathBuf); 2] =
            [("project", self.p.project.join("commands")), ("user", self.p.user.join("commands"))];
        for (layer, dir) in layers {
            for (f, rel) in self.b.walk(&dir, ".toml") {
                let json = self.b.toml(&f);
                let obj = json.object();
                let Some(prompt) = obj.get("prompt").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) else {
                    continue;
                };
                let stem = rel.replace(['/', '\\'], ":").trim_end_matches(".toml").to_string();
                let description = obj.get("description").and_then(Value::as_str).unwrap_or("");
                let listing = char_len(&stem) + char_len(description);
                let body = char_len(prompt.trim());
                let mut it = self.b.item(
                    layer,
                    &stem,
                    ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }),
                    LoadWhen::OnInvoke,
                    calc(listing),
                );
                it.origin.file = Some(self.b.display(&f));
                it.load.content_hash = Some(sha256_hex(prompt));
                idx.push(self.b.push(it));
            }
        }
        // Extensions' commands: lowest precedence, so a user or project command of the same name wins.
        for ext_dir in self.ext_dirs() {
            let Some(manifest) = self.manifest(&ext_dir) else { continue };
            let ext_name = manifest
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| ext_dir.file_name().map(|n| n.to_string_lossy().into_owned()), |s| Some(s.to_string()))
                .unwrap_or_default();
            let ext_commands = ext_dir.join("commands");
            for f in self.b.files_in(&ext_commands, ".toml") {
                let json = self.b.toml(&f);
                let obj = json.object();
                let Some(prompt) = obj.get("prompt").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) else {
                    continue;
                };
                let rel = f.strip_prefix(&ext_commands).unwrap_or(&f).to_string_lossy().replace('\\', "/");
                let stem = format!("{ext_name}.{}", rel.replace('.', "").trim_end_matches(".toml").replace('/', "."));
                let body = char_len(prompt.trim());
                let mut it = self.b.item(
                    "extensions",
                    &stem,
                    ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }),
                    LoadWhen::OnInvoke,
                    calc(char_len(&stem)),
                );
                it.origin.file = Some(self.b.display(&f));
                it.load.content_hash = Some(sha256_hex(prompt));
                idx.push(self.b.push(it));
            }
            // The extension's own context file and the agents it bundles.
            if let Some(context_file) = manifest.get("contextFileName").and_then(Value::as_str) {
                let f = ext_dir.join(context_file);
                let mut seen = BTreeSet::new();
                self.add_context(&f, "extensions", LoadWhen::Startup, None, 0, &mut seen);
            } else {
                let f = ext_dir.join("GEMINI.md");
                let mut seen = BTreeSet::new();
                self.add_context(&f, "extensions", LoadWhen::Startup, None, 0, &mut seen);
            }
            for f in self.b.files_in(&ext_dir.join("agents"), ".md") {
                self.add_agent(&f, "extensions");
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), ext_rank);
    }

    /// Subagent profiles from `.gemini/agents/` (project), `~/.gemini/agents/` (user, inferred) and the
    /// extensions' `agents/` directories.
    fn agents(&mut self) {
        let mut idx = Vec::new();
        let layers: [(&str, PathBuf); 2] =
            [("project", self.p.project.join("agents")), ("user", self.p.user.join("agents"))];
        for (layer, dir) in layers {
            for f in self.b.files_in(&dir, ".md") {
                if let Some(ix) = self.add_agent(&f, layer) {
                    idx.push(ix);
                }
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), ext_rank);
    }

    fn add_agent(&mut self, f: &Path, layer: &str) -> Option<usize> {
        let Ok(text) = self.fs().read_to_string(f) else { return None };
        let fm = frontmatter::parse(&text);
        let name = fm
            .str("name")
            .map(str::to_string)
            .filter(|n| !n.trim().is_empty())
            .or_else(|| f.file_stem().map(|s| s.to_string_lossy().into_owned()))?;
        let body = text[fm.body_start..].trim();
        if body.is_empty() {
            return None;
        }
        let tools = match fm.fields.get("tools").map(FmValue::as_list) {
            Some(ts) if !ts.is_empty() => ToolAccess::Only(ts),
            _ => ToolAccess::All,
        };
        let model = fm.str("model").filter(|m| !m.is_empty()).map(str::to_string);
        let chars = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
        let detail = ItemDetail::Agent(AgentAttrs { tools, model: model.clone() });
        let mut it = self.b.item(layer, &name, detail, LoadWhen::OnInvoke, calc(chars));
        it.origin.file = Some(self.b.display(f));
        it.load.content_hash = Some(sha256_hex(&text));
        Some(self.b.push(it))
    }

    /// Hooks from the `hooks` key in every settings file plus each extension's `hooks/hooks.json`; the
    /// docs merge all layers and every matching hook runs.
    fn hooks(&mut self) {
        for (file, layer, display) in [
            (self.p.system.clone(), "system", self.b.display(&self.p.system)),
            (self.p.project.join("settings.json"), "project", self.b.display(&self.p.project.join("settings.json"))),
            (self.p.user.join("settings.json"), "user", self.b.display(&self.p.user.join("settings.json"))),
        ] {
            if !self.fs().exists(&file) {
                continue;
            }
            let json = self.b.jsonc(&file);
            let obj = json.object();
            if let Some(hooks) = obj.get("hooks").and_then(Value::as_object) {
                hooks_of(self.b, hooks, layer, Some(display.clone()), "");
            }
        }
        let _ = &self.p.system_defaults;
        for ext_dir in self.ext_dirs() {
            let hooks_file = ext_dir.join("hooks/hooks.json");
            if self.fs().exists(&hooks_file) {
                let json = self.b.json(&hooks_file);
                let obj = json.object();
                let hooks = obj.get("hooks").and_then(Value::as_object).unwrap_or(&obj);
                let display = format!("ext:{}", self.b.display(&hooks_file));
                hooks_of(self.b, hooks, "extensions", Some(display), "");
            }
        }
    }

    /// The extensions' manifests: their MCP servers are shadowed by any same-named server in a
    /// settings file, and their skills, agents and policies carry their own items.
    /// The installed extensions' directories, if any.
    fn ext_dirs(&self) -> Vec<PathBuf> {
        self.b.dirs_in(&self.p.user.join("extensions"))
    }

    fn manifest(&mut self, ext_dir: &Path) -> Option<Map<String, Value>> {
        let f = ext_dir.join("gemini-extension.json");
        if !self.fs().exists(&f) {
            return None;
        }
        let json = self.b.json(&f);
        Some(json.object())
    }

    /// MCP servers from every settings file and from the extensions' manifests. A settings file's server
    /// wins the same name over an extension's.
    fn mcp(&mut self) {
        let mut idx = Vec::new();
        for (file, layer, display) in [
            (self.p.system.clone(), "system", self.b.display(&self.p.system)),
            (self.p.project.join("settings.json"), "project", self.b.display(&self.p.project.join("settings.json"))),
            (self.p.user.join("settings.json"), "user", self.b.display(&self.p.user.join("settings.json"))),
            (self.p.system_defaults.clone(), "system_defaults", self.b.display(&self.p.system_defaults)),
        ] {
            if !self.fs().exists(&file) {
                continue;
            }
            let json = self.b.jsonc(&file);
            let obj = json.object();
            if let Some(Value::Object(servers)) = obj.get("mcpServers") {
                for name in servers.keys() {
                    let cfg = servers.get(name).cloned().unwrap_or(Value::Null);
                    idx.push(self.declare_mcp(name, &cfg, layer, &display));
                }
            }
        }
        for ext_dir in self.ext_dirs() {
            let Some(manifest) = self.manifest(&ext_dir) else { continue };
            if let Some(Value::Object(servers)) = manifest.get("mcpServers") {
                for name in servers.keys() {
                    let cfg = servers.get(name).cloned().unwrap_or(Value::Null);
                    idx.push(self.declare_mcp(name, &cfg, "extensions", "gemini-extension.json"));
                }
            }
        }
        // The settings files win a same-name server over an extension's.
        self.b.resolve_clashes(&idx, |i| i.name.clone(), ext_rank);
    }

    /// One MCP server: a url keeps its origin, a command its file name; args and env never surface.
    fn declare_mcp(&mut self, name: &str, cfg: &Value, layer: &str, file: &str) -> usize {
        let (transport, target) = if let Some(u) = cfg.get("url").and_then(Value::as_str) {
            (McpTransport::Http, url_origin(u))
        } else if let Some(c) = cfg.get("command").and_then(Value::as_str) {
            (McpTransport::Stdio, Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned()))
        } else {
            (McpTransport::Unknown, None)
        };
        let attrs = McpAttrs { transport, target, state: McpState::Active };
        let mut it = self.b.item(layer, name, ItemDetail::Mcp(attrs), LoadWhen::Startup, calc(0));
        it.origin.file = Some(file.to_string());
        self.b.push(it)
    }

    /// The policy engine's TOML files at the user tier, plus each extension's `policies/` directory.
    /// Each `[[rule]]` becomes one permission item per tool name.
    fn policies(&mut self) {
        let user_policies = self.p.user.join("policies");
        for f in self.b.files_in(&user_policies, ".toml") {
            self.add_policies(&f, "user");
        }
        for ext_dir in self.ext_dirs() {
            for f in self.b.files_in(&ext_dir.join("policies"), ".toml") {
                self.add_policies(&f, "extensions");
            }
        }
    }

    fn add_policies(&mut self, file: &Path, layer: &str) {
        let json = self.b.toml(file);
        let obj = json.object();
        let Some(rules) = obj.get("rule").and_then(Value::as_array).cloned() else { return };
        for rule in rules {
            let decision = rule.get("decision").and_then(Value::as_str).unwrap_or("");
            let effect = match decision {
                "allow" => PermissionEffect::Allow,
                "deny" => PermissionEffect::Deny,
                "ask_user" => PermissionEffect::Ask,
                _ => continue,
            };
            let tools = match rule.get("toolName") {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>(),
                Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
                _ => continue,
            };
            // The pattern: a shell command's prefix, else the args regex; printable only.
            for t in tools {
                let pattern = rule
                    .get("commandPrefix")
                    .and_then(Value::as_str)
                    .or_else(|| rule.get("argsPattern").and_then(Value::as_str))
                    .and_then(|p| (!printable(p).is_empty()).then(|| printable(p).to_string()));
                let attrs = PermissionAttrs { effect, tool: t, pattern };
                let mut it = self.b.item(
                    layer,
                    &format!("policy:{}", attrs.tool),
                    ItemDetail::Permission(attrs),
                    LoadWhen::Never,
                    calc(0),
                );
                it.origin.file = Some(self.b.display(file));
                self.b.push(it);
            }
        }
    }
}
