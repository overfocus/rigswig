//! The GitHub Copilot CLI adapter. Every rule it models is in `docs/adapters/copilot.md`, with the docs it comes
//! from (docs.github.com for Copilot CLI; all read 2026-11-06).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::adapters::shared::hooks_of;
use crate::model::*;
use crate::read::frontmatter::{self, FmValue};
use crate::read::redact::{printable, sha256_hex, url_origin};
use serde_json::Map;

pub const ID: &str = "copilot";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release era the docs this adapter follows describe (the v0.0.36x prerelease line; read 2026-11-06).
const TOOL_VERSION: &str = "0.0.36x";

pub struct Copilot;

struct Paths {
    home: PathBuf,
    github: PathBuf,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        Self {
            home: ctx.var("COPILOT_HOME").map_or_else(|| ctx.home.join(".copilot"), PathBuf::from),
            github: ctx.repo.clone().join(".github"),
            repo: ctx.repo.clone(),
        }
    }
}

impl Adapter for Copilot {
    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["COPILOT_HOME"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let has = |f: &Path| ctx.fs.exists(f);
        let checks = [
            (p.github.join("copilot-instructions.md"), ".github/copilot-instructions.md", 100),
            (p.github.join("copilot"), ".github/copilot/", 100),
            (p.github.join("agents"), ".github/agents/", 80),
            (p.github.join("hooks"), ".github/hooks/", 60),
            (p.github.join("skills"), ".github/skills/", 60),
            (p.github.join("instructions"), ".github/instructions/", 60),
            (p.github.join("mcp.json"), ".github/mcp.json", 60),
            (p.repo.join("AGENTS.md"), "AGENTS.md", 40),
            (p.home, "Copilot CLI folder", 45),
        ];
        let mut score = 0;
        let mut evidence = Vec::new();
        for (path, label, s) in checks {
            if has(&path) {
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

        // The settings chains carry their trail: user over repository over local, on top of the defaults this
        // adapter cannot see and the MDM baseline it cannot read.
        let user_cfg = b.jsonc(&p.home.join("settings.json")).object();
        let repo_cfg = b.jsonc(&p.github.join("copilot/settings.json")).object();
        let local_cfg = b.jsonc(&p.github.join("copilot/settings.local.json")).object();

        b.add_layer(
            "user",
            LayerKind::User,
            "the Copilot CLI home ($COPILOT_HOME, default ~/.copilot)",
            Some(b.display(&p.home)),
            has(&p.home.join("copilot-instructions.md"))
                || !b.files_in(&p.home.join("instructions"), ".md").is_empty()
                || has(&p.home.join("skills"))
                || has(&p.home.join("agents"))
                || has(&p.home.join("hooks"))
                || has(&p.home.join("mcp-config.json"))
                || !user_cfg.is_empty(),
            LayerMode::Default,
        );
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .github customizations and its root instruction files",
            Some(b.display(&p.github)),
            has(&p.github.join("copilot-instructions.md"))
                || !b.files_in(&p.github.join("instructions"), ".md").is_empty()
                || has(&p.github.join("agents"))
                || has(&p.github.join("hooks"))
                || has(&p.github.join("skills"))
                || has(&p.github.join("copilot/settings.json"))
                || has(&p.github.join("copilot/settings.local.json"))
                || has(&p.github.join("mcp.json"))
                || has(&p.repo.join(".mcp.json"))
                || has(&p.repo.join("AGENTS.md"))
                || has(&p.repo.join("CLAUDE.md"))
                || has(&p.repo.join("GEMINI.md")),
            LayerMode::Default,
        );
        b.add_layer(
            "imports_project",
            LayerKind::Project,
            "the repo's Claude settings the cross-tool subset is read from",
            None,
            has(&p.repo.join(".claude/settings.json")) || has(&p.repo.join(".claude/settings.local.json")),
            LayerMode::Default,
        );
        b.add_layer(
            "imports_user",
            LayerKind::User,
            "the home's Claude settings the cross-tool subset is read from",
            None,
            has(&ctx.home.join(".claude/settings.json")) || has(&ctx.home.join(".claude/settings.local.json")),
            LayerMode::Default,
        );

        b.add_layer(
            "local",
            LayerKind::Local,
            "the gitignored personal settings",
            Some(b.display(&p.github.join("copilot/settings.local.json"))),
            !local_cfg.is_empty(),
            LayerMode::Default,
        );
        let mut run = Run { b: &mut b, p, home: ctx.home.clone(), user_cfg, repo_cfg, local_cfg };
        run.settings();
        run.instructions();
        run.skills();
        run.agents();
        run.hooks();
        run.mcp();
        run.b.link_packages();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the built-in tools, the model and the GitHub MCP server".into(),
            why: "they come with Copilot CLI, not from any file".into(),
        });
        run.b.not_measured_common();

