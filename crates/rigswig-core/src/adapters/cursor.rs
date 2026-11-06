//! The Cursor adapter. Every rule it models is in `docs/adapters/cursor.md`, with the docs it comes
//! from (all read 2026-11-06).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::adapters::shared::{hooks_of, neutral_event};
use crate::fs::EntryKind;
use crate::model::*;
use crate::read::frontmatter;
use crate::read::redact::{printable, sha256_hex, url_origin};

pub const ID: &str = "cursor";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The Cursor release the docs this adapter follows describe (2.5 era: nested AGENTS.md, subagents,
/// commands migrating into skills; read 2026-11-06).
const TOOL_VERSION: &str = "2.5";

pub struct Cursor;

/// Heavy directories the lazy AGENTS.md walk does not enter.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "vendor", "build", "coverage", ".venv", "venv"];

struct Paths {
    user: PathBuf,
    project: PathBuf,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        Self { user: ctx.home.join(".cursor"), project: ctx.repo.join(".cursor"), repo: ctx.repo.clone() }
    }
}

impl Adapter for Cursor {
    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &[]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.project, ".cursor/", 100),
            (p.repo.join(".cursorrules"), ".cursorrules", 60),
            (p.repo.join(".cursorignore"), ".cursorignore", 30),
            (p.user, "Cursor folder", 45),
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

        b.add_layer(
            "user",
            LayerKind::User,
            "the user config directory",
            Some(b.display(&p.user)),
            has(&p.user.join("hooks.json"))
                || has(&p.user.join("mcp.json"))
                || !b.walk(&p.user.join("skills"), "SKILL.md").is_empty()
                || !b.files_in(&p.user.join("agents"), ".md").is_empty()
                || !b.files_in(&p.user.join("commands"), ".md").is_empty(),
            LayerMode::Default,
        );
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .cursor directory and its root markdown",
            Some(b.display(&p.project)),
            has(&p.project) || has(&p.repo.join("AGENTS.md")) || has(&p.repo.join(".cursorrules")),
            LayerMode::Default,
        );
        let imports_project_present = has(&p.repo.join(".claude"))
            || has(&p.repo.join(".codex"))
            || !b.walk(&p.repo.join(".agents/skills"), "SKILL.md").is_empty();
        b.add_layer(
            "imports_project",
            LayerKind::Project,
            "the repo's Claude and Codex files Cursor reads for compatibility",
            None,
            imports_project_present,
            LayerMode::Default,
        );
        b.add_layer(
            "imports_user",
            LayerKind::User,
            "the home's Claude and Codex files Cursor reads for compatibility",
            None,
            has(&ctx.home.join(".claude/skills"))
                || has(&ctx.home.join(".codex/skills"))
                || has(&ctx.home.join(".claude/agents"))
                || has(&ctx.home.join(".codex/agents")),
            LayerMode::Default,
        );

        let mut run = Run { b: &mut b, p, home: ctx.home.clone() };
        run.instructions();
        run.skills();
        run.agents();
        run.hooks();
        run.mcp();
        run.b.link_packages();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the built-in tools and the model".into(),
            why: "they come with Cursor, not from any file".into(),
        });
        run.b.not_measured_common();

        for note in [
            "User Rules (Customize → Rules) and Team Rules from the dashboard: neither lives on disk, so rigswig cannot read either",
            "Cursor's own settings, tool toggles and auto-run approvals: they live in the app's database, not in config files",
            "which repo files a glob rule fires for: decided at runtime against the files in context, so such rules are listed as load-on-match",
            "the @file mentions inside a rule: the file's contents are not inlined into the prompt; the agent reads them with its tools when it needs them",
            "skills under nested .cursor directories in a monorepo and each skill's `paths` frontmatter: both scope skills by the files being worked on, which their listing cannot show",
            "the MCP servers the extension API registers: written to the app's database, not to mcp.json",
            "the enterprise-managed and team hooks Cursor runs from its cloud dashboard, and which cloud-agent turns (read-only early turns) run hooks at all",
            "the `.cursorignore` access list and the default ignore list: they keep files from the agent's tools, they do not load anything into context",
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
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// Instructions: `.cursor/rules/**.mdc` with their frontmatter, AGENTS.md at the root and its
    /// subdirectories, and the legacy root `.cursorrules`.
    fn instructions(&mut self) {
        for (f, rel) in self.b.walk(&self.p.project.join("rules"), ".mdc") {
            let Some(text) = self.rule_text(&f) else { continue };
            let fm = frontmatter::parse(&text);
            let always = fm.str("alwaysApply").is_some_and(|a| a == "true");
            let globs = fm.str("globs").unwrap_or("").trim().to_string();
            let description = fm.str("description").unwrap_or("").trim().to_string();
            let body = text[fm.body_start..].trim();
            if body.is_empty() {
                continue;
            }
            let (when, applies_to) = if always {
                (LoadWhen::Startup, Vec::new())
            } else if !globs.is_empty() {
                (LoadWhen::OnMatch, globs.split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect())
            } else if !description.is_empty() {
                // The agent reads the description and pulls the rule in when relevant.
                (LoadWhen::OnInvoke, Vec::new())
            } else {
                // Only an @-mention applies a rule with no frontmatter fields.
                (LoadWhen::OnInvoke, Vec::new())
            };
            let name = format!("{}/{}", self.b.display(&self.p.project.join("rules")), rel);
            let mut it = self.b.item(
                "project",
                &name,
                ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to }),
                when,
                calc(char_len(body)),
            );
            it.origin.file = Some(name.clone());
            it.load.content_hash = Some(sha256_hex(&text));
            self.b.push(it);
        }

        // AGENTS.md: the root every session, subdirectories only when the agent works in them.
        let root = self.p.repo.join("AGENTS.md");
        self.add_doc(&root, "project", LoadWhen::Startup);
        let repo = self.p.repo.clone();
        let mut seen = BTreeSet::new();
        self.walk_agents_md(repo.as_path(), 0, &mut seen);

        // The legacy root file, kept working by a settings toggle.
        if self.fs().exists(&self.p.repo.join(".cursorrules")) {
            self.b.warn(
                "cursorrules-legacy",
                "the deprecated .cursorrules file is present; Cursor reads it only when 'Include .cursorrules file' is on",
                None,
            );
            let text = self.fs().read_to_string(&self.p.repo.join(".cursorrules")).unwrap_or_default();
            if !text.trim().is_empty() {
                self.add_doc(&self.p.repo.join(".cursorrules"), "project", LoadWhen::Startup);
            }
        }
    }

    /// Nested AGENTS.md, one item per file; the docs do not give a walk budget (inferred: depth 6).
    fn walk_agents_md(&mut self, dir: &Path, depth: usize, seen: &mut BTreeSet<PathBuf>) {
        if depth > 6 {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            // The repo root's own AGENTS.md is added above; only subdirectories join here.
            if (SKIP_DIRS.contains(&e.name.as_str()) || e.name.starts_with('.')) && depth > 0 {
                continue;
            }
            if e.kind == EntryKind::Dir && (SKIP_DIRS.contains(&e.name.as_str()) || e.name.starts_with('.')) {
                continue;
            }
            let child = dir.join(&e.name);
            if e.kind == EntryKind::Dir {
                self.walk_agents_md(&child, depth + 1, seen);
            } else if depth > 0 && e.name == "AGENTS.md" {
                let Ok(real) = self.fs().canonicalize(&child) else { continue };
                if seen.insert(real) {
                    self.add_doc(&child, "project", LoadWhen::OnInvoke);
                }
            }
        }
    }

    /// One plain instruction file, measured against its whole text.
    fn add_doc(&mut self, file: &Path, layer: &str, when: LoadWhen) {
        let Ok(text) = self.fs().read_to_string(file) else { return };
        let body = text.trim();
        if body.is_empty() {
            return;
        }
        let name = self.b.display(file);
        let mut it =
            self.b.item(layer, &name, ItemDetail::Instruction(InstructionAttrs::default()), when, calc(char_len(body)));
        it.origin.file = Some(name.clone());
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// The `.mdc` text, or None for a rule whose frontmatter or body is missing.
    fn rule_text(&self, file: &Path) -> Option<String> {
        let text = self.fs().read_to_string(file).ok()?;
        let fm = frontmatter::parse(&text);
        if fm.body_start == 0 { None } else { Some(text) }
    }

    /// Skills from every source family, invoked by name.
    fn skills(&mut self) {
        for (layer, dir) in [
            ("project", self.p.project.join("skills")),
            ("user", self.p.user.join("skills")),
            ("imports_project", self.p.repo.join(".agents/skills")),
            ("imports_project", self.p.repo.join(".claude/skills")),
            ("imports_project", self.p.repo.join(".codex/skills")),
            ("imports_user", self.home.join(".agents/skills")),
            ("imports_user", self.home.join(".claude/skills")),
            ("imports_user", self.home.join(".codex/skills")),
        ] {
            for (f, _rel) in self.b.walk(&dir, "SKILL.md") {
                self.add_skill(&f, layer);
            }
        }
        // Legacy slash commands, which Cursor migrates into skills.
        for (layer, dir) in [("project", self.p.project.join("commands")), ("user", self.p.user.join("commands"))] {
            for f in self.b.files_in(&dir, ".md") {
                let Some(text) = self.fs().read_to_string(&f).ok().filter(|t| !t.trim().is_empty()) else {
                    continue;
                };
                let stem = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let name = format!("{stem}.md");
                let mut it = self.b.item(
                    layer,
                    &name,
                    ItemDetail::Skill(SkillAttrs { body_chars: Some(char_len(text.trim())) }),
                    LoadWhen::OnInvoke,
                    calc(char_len(&stem)),
                );
                it.origin.file = Some(self.b.display(&f));
                self.b.push(it);
            }
        }
    }

    /// One SKILL.md: invoked by name (`/name` or `@name`), so its listing is measured at startup and the
    /// body only when the skill runs.
    fn add_skill(&mut self, f: &Path, layer: &str) {
        let Ok(text) = self.fs().read_to_string(f) else { return };
        let fm = frontmatter::parse(&text);
        let body = text[fm.body_start..].trim();
        // The name comes from the folder that holds SKILL.md, not the category folder above it.
        let Some(name) = f.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()) else {
            return;
        };
        if body.is_empty() {
            return;
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
        self.b.push(it);
    }

    /// Subagent profiles from the project and user directories, plus Claude and Codex's own folders.
    fn agents(&mut self) {
        for (layer, dir) in [
            ("project", self.p.project.join("agents")),
            ("user", self.p.user.join("agents")),
            ("imports_project", self.p.repo.join(".claude/agents")),
            ("imports_project", self.p.repo.join(".codex/agents")),
            ("imports_user", self.home.join(".claude/agents")),
            ("imports_user", self.home.join(".codex/agents")),
        ] {
            for f in self.b.files_in(&dir, ".md") {
                let Ok(text) = self.fs().read_to_string(&f) else { continue };
                let fm = frontmatter::parse(&text);
                let Some(name) = fm
                    .str("name")
                    .map(str::to_string)
                    .filter(|n| !n.trim().is_empty())
                    .or_else(|| f.file_stem().map(|s| s.to_string_lossy().into_owned()))
                else {
                    continue;
                };
                let body = text[fm.body_start..].trim();
                let model = fm.str("model").filter(|m| !m.is_empty() && *m != "inherit").map(str::to_string);
                let chars = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
                let detail = ItemDetail::Agent(AgentAttrs { tools: ToolAccess::All, model: model.clone() });
                let mut it = self.b.item(layer, &name, detail, LoadWhen::OnInvoke, calc(chars));
                it.origin.file = Some(self.b.display(&f));
                if !body.is_empty() {
                    it.load.content_hash = Some(sha256_hex(&text));
                }
                self.b.push(it);
            }
        }
    }

    /// Hooks from Cursor's own file shape (project and user) and Claude's settings (the compat import).
    fn hooks(&mut self) {
        let cursor_hooks: [(&str, PathBuf, &str); 2] = [
            ("cursor", self.p.project.join("hooks.json"), "project"),
            ("cursor", self.p.user.join("hooks.json"), "user"),
        ];
        let imports_user = self.home.clone();
        let claude_hooks: [(&str, PathBuf, &str); 3] = [
            ("claude", self.p.repo.join(".claude/settings.json"), "imports_project"),
            ("claude", self.p.repo.join(".claude/settings.local.json"), "imports_project"),
            ("claude", imports_user.join(".claude/settings.json"), "imports_user"),
        ];
        for (kind, file, layer) in cursor_hooks.into_iter().chain(claude_hooks) {
            let is_local = kind == "claude" && file.ends_with("settings.local.json");
            if !is_local && self.fs().exists(&file) {
                let json = self.b.json(&file);
                let obj = json.object();
                let hooks = obj.get("hooks").and_then(Value::as_object).unwrap_or(&obj);
                let display = self.b.display(&file);
                if kind == "cursor" {
                    self.hooks_cursor(hooks, layer, display);
                } else {
                    hooks_of(self.b, hooks, layer, Some(display), "");
                }
            } else if is_local && self.fs().exists(&file) {
                let json = self.b.json(&file);
                let obj = json.object();
                if let Some(hooks) = obj.get("hooks").and_then(Value::as_object) {
                    let display = format!("{}/settings.local.json", self.b.display(&self.p.repo.join(".claude")));
                    hooks_of(self.b, hooks, layer, Some(display), "settings.local.json:");
                }
            }
        }
    }

    /// Cursor's hooks.json: the event maps to the hook definitions directly, each with a command and an
    /// optional matcher, and every source's hooks run (none shadows another).
    fn hooks_cursor(&mut self, block: &serde_json::Map<String, Value>, layer: &str, file: String) {
        for (event, defs) in block {
            let Some(defs) = defs.as_array() else { continue };
            for (n, d) in defs.iter().enumerate() {
                let attrs = HookAttrs {
                    event: neutral_event(event),
                    native_event: event.clone(),
                    matcher: d.get("matcher").and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from),
                    handler: HookHandler::Command,
                    script: d.get("command").and_then(Value::as_str).and_then(crate::read::redact::script_of),
                };
                let mut it = self.b.item(
                    layer,
                    &format!("{event}-{}", n + 1),
                    ItemDetail::Hook(attrs),
                    LoadWhen::Never,
                    calc(0),
                );
                it.origin.file = Some(file.clone());
                self.b.push(it);
            }
        }
    }

    /// The MCP servers in `.cursor/mcp.json` (project) and `~/.cursor/mcp.json` (user).
    fn mcp(&mut self) {
        for (file, layer) in [(self.p.project.join("mcp.json"), "project"), (self.p.user.join("mcp.json"), "user")] {
            if !self.fs().exists(&file) {
                continue;
            }
            let json = self.b.json(&file);
            let Some(servers) = json.object().get("mcpServers").and_then(Value::as_object).cloned() else {
                continue;
            };
            let display = self.b.display(&file);
            for (name, cfg) in &servers {
                self.declare_mcp(name, cfg, layer, &display);
            }
        }
    }

    /// One MCP server declaration: urls keep only their origin, commands only their file name; nothing of
    /// args, env or headers is ever read.
    fn declare_mcp(&mut self, name: &str, cfg: &Value, layer: &str, file: &str) {
        let url = cfg.get("url").and_then(Value::as_str);
        let (transport, target) = match url {
            Some(u) => (McpTransport::Http, url_origin(u)),
            None => (
                McpTransport::Stdio,
                cfg.get("command")
                    .and_then(Value::as_str)
                    .and_then(|c| Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned())),
            ),
        };
        let attrs = McpAttrs { transport, target, state: McpState::Active };
        let mut it = self.b.item(layer, name, ItemDetail::Mcp(attrs), LoadWhen::Startup, calc(0));
        it.origin.file = Some(file.to_string());
        self.b.push(it);
    }
}
