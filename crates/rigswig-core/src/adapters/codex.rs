//! The OpenAI Codex CLI adapter. Every rule it models is in `docs/adapters/codex.md`, with the docs it comes from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len, unknown};
use crate::adapters::shared::hooks_of;
use crate::fs::is_within;
use crate::model::*;
use crate::read::frontmatter;
use crate::read::json::{obj_at, str_at};
use crate::read::redact::{printable, safe_base, sha256_hex, url_origin};

pub const ID: &str = "codex";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release whose docs and source this adapter follows.
const TOOL_VERSION: &str = "0.166.0";
/// `project_doc_max_bytes` when nothing sets it.
const DEFAULT_DOC_BYTES: u64 = 32 * 1024;
/// The skill listing's budget when the context window is unknown.
const SKILL_LISTING_BUDGET: u64 = 8000;
/// Keys a project `.codex/config.toml` may not set; Codex ignores them there.
const PROJECT_IGNORED: &[&str] = &[
    "openai_base_url",
    "chatgpt_base_url",
    "apps_mcp_product_sku",
    "model_provider",
    "model_providers",
    "notify",
    "profile",
    "profiles",
    "experimental_realtime_ws_base_url",
    "otel",
];

pub struct Codex;

/// An MCP server as declared: name, layer, its table, and the file shown for it.
type ServerDecl = (String, String, Map<String, Value>, Option<String>);

struct Paths {
    home_dir: PathBuf,
    codex_home: PathBuf,
    /// `/etc/codex` on Unix; `None` on Windows, which has no system config.toml.
    system: Option<PathBuf>,
    requirements: PathBuf,
    managed_config: PathBuf,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let codex_home = match ctx.var("CODEX_HOME") {
            Some(d) => ctx.home.join(Path::new(d).strip_prefix("~/").unwrap_or(Path::new(d))),
            None => ctx.home.join(".codex"),
        };
        let (system, requirements, managed_config) = match ctx.os {
            Os::Windows => (
                None,
                PathBuf::from(r"C:\ProgramData\OpenAI\Codex\requirements.toml"),
                codex_home.join("managed_config.toml"),
            ),
            _ => (
                Some(PathBuf::from("/etc/codex")),
                PathBuf::from("/etc/codex/requirements.toml"),
                PathBuf::from("/etc/codex/managed_config.toml"),
            ),
        };
        Self { home_dir: ctx.home.clone(), codex_home, system, requirements, managed_config, repo: ctx.repo.clone() }
    }
}