        for note in [
            "MDM managed settings and the built-in defaults: the two layers beneath the user settings, which rigswig cannot read",
            "the `permissions-config.json` approval record (`~/.copilot/permissions-config.json`): the CLI's saved tool and directory approvals per location, written automatically",
            "BYOK providers and models (`~/.copilot/providers.json`): credentials in plain text, which rigswig never reads; its models would join the /model picker, not the context",
            "plugins (installed-plugins/, plugin-data/, enabledPlugins and repository defaults) and extensions (extensions/): what they mount is theirs to declare, and the CLI's plugin sources are not files this adapter can see",
            "LSP servers (`~/.copilot/lsp-config.json` and `.github/lsp.json`): language intelligence for the CLI's tools, not context",
            "which skills a session's disabledSkills setting and trust state keep out, and the folder-trust requirement on workspace MCP files",
            "Copilot cloud-agent surfaces (organization instructions, repository MCP on GitHub.com, enterprise-level agents): they follow the repo on GitHub, not the machine",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        drop(run);
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

struct Run<'b, 'a> {
    b: &'b mut Build<'a>,
    p: Paths,
    home: PathBuf,
    user_cfg: Map<String, Value>,
    repo_cfg: Map<String, Value>,
    local_cfg: Map<String, Value>,
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// The settings that decide behaviour, with the docs' merge order: user over repository over local, on top
    /// of defaults and MDM this adapter cannot read.
    fn settings(&mut self) {
        // In override order, strongest first: the docs put the repository's file over the user's and the
        // gitignored local file over both.
        let layers =
            [("local", self.local_cfg.clone()), ("project", self.repo_cfg.clone()), ("user", self.user_cfg.clone())];

        // `model` is either a plain name or the settings object's `model.name`.
        let model_of = |cfg: &Map<String, Value>| -> Option<Value> {
            match cfg.get("model") {
                Some(Value::String(name)) => Some(Value::String(name.clone())),
                Some(Value::Object(inner)) => {
                    inner.get("name").filter(|n| !n.is_null()).cloned().or(Some(Value::String(String::new())))
                }
                _ => None,
            }
        };
        {
            let trail: Vec<(String, Value)> =
                layers.iter().filter_map(|(l, cfg)| model_of(cfg).map(|v| ((*l).to_string(), v))).collect();
            if !trail.is_empty() {
                let steps = trail
                    .iter()
                    .map(|(l, v)| TrailStep {
                        layer: l.clone(),
                        value: match v {
                            Value::String(s) => Value::String(printable(s)),
                            other => other.clone(),
                        },
                        note: None,
                    })
                    .collect();
                let (source, value) = trail.first().cloned().unwrap_or_default();
                self.b.settings.insert(String::from("model"), Decision { value, source: Some(source), trail: steps });
            }
        }
        for (path, name) in [
            (&["effortLevel"][..], "effort"),
            (&["allowedUrls"][..], "allowedUrls"),
            (&["disabledSkills"][..], "disabledSkills"),
        ] {
            let mut trail: Vec<(String, Value)> = Vec::new();
            for (layer, cfg) in &layers {
                if let Some(v) = at(cfg, path) {
                    trail.push(((*layer).to_string(), v));
                }
            }
            if trail.is_empty() {
                continue;
            }
            let steps = trail
                .iter()
                .map(|(l, v)| TrailStep {
                    layer: l.clone(),
                    value: match v {
                        Value::String(s) => Value::String(printable(s)),
                        other => other.clone(),
                    },
                    note: None,
                })
                .collect();
            let (source, value) = trail.first().cloned().unwrap_or_default();
            self.b.settings.insert(name.to_string(), Decision { value, source: Some(source), trail: steps });
        }
    }

    /// The instructions: the personal chains every session, the repository-wide file every session, the
    /// path-specific files on match, and the agent family at the root.
    fn instructions(&mut self) {
        // Personal: copilot-instructions.md, then the topic files in instructions/.
        let global = self.p.home.join("copilot-instructions.md");
        self.add_startup(&global, "user");
        for f in self.b.files_in(&self.p.home.join("instructions"), ".md") {
            self.add_instructions_file(&f, "user", false);
        }
        // Repository-wide: every session.
        self.add_startup(&self.p.github.join("copilot-instructions.md"), "project");
        // Path-specific: applyTo's patterns make them on-match instructions.
        for (f, _rel) in self.b.walk(&self.p.github.join("instructions"), ".md") {
            self.add_instructions_file(&f, "project", true);
        }
        // The agent family at the repo root, additive.
        for name in ["AGENTS.md", "CLAUDE.md", "GEMINI.md"] {
            self.add_startup(&self.p.repo.join(name), "project");
        }
    }

    /// One always-on instruction file, measured against its text.
    fn add_startup(&mut self, file: &Path, layer: &str) {
        let Ok(text) = self.fs().read_to_string(file) else { return };
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let name = self.b.display(file);
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Instruction(InstructionAttrs::default()),
            LoadWhen::Startup,
            calc(char_len(body)),
        );
        it.origin.file = Some(name.clone());
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// One `*.instructions.md`: its `applyTo` frontmatter (a string or a list of globs) makes it an on-match
    /// instruction; without one it loads with everything else.
    fn add_instructions_file(&mut self, file: &Path, layer: &str, from_repo: bool) {
        let Ok(text) = self.fs().read_to_string(file) else { return };
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let fm = frontmatter::parse(&text);
        let applies: Vec<String> = fm
            .fields
            .get("applyTo")
            .map(FmValue::as_list)
            .unwrap_or_default()
            .iter()
            .filter(|p| !p.is_empty())
            .cloned()
            .collect();
        let (when, applies_to) = match (!applies.is_empty(), from_repo) {
            (true, _) => (LoadWhen::OnMatch, applies),
            // A personal topic file without applyTo loads like the global one; one with applyTo stays scoped.
            (false, true) => (LoadWhen::Startup, Vec::new()),
            (false, false) => (LoadWhen::Startup, Vec::new()),
        };
        let name = self.b.display(file);
        let body_chars = char_len(text[fm.body_start..].trim());
        let chars = if body_chars > 0 { body_chars + char_len(fm.str("description").unwrap_or("")) } else { 0 };
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to }),
            when,
            calc(chars),
        );
        it.origin.file = Some(name.clone());
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// Skills from every source family, invoked by name; project roots beat the personal ones on the same name.
    fn skills(&mut self) {
        let roots: [(&str, PathBuf, u32); 5] = [
            ("project", self.p.github.join("skills"), 0),
            ("imports_project", self.p.repo.join(".claude/skills"), 1),
            ("imports_project", self.p.repo.join(".agents/skills"), 2),
            ("user", self.p.home.join("skills"), 3),
            // The docs' second personal root; its default location is inferred (~/.agents).
            ("imports_user", self.home.join(".agents/skills"), 4),
        ];
        let mut idx = Vec::new();
        // Same-name priority, keyed by each item's origin file (project roots run first).
        let priorities: std::collections::BTreeMap<String, u32> = roots
            .iter()
            .flat_map(|(_, dir, rank)| {
                self.b
                    .dirs_in(dir)
                    .into_iter()
                    .map(|d| (self.b.display(&d.join("SKILL.md")), *rank))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (layer, dir, _rank) in roots {
            for d in self.b.dirs_in(&dir) {
                let skill_md = d.join("SKILL.md");
                if self.fs().exists(&skill_md) {
                    if let Some(ix) = self.add_skill(&skill_md, layer) {
                        idx.push(ix);
                    }
                }
            }
        }
        self.b.resolve_clashes(
            &idx,
            |i| i.name.clone(),
            |i| i.origin.file.as_deref().and_then(|f| priorities.get(f)).copied().unwrap_or(9),
        );
    }

    /// One SKILL.md: named by its folder, invoked by name, so the listing (name + description) is measured at
    /// startup and the body only when the skill runs.
    fn add_skill(&mut self, f: &Path, layer: &str) -> Option<usize> {
        let Ok(text) = self.fs().read_to_string(f) else { return None };
        let fm = frontmatter::parse(&text);
        let body = text[fm.body_start..].trim();
        if body.is_empty() {
            return None;
        }
        let name = fm.str("name").map(str::to_string).unwrap_or_else(|| {
            f.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        });
        if name.is_empty() {
            return None;
        }
        let listing = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Skill(SkillAttrs { body_chars: Some(char_len(body)) }),
            LoadWhen::OnInvoke,
            calc(listing),
        );
        it.origin.file = Some(self.b.display(f));
        it.load.content_hash = Some(sha256_hex(&text));
        Some(self.b.push(it))
    }

    /// Custom agents from the repo's .github/agents and the user's home agents; a personal agent wins the same
    /// ID (the docs' inverted precedence, pinned).
    fn agents(&mut self) {
        let mut idx = Vec::new();
        for (layer, dir) in [("project", self.p.github.join("agents")), ("user", self.p.home.join("agents"))] {
            let mut found = Vec::new();
            self.walk_agents(&dir, 0, "", &mut found);
            for (id, f) in found {
                if let Some(ix) = self.add_agent(&f, layer, &id) {
                    idx.push(ix);
                }
            }
        }
        // The docs put the personal agent ahead on the same ID (pinned): user 0, project 1. Each priority is
        // keyed by the item's origin file.
        let priorities: std::collections::BTreeMap<String, u32> =
            [(self.p.home.join("agents"), 0u32), (self.p.github.join("agents"), 1u32)]
                .iter()
                .flat_map(|(dir, rank)| {
                    self.b.files_in(dir, ".md").into_iter().map(|f| (self.b.display(&f), *rank)).collect::<Vec<_>>()
                })
                .collect();
        self.b.resolve_clashes(
            &idx,
            |i| i.name.clone(),
            |i| i.origin.file.as_deref().and_then(|f| priorities.get(f)).copied().unwrap_or(9),
        );
    }

    /// Every `.agent.md` under `dir`: the agent's ID is its file name without `.agent.md`/`.md`, with a
    /// subdirectory's name prefixing it joined by `--` (the docs' ID form).
    fn walk_agents(&self, dir: &Path, depth: usize, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
        if depth > 4 {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            let child = dir.join(&e.name);
            if e.kind == crate::fs::EntryKind::Dir {
                let sub = if prefix.is_empty() { e.name.clone() } else { format!("{prefix}--{}", e.name) };
                self.walk_agents(&child, depth + 1, &sub, out);
            } else if e.name.ends_with(".md") {
                let stem = e.name.trim_end_matches(".md").trim_end_matches(".agent").to_string();
                let id = if prefix.is_empty() { stem } else { format!("{prefix}--{stem}") };
                out.push((id, child));
            }
        }
    }

    /// One `.agent.md`: the file stem is the agent's ID with its subdirectory prefix (a `name` frontmatter is a
    /// display label, which the docs say does not deduplicate); tools keep the list, or everything when unset.
    fn add_agent(&mut self, f: &Path, layer: &str, agent_id: &str) -> Option<usize> {
        let Ok(text) = self.fs().read_to_string(f) else { return None };
        let fm = frontmatter::parse(&text);
        let body = text[fm.body_start..].trim();
        if body.is_empty() {
            return None;
        }
        let tools = match fm.fields.get("tools").map(FmValue::as_list) {
            Some(ts) if !ts.is_empty() => ToolAccess::Only(ts),
            _ => ToolAccess::All,
        };
        let model = fm.str("model").filter(|m| !m.is_empty()).map(str::to_string);
        let stem = agent_id.rsplit("--").next().unwrap_or(agent_id);
        let chars = char_len(stem) + char_len(fm.str("description").unwrap_or(""));
        let detail = ItemDetail::Agent(AgentAttrs { tools, model: model.clone() });
        let mut it = self.b.item(layer, agent_id, detail, LoadWhen::OnInvoke, calc(chars));
        it.origin.file = Some(self.b.display(f));
        it.load.content_hash = Some(sha256_hex(&text));
        Some(self.b.push(it))
    }

    /// Hooks: the repository's `.github/hooks/*.json`, the user's home `hooks/` JSON files, each settings
    /// layer's inline `hooks` key, and Claude's settings for the cross-tool subset. Every layer's matching
    /// hooks run, so the sources are additive.
    fn hooks(&mut self) {
        for (dir, layer) in [(self.p.github.join("hooks"), "project"), (self.p.home.join("hooks"), "user")] {
            for f in self.b.files_in(&dir, ".json") {
                let json = self.b.json(&f);
                let obj = json.object();
                let display = self.b.display(&f);
                if let Some(hooks) = obj.get("hooks").and_then(Value::as_object) {
                    hooks_flat(self.b, hooks, layer, display);
                } else if !obj.is_empty() {
                    // A hook file whose top level is the event map itself.
                    hooks_flat(self.b, &obj, layer, display);
                }
            }
        }
        for (cfg, layer, prefix) in [
            (&self.user_cfg, "user", "settings:"),
            (&self.repo_cfg, "project", "settings:"),
            (&self.local_cfg, "local", "settings.local:"),
        ] {
            // The `hooks` key in Copilot's own settings files uses the flat entry shape.
            if let Some(Value::Object(hooks)) = cfg.get("hooks") {
                hooks_flat(self.b, hooks, layer, format!("{prefix}settings.json"));
            }
        }
        // The cross-tool subset: Claude's settings files, whose hooks keep Claude's shape.
        for (file, layer) in [
            (self.p.repo.join(".claude/settings.json"), "imports_project"),
            (self.p.repo.join(".claude/settings.local.json"), "imports_project"),
            (self.home.join(".claude/settings.json"), "imports_user"),
            (self.home.join(".claude/settings.local.json"), "imports_user"),
        ] {
            if !self.fs().exists(&file) {
                continue;
            }
            let json = self.b.json(&file);
            if let Some(Value::Object(hooks)) = json.object().get("hooks") {
                hooks_of(self.b, hooks, layer, Some(self.b.display(&file)), "");
            }
        }
    }

    /// MCP servers: the user's `mcp-config.json`, the repo's `.github/mcp.json`, and the repo's `.mcp.json`.
    /// The workspace files outrank the user's on a name conflict; `local`/`stdio` keep their command's file
    /// name, `http`/`sse` keep the origin. Args, env and headers never surface.
    fn mcp(&mut self) {
        let ranks: [(&Path, u32); 3] = [
            (&self.p.home.join("mcp-config.json"), 1),
            (&self.p.github.join("mcp.json"), 0),
            (&self.p.repo.join(".mcp.json"), 0),
        ];
        let mut idx = Vec::new();
        for (file, layer) in [
            (self.p.home.join("mcp-config.json"), "user"),
            (self.p.github.join("mcp.json"), "project"),
            (self.p.repo.join(".mcp.json"), "project"),
        ] {
            if !self.fs().exists(&file) {
                continue;
            }
            let json = self.b.json(&file);
            if let Some(servers) = json.object().get("mcpServers").and_then(Value::as_object).cloned() {
                let display = self.b.display(&file);
                for (name, cfg) in &servers {
                    idx.push(self.declare_mcp(name, cfg, layer, &display));
                }
            }
        }
        // The workspace files outrank the user's on a name conflict (pinned).
        let priorities: std::collections::BTreeMap<String, u32> =
            ranks.iter().filter(|(f, _)| self.fs().exists(f)).map(|(f, r)| (self.b.display(f), *r)).collect();
        self.b.resolve_clashes(
            &idx,
            |i| i.name.clone(),
            |i| i.origin.file.as_deref().and_then(|f| priorities.get(f)).copied().unwrap_or(9),
        );
    }

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
}

/// The flat event map in a hook file: `{ event: [ { type, bash, powershell, ... } ] }`, one item per entry.
fn hooks_flat(b: &mut Build, block: &Map<String, Value>, layer: &str, file: String) {
    let neutral = |e: &str| {
        // Copilot's camelCase event name mapped to the shared neutral spellings.
        match e {
            "userPromptSubmitted" => "prompt-submit".into(),
            other => crate::adapters::shared::neutral_event(other),
        }
    };
    for (event, defs) in block {
        let Some(defs) = defs.as_array() else { continue };
        for (n, d) in defs.iter().enumerate() {
            let script = d
                .get("bash")
                .and_then(Value::as_str)
                .or_else(|| d.get("powershell").and_then(Value::as_str))
                .and_then(crate::read::redact::script_of);
            let attrs = HookAttrs {
                event: neutral(event),
                native_event: event.clone(),
                matcher: d.get("matcher").and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from),
                handler: HookHandler::Command,
                script,
            };
            let mut it =
                b.item(layer, &format!("{event}-{}", n + 1), ItemDetail::Hook(attrs), LoadWhen::Never, calc(0));
            it.origin.file = Some(file.clone());
            b.push(it);
        }
    }
}

/// A dotted path's value, when every step resolves and the end is not null.
fn at(cfg: &Map<String, Value>, path: &[&str]) -> Option<Value> {
    let mut at = cfg;
    for (i, k) in path.iter().enumerate() {
        let next = at.get(*k)?;
        if i + 1 == path.len() {
            return if next.is_null() { None } else { Some(next.clone()) };
        }
        at = next.as_object()?;
    }
    None
}
