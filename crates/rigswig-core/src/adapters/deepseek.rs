//! The DeepSeek-CLI adapter (the community CLI, github.com/charsdavy/deepseek-cli). Every rule it models is in
//! `docs/adapters/deepseek-cli.md`, with the sources it comes from (all read 2026-11-06).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len, unknown};
use crate::model::*;
use crate::read::redact::{printable, sha256_hex, url_origin};

pub const ID: &str = "deepseek";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release the sources this adapter follows describe (v0.4 era; read 2026-11-06).
const TOOL_VERSION: &str = "0.4";
/// The instruction candidates at the repo root, in the loader's order: the first existing file wins and the
/// rest are not read.
const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "deepseek.md", ".deepseek", "CLAUDE.md", ".cursorrules"];
/// The skill sources' scan order (deepseek wins a name clash outright; later sources fill the gaps).
const SKILL_SCAN_ORDER: u32 = 0;

struct Paths {
    /// `~/.deepseek-cli` (the source pins CONFIG_DIR to the home; no relocation variable for the directory).
    user: PathBuf,
    project: PathBuf,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        Self { user: ctx.home.join(".deepseek-cli"), project: ctx.repo.join(".deepseek"), repo: ctx.repo.clone() }
    }
}

pub struct DeepSeekCli;