/// Objects merge key by key; anything else (arrays included) is replaced by the higher layer.
fn overlay(into: &mut Value, from: &Value) {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(k) {
                    Some(slot) => overlay(slot, v),
                    None => {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

/// The longest prefix of `s` that fits in `max` bytes without splitting a character.
fn prefix_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trust {
    Trusted,
    Untrusted,
    Unknown,
}

/// One config layer's file contents, highest precedence first: managed, project, user, system.
struct Layered {
    id: &'static str,
    obj: Map<String, Value>,
}

struct Run<'a, 'b> {
    b: &'b mut Build<'a>,
    p: Paths,
    /// Project root (nearest folder with a root marker) down to the repo.
    chain: Vec<PathBuf>,
    trust: Trust,
    layers: Vec<Layered>,
    /// Every config layer merged, lowest first.
    merged: Map<String, Value>,
    requirements: Map<String, Value>,
    /// Enabled plugins: their layer id and folder.
    plugin_dirs: Vec<(String, PathBuf)>,
}

impl Adapter for Codex {
    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["CODEX_HOME"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.repo.join(".codex"), ".codex/", 100),
            (p.repo.join(".agents/skills"), ".agents/skills/", 60),
            (p.repo.join("AGENTS.md"), "AGENTS.md", 40),
            (p.codex_home.clone(), "Codex home folder", 50),
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
        let mut run = Run {
            b: &mut b,
            p,
            chain: Vec::new(),
            trust: Trust::Unknown,
            layers: Vec::new(),
            merged: Map::new(),
            requirements: Map::new(),
            plugin_dirs: Vec::new(),
        };
        run.config();
        run.settings();
        run.instructions();
        run.skills_prompts_agents();
        run.hooks();
        run.mcp();
        run.b.link_packages();

        b.not_measured.push(NotMeasured {
            what: "the system prompt, built-in tools and bundled system skills".into(),
            why: "they come with Codex, not from any file".into(),
        });
        b.not_measured_common();
        for note in [
            "command-line flags and overrides (-c, --model, --profile) and profile files (~/.codex/<name>.config.toml)",
            "cloud-delivered config and requirements, and macOS MDM preferences (com.openai.codex)",
            "hook trust review: Codex runs a non-managed hook only after its hash is reviewed",
            "built-in agents (default, worker, explorer) and the [agents] settings",
            "plugin manifest paths, apps (.app.json) and marketplaces other than the install cache",
            "the MCP allowlist's identity matching in requirements.toml (names only are compared)",
            "whether repo skills need a trusted project (assumed not)",
            "whether a project agent beats a user agent of the same name (assumed yes)",
            "whether an installed plugin with no setting is enabled (assumed yes)",
        ] {
            b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// Layers, the project root and trust, every config file, merged.
    fn config(&mut self) {
        let req = self.b.toml(&self.p.requirements.clone());
        let managed = self.b.toml(&self.p.managed_config.clone());
        let user_file = self.p.codex_home.join("config.toml");
        let user = self.b.toml(&user_file);
        let system_file = self.p.system.as_ref().map(|s| s.join("config.toml"));
        let system = system_file.as_ref().map(|f| self.b.toml(f));

        // The project root: the nearest folder with a root marker (from non-project layers), else the repo alone.
        let mut base = Value::Object(Map::new());
        for l in
            [system.as_ref().map(|s| s.object()), Some(user.object()), Some(managed.object())].into_iter().flatten()
        {
            overlay(&mut base, &Value::Object(l));
        }
        let markers: Vec<String> = match base.get("project_root_markers").and_then(Value::as_array) {
            Some(a) => a.iter().filter_map(Value::as_str).map(String::from).collect(),
            None => vec![".git".into()],
        };
        let root = self
            .p
            .repo
            .ancestors()
            .find(|d| markers.iter().any(|m| self.fs().exists(&d.join(m))))
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.p.repo.clone());
        let mut chain: Vec<PathBuf> =
            self.p.repo.ancestors().take_while(|d| d.starts_with(&root)).map(Path::to_path_buf).collect();
        chain.reverse();
        self.chain = chain;

        // Trust: [projects."<path>"].trust_level in the user's config, for the repo or its project root.
        let projects = obj_at(&user.object(), "projects").cloned().unwrap_or_default();
        let mut keys = vec![self.p.repo.clone(), root.clone()];
        if let Ok(real) = self.fs().canonicalize(&self.p.repo) {
            keys.push(real);
        }
        self.trust = keys
            .iter()
            .find_map(|k| obj_at(&projects, &k.to_string_lossy()).and_then(|e| str_at(e, "trust_level")))
            .map_or(Trust::Unknown, |t| if t == "trusted" { Trust::Trusted } else { Trust::Untrusted });

        // Project config: every .codex/config.toml from the root down; the closest wins. Trusted projects only.
        let mut project = Value::Object(Map::new());
        let mut project_present = false;
        for d in self.chain.clone() {
            let f = d.join(".codex/config.toml");
            let j = self.b.toml(&f);
            project_present |= j.present();
            if self.trust == Trust::Trusted {
                overlay(&mut project, &Value::Object(j.object()));
            }
        }
        let mut project = project.as_object().cloned().unwrap_or_default();
        for k in PROJECT_IGNORED {
            if project.remove(*k).is_some() {
                self.b.warn(
                    "project-key-ignored",
                    format!("{k} is set in project config, where Codex ignores it"),
                    None,
                );
            }
        }
        if project_present && self.trust != Trust::Trusted {
            self.b.warn(
                "project-untrusted",
                "this project is not marked trusted, so Codex skips its .codex/ config, hooks and agents",
                None,
            );
        }

        let sys_label = system_file.as_ref().map(|f| self.b.display(f));
        let req_label = self.b.display(&self.p.requirements.clone());
        let man_label = self.b.display(&self.p.managed_config.clone());
        let user_label = self.b.display(&user_file);
        self.b.add_layer(
            "requirements",
            LayerKind::Managed,
            "requirements (enforced)",
            Some(req_label),
            req.present(),
            LayerMode::Enforce,
        );
        self.b.add_layer(
            "managed",
            LayerKind::Managed,
            "managed defaults",
            Some(man_label),
            managed.present(),
            LayerMode::Default,
        );
        self.b.add_layer("session", LayerKind::Session, "command line (not read)", None, false, LayerMode::Default);
        let trusted = self.trust == Trust::Trusted;
        self.b.add_layer(
            "project",
            LayerKind::Project,
            "project config (trusted projects only)",
            Some(".codex/config.toml".into()),
            project_present && trusted,
            LayerMode::Default,
        );
        self.b.add_layer("user", LayerKind::User, "user config", Some(user_label), user.present(), LayerMode::Default);
        self.b.add_layer(
            "system",
            LayerKind::Managed,
            "system defaults",
            sys_label,
            system.as_ref().is_some_and(|s| s.present()),
            LayerMode::Default,
        );

        self.requirements = req.object();
        self.layers = vec![
            Layered { id: "managed", obj: managed.object() },
            Layered { id: "project", obj: project },
            Layered { id: "user", obj: user.object() },
            Layered { id: "system", obj: system.map(|s| s.object()).unwrap_or_default() },
        ];
        let mut merged = Value::Object(Map::new());
        for l in self.layers.iter().rev() {
            overlay(&mut merged, &Value::Object(l.obj.clone()));
        }
        self.merged = merged.as_object().cloned().unwrap_or_default();
        self.b.settings.insert(
            "projectTrust".into(),
            Decision {
                value: match self.trust {
                    Trust::Trusted => "trusted",
                    Trust::Untrusted => "untrusted",
                    Trust::Unknown => "unknown",
                }
                .into(),
                source: (self.trust != Trust::Unknown).then(|| "user".into()),
                trail: if self.trust == Trust::Unknown {
                    Vec::new()
                } else {
                    vec![TrailStep {
                        layer: "user".into(),
                        value: Value::Null,
                        note: Some("projects.<path>.trust_level".into()),
                    }]
                },
            },
        );
    }

    fn settings(&mut self) {
        for key in [
            "model",
            "model_provider",
            "model_reasoning_effort",
            "approval_policy",
            "sandbox_mode",
            "project_doc_max_bytes",
        ] {
            let mut trail: Vec<TrailStep> = self
                .layers
                .iter()
                .filter_map(|l| l.obj.get(key).map(|v| TrailStep { layer: l.id.into(), value: v.clone(), note: None }))
                .collect();
            let Some(first) = trail.first().cloned() else { continue };
            let allowed_key = match key {
                "approval_policy" => Some("allowed_approval_policies"),
                "sandbox_mode" => Some("allowed_sandbox_modes"),
                _ => None,
            };
            if let Some(allowed) = allowed_key.and_then(|k| self.requirements.get(k)).and_then(Value::as_array) {
                if !allowed.contains(&first.value) {
                    trail.push(TrailStep {
                        layer: "requirements".into(),
                        value: Value::Array(allowed.clone()),
                        note: Some("not allowed by requirements: Codex falls back to an allowed value".into()),
                    });
                    self.b.warn(
                        "not-allowed",
                        format!("{key} is set to a value requirements.toml does not allow"),
                        None,
                    );
                }
            }
            let name = match key {
                "model_provider" => "provider",
                other => other,
            };
            self.b.settings.insert(name.into(), Decision { value: first.value, source: Some(first.layer), trail });
        }
    }

    fn add_doc(&mut self, file: &Path, layer: &str, text: &str, counted: &str) -> usize {
        let name = self.b.display(file);
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Instruction(InstructionAttrs::default()),
            LoadWhen::Startup,
            calc(char_len(counted)),
        );
        it.origin.file = Some(name);
        it.load.content_hash = Some(sha256_hex(text));
        self.b.push(it)
    }

    /// AGENTS.md: the global file (outside the budget), then one file per folder from the project root down, sharing
    /// `project_doc_max_bytes`; the file that crosses it is cut, later ones are dropped.
    fn instructions(&mut self) {
        for name in ["AGENTS.override.md", "AGENTS.md"] {
            let f = self.p.codex_home.join(name);
            if let Ok(text) = self.fs().read_to_string(&f) {
                if !text.trim().is_empty() {
                    let t = text.trim().to_string();
                    self.add_doc(&f, "user", &text, &t);
                    break;
                }
            }
        }
        if self.trust == Trust::Untrusted {
            self.b.warn(
                "project-untrusted-docs",
                "this project is marked untrusted, so Codex reads none of its AGENTS.md files",
                None,
            );
            return;
        }
        let fallbacks: Vec<String> = self
            .merged
            .get("project_doc_fallback_filenames")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
            .unwrap_or_default();
        let budget = self.merged.get("project_doc_max_bytes").and_then(Value::as_u64).unwrap_or(DEFAULT_DOC_BYTES);
        let mut left = budget as usize;
        for d in self.chain.clone() {
            let mut names = vec!["AGENTS.override.md".to_string(), "AGENTS.md".to_string()];
            names.extend(fallbacks.iter().cloned());
            // The first candidate that exists as a file is the folder's pick, even if it is empty.
            let Some((f, text)) =
                names.iter().map(|n| d.join(n)).find_map(|f| self.fs().read_to_string(&f).ok().map(|t| (f, t)))
            else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            if left == 0 {
                let i = self.add_doc(&f, "project", &text, &text);
                Build::disable(&mut self.b.items[i]);
                self.b.warn(
                    "agents-md-budget",
                    format!("dropped: the AGENTS.md files already fill project_doc_max_bytes ({budget} bytes)"),
                    Some(&f),
                );
                continue;
            }
            let kept = prefix_bytes(&text, left).to_string();
            if kept.len() < text.len() {
                self.b.warn("agents-md-budget", format!("cut to fit project_doc_max_bytes ({budget} bytes)"), Some(&f));
            }
            left -= kept.len();
            self.add_doc(&f, "project", &text, &kept);
        }
    }

    /// Push an item whose same-named siblings must all stay (skills): ids get `#2`, `#3`... without shadowing.
    fn push_distinct(&mut self, mut it: Item) -> usize {
        let base = it.id.clone();
        let mut n = 2;
        while self.b.items.iter().any(|i| i.id == it.id) {
            it.id = format!("{base}#{n}");
            n += 1;
        }
        self.b.items.push(it);
        self.b.items.len() - 1
    }

    fn skill(&mut self, dir: &Path, layer: &str, ns: Option<&str>) -> Option<usize> {
        let file = dir.join("SKILL.md");
        let text = self.fs().read_to_string(&file).ok()?;
        let fm = frontmatter::parse(&text);
        let base = fm
            .str("name")
            .map(String::from)
            .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned());
        let name = ns.map_or(base.clone(), |ns| format!("{ns}:{base}"));
        let chars = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
        let body = char_len(&text[fm.body_start..]);
        let mut it = self.b.item(
            layer,
            &name,
            ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }),
            LoadWhen::Startup,
            calc(chars),
        );
        it.origin.file = Some(self.b.display(&file));
        it.load.content_hash = Some(sha256_hex(&text));
        Some(self.push_distinct(it))
    }

    fn skills_in(&mut self, root: &Path, layer: &str, ns: Option<&str>, inside: Option<&Path>) -> Vec<usize> {
        let mut out = Vec::new();
        for d in self.b.dirs_in(root) {
            if let Some(canon_root) = inside {
                match self.fs().canonicalize(&d.join("SKILL.md")) {
                    Ok(real) if is_within(&real, canon_root) => {}
                    Ok(_) => {
                        self.b.warn("plugin-path-escape", "a plugin skill leaves the plugin folder, ignored", None);
                        continue;
                    }
                    Err(_) => continue,
                }
            }
            out.extend(self.skill(&d, layer, ns));
        }
        out
    }

    fn skills_prompts_agents(&mut self) {
        // Skills. Same-named skills are all listed; Codex does not merge them.
        let mut skills = Vec::new();
        let root = self.chain.first().cloned().unwrap_or_else(|| self.p.repo.clone());
        for d in self.chain.clone().iter().rev() {
            skills.extend(self.skills_in(&d.join(".agents/skills"), "project", None, None));
        }
        skills.extend(self.skills_in(&root.join(".codex/skills"), "project", None, None));
        skills.extend(self.skills_in(&self.p.home_dir.join(".agents/skills"), "user", None, None));
        let legacy = self.skills_in(&self.p.codex_home.join("skills"), "user", None, None);
        if !legacy.is_empty() {
            self.b.warn(
                "deprecated-location",
                "skills in the Codex home folder's skills/ use a deprecated location (~/.agents/skills replaces it)",
                None,
            );
        }
        skills.extend(legacy);
        if let Some(sys) = self.p.system.clone() {
            skills.extend(self.skills_in(&sys.join("skills"), "system", None, None));
        }
        skills.extend(self.plugins());

        // [[skills.config]]: by name or path, the last match wins (lowest layer first).
        let mut rules: Vec<Map<String, Value>> = Vec::new();
        for l in self.layers.iter().rev() {
            if let Some(a) = l.obj.get("skills").and_then(|s| s.get("config")).and_then(Value::as_array) {
                rules.extend(a.iter().filter_map(|r| r.as_object().cloned()));
            }
        }
        for &i in &skills {
            let it = &self.b.items[i];
            let file = it.origin.file.clone().unwrap_or_default();
            let mut on = true;
            for r in &rules {
                let by_name = str_at(r, "name").is_some_and(|n| n == it.name);
                let by_path = str_at(r, "path").is_some_and(|p| {
                    let p = p.trim_end_matches('/');
                    let abs = self.p.home_dir.join(p.strip_prefix("~/").unwrap_or(p));
                    file == self.b.display(&abs) || file.trim_end_matches("/SKILL.md") == self.b.display(&abs)
                });
                if by_name || by_path {
                    on = r.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                }
            }
            if !on {
                Build::disable(&mut self.b.items[i]);
            }
        }
        let listed: u64 =
            skills.iter().filter(|&&i| self.b.items[i].effective).map(|&i| self.b.items[i].load.chars.value).sum();
        if listed > SKILL_LISTING_BUDGET {
            self.b.warn(
                "skill-listing-budget",
                format!("the skill listing is {listed} characters; Codex keeps it within 2% of the context window ({SKILL_LISTING_BUDGET} characters when the window is unknown), so some descriptions are cut"),
                None,
            );
        }

        // Custom prompts (deprecated): top-level Markdown in the Codex home's prompts/, invoked explicitly.
        for f in self.b.files_in(&self.p.codex_home.join("prompts"), ".md") {
            let Ok(text) = self.fs().read_to_string(&f) else { continue };
            let fm = frontmatter::parse(&text);
            let stem = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let name = format!("prompts:{stem}");
            let chars = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
            let mut it =
                self.b.item("user", &name, ItemDetail::Command(CommandAttrs {}), LoadWhen::OnInvoke, calc(chars));
            it.origin.file = Some(self.b.display(&f));
            it.load.content_hash = Some(sha256_hex(&text));
            self.b.push(it);
        }

        // Custom agents: one TOML file each; `name` is the identity. Project agents need a trusted project.
        let mut agents = Vec::new();
        let mut roots = vec![(self.p.codex_home.join("agents"), "user")];
        if self.trust == Trust::Trusted {
            roots.extend(self.chain.iter().map(|d| (d.join(".codex/agents"), "project")));
        }
        for (dir, layer) in roots {
            for f in self.b.files_in(&dir, ".toml") {
                let Some(t) = self.b.toml(&f).value().and_then(Value::as_object).cloned() else { continue };
                let name = str_at(&t, "name")
                    .map(String::from)
                    .unwrap_or_else(|| f.file_stem().unwrap_or_default().to_string_lossy().into_owned());
                let chars = char_len(&name) + char_len(str_at(&t, "description").unwrap_or(""));
                let attrs = AgentAttrs { tools: ToolAccess::All, model: str_at(&t, "model").map(String::from) };
                let mut it = self.b.item(layer, &name, ItemDetail::Agent(attrs), LoadWhen::Startup, calc(chars));
                it.origin.file = Some(self.b.display(&f));
                if let Ok(text) = self.fs().read_to_string(&f) {
                    it.load.content_hash = Some(sha256_hex(&text));
                }
                agents.push(self.b.push(it));
            }
        }
        self.b.resolve_clashes(&agents, |i| i.name.clone(), |i| if i.origin.layer == "project" { 0 } else { 1 });
    }

    /// Installed plugins (the install cache): their package and layer, and their skills. Returns the skills.
    fn plugins(&mut self) -> Vec<usize> {
        let features_off = self.merged.get("features").and_then(|f| f.get("plugins")) == Some(&Value::Bool(false));
        let cache = self.p.codex_home.join("plugins/cache");
        let default_on = self
            .merged
            .get("plugins")
            .and_then(|p| p.get("_default"))
            .and_then(|d| d.get("enabled"))
            .and_then(Value::as_bool);
        let mut skills = Vec::new();
        for market in self.b.dirs_in(&cache) {
            for plugin in self.b.dirs_in(&market) {
                let Some(version) = self.b.dirs_in(&plugin).into_iter().last() else { continue };
                let m = market.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let n = plugin.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let id = format!("{n}@{m}");
                let mut scopes = BTreeMap::new();
                let mut decided = None;
                for l in &self.layers {
                    if let Some(v) = l
                        .obj
                        .get("plugins")
                        .and_then(|p| p.get(&id))
                        .and_then(|e| e.get("enabled"))
                        .and_then(Value::as_bool)
                    {
                        scopes.insert(l.id.to_string(), v);
                        decided.get_or_insert((l.id.to_string(), v));
                    }
                }
                let enabled = !features_off && decided.as_ref().map_or(default_on.unwrap_or(true), |(_, v)| *v);
                self.b.packages.push(Package {
                    id: printable(&id),
                    kind: PackageKind::Plugin,
                    source: printable(&m),
                    enabled,
                    decided_by: decided.map(|(l, _)| l),
                    scopes,
                    contributes: Vec::new(),
                });
                if !enabled {
                    continue;
                }
                let layer = format!("plugin:{}", printable(&id));
                let shown = self.b.display(&version);
                self.b.add_layer(
                    &layer,
                    LayerKind::Package,
                    &format!("plugin {id}"),
                    Some(shown),
                    true,
                    LayerMode::Default,
                );
                let canon = self.fs().canonicalize(&version).unwrap_or_else(|_| version.clone());
                skills.extend(self.skills_in(&version.join("skills"), &layer, Some(&n), Some(&canon)));
                self.plugin_dirs.push((layer, version));
            }
        }
        if features_off {
            self.b.warn("plugins-off", "[features] plugins = false turns every plugin off", None);
        }
        skills
    }

    fn hooks(&mut self) {
        let mut all = Vec::new();
        let user_hooks = self.p.codex_home.join("hooks.json");
        let mut sources: Vec<(PathBuf, &str, String)> = vec![(user_hooks, "user", String::new())];
        if self.trust == Trust::Trusted {
            for d in self.chain.clone() {
                // Folders above the repo prefix their hooks' names, so each stays unique.
                let prefix = if d == self.p.repo { String::new() } else { format!("{}:", self.b.display(&d)) };
                sources.push((d.join(".codex/hooks.json"), "project", prefix));
            }
        }
        for (f, layer, prefix) in sources {
            let j = self.b.json(&f).object();
            if let Some(block) = obj_at(&j, "hooks").cloned() {
                let shown = Some(self.b.display(&f));
                all.extend(hooks_of(self.b, &block, layer, shown, &prefix));
            }
        }
        // Inline [[hooks.<Event>]] tables in each config layer: every source loads, none replaces another.
        for l in &self.layers {
            if let Some(block) = obj_at(&l.obj, "hooks").cloned() {
                let file = self.b.layers.iter().find(|x| x.id == l.id).and_then(|x| x.path.clone());
                all.extend(hooks_of(self.b, &block, l.id, file, "config.toml:"));
            }
        }
        for (layer, dir) in self.plugin_dirs.clone() {
            let f = dir.join("hooks/hooks.json");
            let j = self.b.json(&f).object();
            if let Some(block) = obj_at(&j, "hooks").cloned() {
                let s = Some(self.b.display(&f));
                all.extend(hooks_of(self.b, &block, &layer, s, ""));
            }
        }
        let off = self.merged.get("features").and_then(|f| f.get("hooks").or_else(|| f.get("codex_hooks")))
            == Some(&Value::Bool(false));
        if off {
            for i in all {
                Build::disable(&mut self.b.items[i]);
            }
            self.b.warn("hooks-off", "[features] hooks = false turns every hook off", None);
        }
    }

    fn mcp(&mut self) {
        let allow: Option<Vec<String>> = obj_at(&self.requirements, "mcp_servers").map(|m| m.keys().cloned().collect());
        let mut idx = Vec::new();
        let mut decls: Vec<ServerDecl> = Vec::new();
        for l in &self.layers {
            if let Some(m) = obj_at(&l.obj, "mcp_servers") {
                let file = self.b.layers.iter().find(|x| x.id == l.id).and_then(|x| x.path.clone());
                for (name, s) in m {
                    if let Some(s) = s.as_object() {
                        decls.push((name.clone(), l.id.to_string(), s.clone(), file.clone()));
                    }
                }
            }
        }
        // Plugin servers: the plugin's .mcp.json, switched by [plugins."<id>".mcp_servers.<name>].
        for (layer, dir) in self.plugin_dirs.clone() {
            let f = dir.join(".mcp.json");
            let j = self.b.json(&f).object();
            let map = obj_at(&j, "mcpServers").cloned().unwrap_or(j);
            let pid = layer.trim_start_matches("plugin:").to_string();
            for (name, s) in map {
                let Some(mut s) = s.as_object().cloned() else { continue };
                if let Some(en) = self
                    .merged
                    .get("plugins")
                    .and_then(|p| p.get(&pid))
                    .and_then(|p| p.get("mcp_servers"))
                    .and_then(|m| m.get(&name))
                    .and_then(|m| m.get("enabled"))
                {
                    s.insert("enabled".into(), en.clone());
                }
                decls.push((name, layer.clone(), s, Some(self.b.display(&f))));
            }
        }
        for (name, layer, s, file) in decls {
            let cmd = str_at(&s, "command");
            let url = str_at(&s, "url");
            let (transport, target) = match (cmd, url) {
                (Some(c), _) => (McpTransport::Stdio, c.split_whitespace().next().map(safe_base)),
                (None, Some(u)) => (McpTransport::Http, Some(url_origin(u).unwrap_or_else(|| "(unparsed url)".into()))),
                _ => (McpTransport::Unknown, None),
            };
            let state = if allow.as_ref().is_some_and(|a| !a.contains(&name)) {
                McpState::Denied
            } else if s.get("enabled") == Some(&Value::Bool(false)) {
                McpState::Disabled
            } else {
                McpState::Active
            };
            let mut it = self.b.item(
                &layer,
                &name,
                ItemDetail::Mcp(McpAttrs { transport, target, state }),
                LoadWhen::Startup,
                unknown(),
            );
            it.origin.file = file;
            if state != McpState::Active {
                Build::disable(&mut it);
            }
            idx.push(self.b.push(it));
        }
        let order = |l: &str| match l {
            "managed" => 0,
            "project" => 1,
            "user" => 2,
            "system" => 3,
            _ => 4,
        };
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |i| order(&i.origin.layer));
        self.b.note_not_modelled(
            "an MCP server set in two config layers is taken whole from the higher one (Codex may merge its fields)",
        );
    }
}