impl Adapter for DeepSeekCli {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &[
            "DEEPSEEK_MODEL",
            "DEEPSEEK_BASE_URL",
            "DEEPSEEK_SKILLS_DIR",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "CODEMAKER_HOME",
        ]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.project, ".deepseek/", 100),
            (p.repo.join("deepseek.md"), "deepseek.md", 60),
            (p.repo.join(".mcp.json"), ".mcp.json", 40),
            (p.user, "DeepSeek CLI folder", 45),
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
            "env",
            LayerKind::Session,
            "the launch environment (the DEEPSEEK_* variables the caller passed)",
            None,
            !ctx.env.is_empty(),
            LayerMode::Default,
        );
        b.add_layer(
            "user",
            LayerKind::User,
            "the user config directory (~/.deepseek-cli)",
            Some(b.display(&p.user)),
            has(&p.user.join("config.json"))
                || has(&p.user.join("mcp.json"))
                || has(&p.user.join("hooks.json"))
                || has(&p.user.join("skills"))
                || !b.dirs_in(&p.user.join("memories")).is_empty(),
            LayerMode::Default,
        );
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .deepseek directory and its root instruction file",
            Some(b.display(&p.project)),
            has(&p.project) || INSTRUCTION_FILES.iter().any(|f| has(&p.repo.join(f))) || has(&p.repo.join(".mcp.json")),
            LayerMode::Default,
        );
        b.add_layer(
            "imports_project",
            LayerKind::Project,
            "the repo's Claude, Codex and codemaker skill directories",
            None,
            has(&p.repo.join(".claude/skills"))
                || has(&p.repo.join(".codex/skills"))
                || has(&p.repo.join(".codemaker/skills")),
            LayerMode::Default,
        );
        b.add_layer(
            "imports_user",
            LayerKind::User,
            "the home's Claude, Codex and codemaker skill directories",
            None,
            has(&ctx.home.join(".claude/skills"))
                || has(&ctx.home.join(".codex/skills"))
                || has(&ctx.home.join(".codemaker/skills")),
            LayerMode::Default,
        );

        let user_cfg = b.json(&p.user.join("config.json")).object();
        let project_cfg = b.jsonc(&p.project.join("config.json")).object();
        let mut run = Run { b: &mut b, p, home: ctx.home.clone(), user_cfg, project_cfg };
        run.settings();
        run.instructions();
        run.memory();
        run.skills();
        run.hooks();
        run.mcp();
        run.b.link_packages();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the 14 built-in tools and the model".into(),
            why: "they come with the CLI, not from any file".into(),
        });
        run.b.not_measured_common();

        for note in [
            "which skills are active: the session's /skill picker turns a skill's content on and off mid-session; the catalog itself is what loads before you type",
            "environment variables beyond the model: the DEEPSEEK_API_KEY (never read), and the runtime approval mode's --yolo and --approval-mode flags",
            "the @file references a prompt mentions: their contents attach at invocation, not at startup",
            "hook scripts' stdout decisions (allow, rewrite) and their DEEPSEEK_HOOK runtime environment",
            "session persistence, compaction and the wrap-up summaries",
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
    project_cfg: Map<String, Value>,
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// The settings, resolved in the source's order: env over the project config over the global config.
    fn settings(&mut self) {
        for (keys, name, env_var) in [
            (vec!["defaultModel"], "model", Some("DEEPSEEK_MODEL")),
            (vec!["baseUrl"], "baseUrl", Some("DEEPSEEK_BASE_URL")),
            (vec!["approvalMode"], "approvalMode", None),
            (vec!["maxContext"], "maxContext", None),
        ] {
            let mut trail: Vec<(String, Value)> = Vec::new();
            if let Some(var) = env_var {
                if let Some(v) = self.b.ctx.var(var) {
                    trail.push(("env".to_string(), Value::String(printable(v))));
                }
            }
            for (layer, cfg) in [("project", &self.project_cfg), ("user", &self.user_cfg)] {
                if let Some(v) = cfg.get(keys[0]) {
                    if !(v.is_null() || v.as_str().is_some_and(|s| s.is_empty())) {
                        trail.push((layer.to_string(), v.clone()));
                    }
                }
            }
            if trail.is_empty() {
                continue;
            }
            let steps = trail
                .iter()
                .map(|(layer, v)| TrailStep {
                    layer: layer.clone(),
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

    /// The one project instruction file the loader reads: it walks its list at the repo root and the first
    /// existing file wins — the rest are never read. A file that exists but reads empty behaves like a miss
    /// (**inferred**: the loader would inject nothing, so nothing is measured).
    fn instructions(&mut self) {
        for name in INSTRUCTION_FILES {
            let f = self.p.repo.join(name);
            let Some(text) = self.b.text(&f).ok().filter(|t| !t.trim().is_empty()) else { continue };
            let name = self.b.display(&f);
            let mut it = self.b.item(
                "project",
                &name,
                ItemDetail::Instruction(InstructionAttrs::default()),
                LoadWhen::Startup,
                calc(char_len(text.trim())),
            );
            it.origin.file = Some(name.clone());
            it.load.content_hash = Some(sha256_hex(&text));
            self.b.push(it);
            return;
        }
    }

    /// The per-project long-term memory, generated and loaded at session start into the project context
    /// (`~/.deepseek-cli/memories/<sha256(cwd) first 12>/MEMORY.md`).
    fn memory(&mut self) {
        let hash = &sha256_hex(self.p.repo.to_string_lossy().as_ref())[..12];
        let f = self.p.user.join("memories").join(hash).join("MEMORY.md");
        let Ok(text) = self.b.text(&f) else { return };
        if text.trim().is_empty() {
            return;
        }
        let mut it = self.b.item(
            "user",
            &format!("memories/{hash}/MEMORY.md"),
            ItemDetail::Instruction(InstructionAttrs::default()),
            LoadWhen::Startup,
            calc(char_len(text.trim())),
        );
        it.origin.file = Some(format!("~/.deepseek-cli/memories/{hash}/MEMORY.md"));
        it.load.content_hash = Some(sha256_hex(&text));
        self.b.push(it);
    }

    /// Skills from every source, in the scan order: flat `<name>.md` and `<name>/SKILL.md` at each root,
    /// deduplicated by name with deepseek's roots first.
    fn skills(&mut self) {
        let claude_home = self.b.ctx.var("CLAUDE_CONFIG_DIR").map_or_else(|| self.home.join(".claude"), PathBuf::from);
        let codex_home = self.b.ctx.var("CODEX_HOME").map_or_else(|| self.home.join(".codex"), PathBuf::from);
        let codemaker_home =
            self.b.ctx.var("CODEMAKER_HOME").map_or_else(|| self.home.join(".codemaker"), PathBuf::from);
        let deepseek_skills =
            self.b.ctx.var("DEEPSEEK_SKILLS_DIR").map_or_else(|| self.p.user.join("skills"), PathBuf::from);
        let roots: [(&str, PathBuf, u32); 8] = [
            ("user", deepseek_skills, 0),
            ("project", self.p.project.join("skills"), 1),
            ("imports_user", claude_home.join("skills"), 2),
            ("imports_project", self.p.repo.join(".claude/skills"), 3),
            ("imports_user", codex_home.join("skills"), 4),
            ("imports_project", self.p.repo.join(".codex/skills"), 5),
            ("imports_user", codemaker_home.join("skills"), 6),
            ("imports_project", self.p.repo.join(".codemaker/skills"), 7),
        ];
        let mut idx = Vec::new();
        // Same-name priority, keyed by each item's origin file: the scan order wins.
        let priorities: std::collections::BTreeMap<String, u32> = roots
            .iter()
            .flat_map(|(layer, dir, order)| {
                let mut out = Vec::new();
                for f in self.b.files_in(dir, ".md") {
                    out.push((self.b.display(&f), *order));
                }
                for d in self.b.dirs_in(dir) {
                    out.push((self.b.display(&d.join("SKILL.md")), *order));
                }
                let _ = layer;
                out
            })
            .collect();
        for (layer, dir, _order) in roots {
            for f in self.b.files_in(&dir, ".md") {
                if let Some(ix) = self.add_skill(&f, layer) {
                    idx.push(ix);
                }
            }
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
            |i| i.origin.file.as_deref().and_then(|f| priorities.get(f)).copied().unwrap_or(SKILL_SCAN_ORDER + 9),
        );
    }

    /// One skill: folded into the system prompt when active, so the listing (name + the extracted
    /// description) is measured at startup and the body on invoke.
    fn add_skill(&mut self, f: &Path, layer: &str) -> Option<usize> {
        let Ok(text) = self.b.text(f) else { return None };
        let body = text.trim();
        if body.is_empty() {
            return None;
        }
        let name = if f.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            f.parent().and_then(Path::file_name)?.to_string_lossy().into_owned()
        } else {
            f.file_stem()?.to_string_lossy().into_owned()
        };
        // The description is a best-effort first heading or paragraph; the loader caps it at 80 characters.
        let description = extracted_description(&text);
        let listing = char_len(&name) + char_len(&description);
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

    /// Hooks: two flat-array JSON files, the project's first (its hooks execute first), then the global one.
    fn hooks(&mut self) {
        for (file, layer) in [(self.p.project.join("hooks.json"), "project"), (self.p.user.join("hooks.json"), "user")]
        {
            if !self.fs().exists(&file) {
                continue;
            }
            // The file holds a bare array of `{ event, command, matcher? }` definitions.
            let json = self.b.json(&file);
            let Some(Value::Array(defs)) = json.value() else { continue };
            let defs = defs.clone();
            hooks_flat(self.b, &defs, layer, self.b.display(&file));
        }
    }

    /// The MCP servers in `~/.deepseek-cli/mcp.json` (global) and the repo's `.mcp.json` (project).
    fn mcp(&mut self) {
        for (file, layer) in [(self.p.user.join("mcp.json"), "user"), (self.p.repo.join(".mcp.json"), "project")] {
            if !self.fs().exists(&file) {
                continue;
            }
            let json = self.b.json(&file);
            let Some(servers) = json.object().get("mcpServers").and_then(Value::as_object).cloned() else {
                continue;
            };
            let display = self.b.display(&file);
            let mut idx = Vec::new();
            for (name, cfg) in &servers {
                idx.push(self.declare_mcp(name, cfg, layer, &display));
            }
            self.b.resolve_clashes(&idx, |i| i.name.clone(), |i| u32::from(i.origin.layer == "user"));
        }
    }

    /// One MCP server: a command keeps its file name, a url its origin; env and args never surface.
    fn declare_mcp(&mut self, name: &str, cfg: &Value, layer: &str, file: &str) -> usize {
        let (transport, target) = if let Some(u) = cfg.get("url").and_then(Value::as_str) {
            (McpTransport::Http, url_origin(u))
        } else if let Some(c) = cfg.get("command").and_then(Value::as_str) {
            (McpTransport::Stdio, Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned()))
        } else {
            (McpTransport::Unknown, None)
        };
        let attrs = McpAttrs { transport, target, state: McpState::Active };
        let mut it = self.b.item(layer, name, ItemDetail::Mcp(attrs), LoadWhen::Startup, unknown());
        it.origin.file = Some(file.to_string());
        self.b.push(it)
    }
}

/// The flat hook array: one item per hook definition with its event and matcher.
fn hooks_flat(b: &mut Build, defs: &[Value], layer: &str, file: String) {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for d in defs {
        let Some(event) = d.get("event").and_then(Value::as_str) else { continue };
        let n = *counts.entry(event.to_string()).and_modify(|c| *c += 1).or_insert(0);
        let attrs = HookAttrs {
            event: crate::adapters::shared::neutral_event(event),
            native_event: event.to_string(),
            matcher: d.get("matcher").and_then(Value::as_str).filter(|m| !m.is_empty()).map(String::from),
            handler: HookHandler::Command,
            script: d.get("command").and_then(Value::as_str).and_then(crate::read::redact::script_of),
        };
        let mut it = b.item(layer, &format!("{event}-{}", n + 1), ItemDetail::Hook(attrs), LoadWhen::Never, calc(0));
        it.origin.file = Some(file.clone());
        b.push(it);
    }
}

/// The skill listing's description: the first heading or paragraph after the frontmatter, capped at 80 chars.
fn extracted_description(text: &str) -> String {
    let mut lines = text.split('\n');
    let mut first = lines.next().unwrap_or("").trim().to_string();
    if first == "---" {
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
        first = String::new();
    }
    let _ = &first;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("<!--") || t == "---" {
            continue;
        }
        if let Some(h) = t.strip_prefix('#') {
            let h = h.trim_start();
            return short(h);
        }
        return short(t);
    }
    String::new()
}

fn short(s: &str) -> String {
    let chars: Vec<char> = printable(s).chars().collect();
    if chars.len() <= 80 {
        chars.into_iter().collect()
    } else {
        let cut: String = chars[..79].iter().collect();
        format!("{cut}…")
    }
}
