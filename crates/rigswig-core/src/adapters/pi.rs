//! The Pi adapter (the `pi` coding agent, github.com/earendil-works/pi). Every rule it models is in
//! `docs/adapters/pi.md`, with the source files it comes from (all read 2026-11-07, at release 1.5.0).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::fs::EntryKind;
use crate::model::*;
use crate::read::frontmatter;
use crate::read::redact::{printable, sha256_hex, url_origin};

pub const ID: &str = "pi";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release whose source this adapter follows (`packages/coding-agent` 1.5.0; read 2026-11-07).
const TOOL_VERSION: &str = "1.5.0";
/// The context-file candidates, in the order `loadContextFileFromDir` tries them; the first that exists wins.
const CONTEXT_CANDIDATES: &[&str] = &["AGENTS.override.md", "AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];
/// The `.pi/` entries whose presence makes Pi ask for project trust (`trust-manager.ts`).
const TRUST_GATED: &[&str] =
    &["settings.json", "mcp.json", "extensions", "skills", "prompts", "themes", "SYSTEM.md", "APPEND_SYSTEM.md"];
/// How deep the recursive skill walk goes before it stops (Pi has no limit; this guards link loops).
const MAX_DEPTH: usize = 8;

/// Where a skill folder's loose Markdown files count as skills (`collectSkillEntries`'s two modes): in a Pi folder
/// only at its top, in an Agent Skills folder only below it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pi,
    Agents,
}

/// The project-trust state for the repo, and what decided it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Trust {
    /// No trust-gated resource in the project: nothing to decide.
    NotNeeded,
    Trusted,
    Untrusted,
    /// Pi will ask at startup (`defaultProjectTrust: "ask"`, the default, and no saved decision).
    Ask,
}

struct Paths {
    /// The agent directory: `~/.pi/agent`, or `$PI_CODING_AGENT_DIR`.
    agent: PathBuf,
    /// The repo's `.pi`.
    project: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let agent = match ctx.var("PI_CODING_AGENT_DIR") {
            Some("~") => ctx.home.clone(),
            Some(v) => v.strip_prefix("~/").map_or_else(|| PathBuf::from(v), |rest| ctx.home.join(rest)),
            None => ctx.home.join(".pi/agent"),
        };
        Self { agent, project: ctx.repo.join(".pi"), repo: ctx.repo.clone(), home: ctx.home.clone() }
    }
}

pub struct Pi;

impl Adapter for Pi {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["PI_CODING_AGENT_DIR"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.project.clone(), ".pi/", 100),
            (p.repo.join(".agents/skills"), ".agents/skills/", 60),
            (p.agent.clone(), "Pi agent folder", 45),
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

        let user_cfg = b.json(&p.agent.join("settings.json")).object();
        let project_file = b.json(&p.project.join("settings.json")).object();

        // Lower rank wins: project resources come before user ones in Pi's resource precedence
        // (`resourcePrecedenceRank` in package-manager.ts), and project settings deep-merge over user settings.
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .pi directory (behind project trust), its .agents/skills, and the AGENTS.md chain",
            Some(b.display(&p.project)),
            false,
            LayerMode::Default,
        );
        b.add_layer(
            "user",
            LayerKind::User,
            "the agent directory ($PI_CODING_AGENT_DIR, default ~/.pi/agent) and ~/.agents/skills",
            Some(b.display(&p.agent)),
            ctx.fs.exists(&p.agent),
            LayerMode::Default,
        );

        let mut run = Run { b: &mut b, p, user_cfg, project_file, project_cfg: Map::new(), trust: Trust::NotNeeded };
        run.trust();
        run.settings();
        run.instructions();
        run.system_files();
        let packages = run.packages();
        run.skills(&packages);
        run.prompts(&packages);
        run.mcp();
        run.b.link_packages();
        let layer_present = run.b.items.iter().any(|i| i.origin.layer == "project")
            || run.trust != Trust::NotNeeded
            || run.b.fs().exists(&run.p.project);
        if layer_present {
            run.b.set_present("project");
        }
        if run.b.items.iter().any(|i| i.origin.layer == "user") {
            run.b.set_present("user");
        }

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the built-in tools' descriptions and the skills preamble".into(),
            why: "they come with Pi, not from any file".into(),
        });
        run.b.not_measured.push(NotMeasured {
            what: "tools, prompt text and event handlers that extensions add".into(),
            why: "extensions are TypeScript that rigswig lists but never runs".into(),
        });
        run.b.not_measured_common();

        for note in [
            "extensions (.pi/extensions, the agent directory's extensions/, packages' extensions and the built-in builtin:llama.cpp, builtin:codemode and builtin:tool-search): executable TypeScript whose tools, commands, hooks (tool_call, session_start, before_agent_start ...) and prompt changes are only known by running it",
            "a project_trust decision an extension makes, and the --approve / --no-approve flags: rigswig reads trust.json and defaultProjectTrust only",
            "the command-line flags (--model, --provider, --tools, --no-context-files, --skill, --prompt-template, -e) and the session directory",
            "glob patterns and !/+/- overrides in the skills, prompts and extensions settings, and a package's object-form filters other than an empty list: such entries are reported, not applied",
            ".gitignore, .ignore and .fdignore files inside skill and prompt folders, which Pi honours while it walks them",
            "the main repository's context file that a nested linked worktree's own copy shadows",
            "MCP exposure and toolExposure (which tools reach the model, directly or through tool search), OAuth state, and servers registered by extensions",
            "packages installed in the legacy global npm root, temporary -e installs, and a git package's ref",
            "themes, keybindings, models.json and auth.json, which do not change what loads into the context",
        ] {
            run.b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        drop(run);
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

/// One package from a `packages` setting: where it is installed and which resources it may contribute.
struct Pkg {
    id: String,
    dir: Option<PathBuf>,
    /// False when the object form's `skills` (or `prompts`) list is empty, which turns that kind off.
    skills: bool,
    prompts: bool,
}

struct Run<'b, 'a> {
    b: &'b mut Build<'a>,
    p: Paths,
    user_cfg: Map<String, Value>,
    /// The project's settings file as written, read whether or not it applies.
    project_file: Map<String, Value>,
    /// The project's settings as they apply: empty unless the project is trusted.
    project_cfg: Map<String, Value>,
    trust: Trust,
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    fn project_loads(&self) -> bool {
        matches!(self.trust, Trust::Trusted | Trust::Ask | Trust::NotNeeded)
    }

    /// Project trust (docs/security.md, trust-manager.ts): asked only when the project has a trust-gated
    /// resource; a saved decision in `<agent>/trust.json` for the repo or its nearest ancestor applies, then the
    /// user-only `defaultProjectTrust` setting.
    fn trust(&mut self) {
        // The gate looks at every ancestor's .agents/skills (not only up to the git root, as discovery does).
        let user_agents = self.p.home.join(".agents/skills");
        let gated = TRUST_GATED.iter().any(|e| self.fs().exists(&self.p.project.join(e)))
            || self
                .p
                .repo
                .ancestors()
                .map(|d| d.join(".agents/skills"))
                .any(|d| d != user_agents && self.fs().exists(&d));
        if !gated {
            self.trust = Trust::NotNeeded;
            self.project_cfg = self.project_file.clone();
            return;
        }
        let trust_file = self.p.agent.join("trust.json");
        let saved = self.b.json(&trust_file).object();
        let canonical = self.fs().canonicalize(&self.p.repo).unwrap_or_else(|_| self.p.repo.clone());
        let mut decided: Option<(bool, String)> = None;
        'walk: for dir in canonical.ancestors() {
            for (key, v) in &saved {
                if Path::new(key) == dir || (self.repo_alias(key, dir)) {
                    if let Some(t) = v.as_bool() {
                        decided = Some((t, key.clone()));
                        break 'walk;
                    }
                }
            }
        }
        let default = self.user_cfg.get("defaultProjectTrust").and_then(Value::as_str).unwrap_or("ask");
        let (trust, value, note) = match decided {
            Some((true, key)) => (Trust::Trusted, "trusted", format!("saved in trust.json for {}", self.show(&key))),
            Some((false, key)) => {
                (Trust::Untrusted, "untrusted", format!("saved in trust.json for {}", self.show(&key)))
            }
            None => match default {
                "always" => (Trust::Trusted, "trusted", "no saved decision; defaultProjectTrust is always".to_string()),
                "never" => {
                    (Trust::Untrusted, "untrusted", "no saved decision; defaultProjectTrust is never".to_string())
                }
                _ => {
                    (Trust::Ask, "ask", "no saved decision; Pi asks at startup (defaultProjectTrust: ask)".to_string())
                }
            },
        };
        self.trust = trust;
        self.b.settings.insert(
            "projectTrust".into(),
            Decision {
                value: Value::String(value.into()),
                source: Some("user".into()),
                trail: vec![TrailStep { layer: "user".into(), value: Value::String(value.into()), note: Some(note) }],
            },
        );
        match trust {
            Trust::Untrusted => self.b.warn(
                "project-untrusted",
                "Pi does not trust this project, so its .pi settings, MCP servers, skills, prompts and system files and its .agents/skills do not load; AGENTS.md files still do",
                None,
            ),
            Trust::Ask => self.b.warn(
                "project-trust-undecided",
                "this project has .pi resources or .agents/skills and no saved trust decision: Pi asks at startup (print and RPC modes skip them); rigswig counts them as if trusted",
                None,
            ),
            _ => {}
        }
        if self.project_loads() {
            self.project_cfg = self.project_file.clone();
        }
    }

    /// A trust.json key names `dir` when both resolve to the same folder (the repo may be reached through a link).
    fn repo_alias(&self, key: &str, dir: &Path) -> bool {
        self.fs().canonicalize(Path::new(key)).is_ok_and(|k| k == dir)
    }

    fn show(&self, key: &str) -> String {
        self.b.display(Path::new(key))
    }

    /// The project `.agents/skills` folders: the repo and each ancestor up to the git root (all the way up when
    /// there is none), never the user's own `~/.agents/skills`.
    fn agents_skill_dirs(&self) -> Vec<PathBuf> {
        let root = self.p.repo.ancestors().find(|d| self.fs().exists(&d.join(".git")));
        let mut out = Vec::new();
        for d in self.p.repo.ancestors() {
            let s = d.join(".agents/skills");
            if s != self.p.home.join(".agents/skills") {
                out.push(s);
            }
            if Some(d) == root {
                break;
            }
        }
        out
    }

    /// The settings that decide behaviour. Project values deep-merge over user values (settings-manager.ts); a
    /// project file that trust keeps out is not in the trail.
    fn settings(&mut self) {
        for (key, name) in [
            ("defaultModel", "model"),
            ("defaultProvider", "provider"),
            ("defaultThinkingLevel", "thinkingLevel"),
            ("defaultTools", "defaultTools"),
            ("enableSkillCommands", "enableSkillCommands"),
        ] {
            let mut trail = Vec::new();
            for (layer, cfg) in [("project", &self.project_cfg), ("user", &self.user_cfg)] {
                if let Some(v) = cfg.get(key).filter(|v| !v.is_null()) {
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
        // User-only: a project value is ignored (docs/settings.md).
        if let Some(v) = self.user_cfg.get("defaultProjectTrust").and_then(Value::as_str) {
            let value = Value::String(printable(v));
            self.b.settings.insert(
                "defaultProjectTrust".into(),
                Decision {
                    value: value.clone(),
                    source: Some("user".into()),
                    trail: vec![TrailStep { layer: "user".into(), value, note: None }],
                },
            );
        }
        for key in ["skills", "prompts"] {
            let patterned = [&self.user_cfg, &self.project_cfg].iter().any(|c| {
                c.get(key).and_then(Value::as_array).is_some_and(|a| {
                    a.iter().filter_map(Value::as_str).any(|s| s.starts_with(['!', '+', '-']) || s.contains(['*', '?']))
                })
            });
            if patterned {
                self.b.warn(
                    "settings-patterns-not-applied",
                    format!("the {key} setting uses glob or !/+/- patterns; rigswig lists what the folders hold without applying them"),
                    None,
                );
            }
        }
    }

    /// Context files: the agent directory's, then one per directory from the filesystem root down to the working
    /// directory, the first candidate in each directory winning (resource-loader.ts `loadProjectContextFiles`).
    /// They load whatever the project's trust state.
    fn instructions(&mut self) {
        let agent = self.p.agent.clone();
        self.context_dir(&agent, "user");
        let dirs: Vec<PathBuf> = self.p.repo.ancestors().map(Path::to_path_buf).collect();
        for d in dirs.iter().rev() {
            if *d == agent {
                continue;
            }
            self.context_dir(d, "project");
        }
    }

    /// One directory's context file: the first candidate loads, and the others there are shadowed by it.
    fn context_dir(&mut self, dir: &Path, layer: &str) {
        let mut winner: Option<(String, PathBuf)> = None;
        let listed: Vec<String> = self.fs().read_dir(dir).unwrap_or_default().into_iter().map(|e| e.name).collect();
        let mut handled: Vec<String> = Vec::new();
        for name in CONTEXT_CANDIDATES {
            let f = dir.join(name);
            if !matches!(self.fs().kind(&f), Ok(EntryKind::File)) {
                continue;
            }
            // On a case-insensitive disk AGENTS.MD opens AGENTS.md: the same file, not a second one. A spelling
            // already handled counts again only when the folder really holds a file of this exact name.
            let lower = name.to_lowercase();
            if handled.contains(&lower) && !listed.iter().any(|n| n == name) {
                continue;
            }
            handled.push(lower);
            let real = self.fs().canonicalize(&f).unwrap_or_else(|_| f.clone());
            let Ok(text) = self.fs().read_to_string(&f) else { continue };
            let shown = self.b.display(&f);
            let mut it = self.b.item(
                layer,
                &shown,
                ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to: Vec::new() }),
                LoadWhen::Startup,
                calc(char_len(text.trim())),
            );
            it.origin.file = Some(shown);
            it.load.content_hash = Some(sha256_hex(&text));
            match &winner {
                Some((id, _)) => {
                    let id = id.clone();
                    Build::shadow(&mut it, &id);
                    self.b.push(it);
                }
                None => {
                    let ix = self.b.push(it);
                    winner = Some((self.b.items[ix].id.clone(), real));
                }
            }
        }
    }

    /// `SYSTEM.md` replaces Pi's system prompt and `APPEND_SYSTEM.md` is added to it; for each, the project's copy
    /// (behind trust) wins over the agent directory's (resource-loader.ts).
    fn system_files(&mut self) {
        for name in ["SYSTEM.md", "APPEND_SYSTEM.md"] {
            let mut winner: Option<String> = None;
            for (layer, dir) in [("project", self.p.project.clone()), ("user", self.p.agent.clone())] {
                let f = dir.join(name);
                let Ok(text) = self.fs().read_to_string(&f) else { continue };
                let shown = self.b.display(&f);
                let mut it = self.b.item(
                    layer,
                    &shown,
                    ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to: Vec::new() }),
                    LoadWhen::Startup,
                    calc(char_len(text.trim())),
                );
                it.origin.file = Some(shown);
                it.load.content_hash = Some(sha256_hex(&text));
                if layer == "project" && !self.project_loads() {
                    Build::disable(&mut it);
                    self.b.push(it);
                    continue;
                }
                match &winner {
                    Some(w) => {
                        let w = w.clone();
                        Build::shadow(&mut it, &w);
                        self.b.push(it);
                    }
                    None => {
                        let ix = self.b.push(it);
                        winner = Some(self.b.items[ix].id.clone());
                        if name == "SYSTEM.md" {
                            self.b.not_measured.push(NotMeasured {
                                what: "SYSTEM.md replaces Pi's built-in system prompt".into(),
                                why: "the built-in prompt it removes is not measured either".into(),
                            });
                        }
                    }
                }
            }
        }
    }

    /// Packages from the `packages` setting of each scope (docs/packages.md). The same package in both scopes is the
    /// project's (identity: npm name, git host and path, or local path). Installed under `<scope>/npm/node_modules`
    /// or `<scope>/git/<host>/<path>`; a local path is read where it points.
    fn packages(&mut self) -> Vec<Pkg> {
        let mut out: Vec<Pkg> = Vec::new();
        let scopes = [
            ("project", self.project_cfg.clone(), self.p.project.clone()),
            ("user", self.user_cfg.clone(), self.p.agent.clone()),
        ];
        for (scope, cfg, base) in scopes {
            for entry in cfg.get("packages").and_then(Value::as_array).into_iter().flatten() {
                let (source, obj) = match entry {
                    Value::String(s) => (s.clone(), None),
                    Value::Object(o) => match o.get("source").and_then(Value::as_str) {
                        Some(s) => (s.to_string(), Some(o.clone())),
                        None => continue,
                    },
                    _ => continue,
                };
                let Some((id, label, dir)) = self.locate(&source, &base) else {
                    self.b.warn("package-unreadable", "a packages entry rigswig cannot place was skipped", None);
                    continue;
                };
                if out.iter().any(|p| p.id == id) {
                    // The project's entry replaces the user's.
                    if let Some(pk) = self.b.packages.iter_mut().find(|p| p.id == id) {
                        pk.scopes.insert(scope.into(), true);
                    }
                    continue;
                }
                let empty = |k: &str| {
                    obj.as_ref().and_then(|o| o.get(k)).and_then(Value::as_array).is_some_and(|a| a.is_empty())
                };
                let installed = dir.as_ref().is_some_and(|d| self.fs().exists(d));
                if !installed {
                    self.b.warn(
                        "package-not-installed",
                        format!(
                            "package {} is in the {scope} settings but not installed; Pi installs it at startup",
                            printable(&label)
                        ),
                        None,
                    );
                }
                let mut scopes = std::collections::BTreeMap::new();
                scopes.insert(scope.to_string(), true);
                self.b.packages.push(Package {
                    id: id.clone(),
                    kind: PackageKind::Pack,
                    source: printable(&label),
                    enabled: true,
                    decided_by: Some(scope.into()),
                    scopes,
                    contributes: Vec::new(),
                });
                let layer = format!("plugin:{id}");
                let rank_label = format!("Pi package {}", printable(&label));
                self.b.add_layer(
                    &layer,
                    LayerKind::Package,
                    &rank_label,
                    dir.as_ref().map(|d| self.b.display(d)),
                    installed,
                    LayerMode::Default,
                );
                out.push(Pkg {
                    id,
                    dir: dir.filter(|_| installed),
                    skills: !empty("skills"),
                    prompts: !empty("prompts"),
                });
            }
        }
        out
    }

    /// A package source's identity, its label, and its install folder.
    fn locate(&self, source: &str, base: &Path) -> Option<(String, String, Option<PathBuf>)> {
        let s = source.trim();
        if let Some(spec) = s.strip_prefix("npm:") {
            let name = npm_name(spec.trim());
            if name.is_empty() {
                return None;
            }
            let dir = base.join("npm/node_modules").join(&name);
            return Some((safe_id(&name), format!("npm:{name}"), Some(dir)));
        }
        let git = s.strip_prefix("git:").map(str::trim);
        let is_url = s.contains("://");
        if git.is_some() || is_url {
            let url = git.unwrap_or(s);
            let (host, path) = git_host_path(url)?;
            let dir = base.join("git").join(&host).join(&path);
            let label = format!("{host}/{path}");
            return Some((safe_id(&label), label, Some(dir)));
        }
        // A local path, relative to the settings file's folder.
        let dir = match s.strip_prefix("~/") {
            Some(rest) => self.p.home.join(rest),
            None if Path::new(s).is_absolute() => PathBuf::from(s),
            None => base.join(s),
        };
        let label = dir.file_name().map_or_else(|| "local".into(), |n| n.to_string_lossy().into_owned());
        Some((safe_id(&format!("local-{label}")), label, Some(dir)))
    }

    /// A package's resource folders of one kind: the `pi` key's list in its package.json, else the conventional one.
    fn package_dirs(&mut self, dir: &Path, kind: &str) -> Vec<PathBuf> {
        let manifest = dir.join("package.json");
        if self.fs().exists(&manifest) {
            let m = self.b.json(&manifest).object();
            if let Some(pi) = m.get("pi").and_then(Value::as_object) {
                return pi
                    .get(kind)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.contains(['*', '?']) && !s.starts_with(['!', '+', '-']))
                    .map(|s| dir.join(s.trim_start_matches("./")))
                    .filter(|p| crate::fs::is_within(p, dir))
                    .collect();
            }
        }
        vec![dir.join(kind)]
    }

    /// Skills, in Pi's precedence (package-manager.ts `resourcePrecedenceRank` and `addAutoDiscoveredResources`):
    /// project settings paths, `.pi/skills`, the project `.agents/skills` chain, user settings paths, the agent
    /// directory's `skills/`, `~/.agents/skills`, then packages. The first skill of a name wins (skills.ts).
    fn skills(&mut self, packages: &[Pkg]) {
        let mut roots: Vec<(String, PathBuf, Mode, bool)> = Vec::new();
        let gated = !self.project_loads();
        for p in self.setting_paths(&self.project_file.clone(), "skills", &self.p.project.clone()) {
            roots.push(("project".into(), p, Mode::Pi, gated));
        }
        roots.push(("project".into(), self.p.project.join("skills"), Mode::Pi, gated));
        for d in self.agents_skill_dirs() {
            roots.push(("project".into(), d, Mode::Agents, gated));
        }
        for p in self.setting_paths(&self.user_cfg.clone(), "skills", &self.p.agent.clone()) {
            roots.push(("user".into(), p, Mode::Pi, false));
        }
        roots.push(("user".into(), self.p.agent.join("skills"), Mode::Pi, false));
        roots.push(("user".into(), self.p.home.join(".agents/skills"), Mode::Agents, false));
        for pkg in packages.iter().filter(|p| p.skills) {
            let Some(dir) = &pkg.dir else { continue };
            for d in self.package_dirs(dir, "skills") {
                roots.push((format!("plugin:{}", pkg.id), d, Mode::Pi, false));
            }
        }

        let mut idx = Vec::new();
        let mut seen_files: Vec<PathBuf> = Vec::new();
        for (layer, root, mode, blocked) in roots {
            let files = if matches!(self.fs().kind(&root), Ok(EntryKind::File)) {
                vec![root.clone()]
            } else {
                let mut v = Vec::new();
                self.collect_skills(&root, &root, mode, 0, &mut v);
                v
            };
            for f in files {
                let real = self.fs().canonicalize(&f).unwrap_or_else(|_| f.clone());
                if seen_files.contains(&real) {
                    continue;
                }
                seen_files.push(real);
                if let Some(ix) = self.add_skill(&f, &layer, blocked) {
                    idx.push(ix);
                }
            }
        }
        // Pushed in precedence order, so the first effective one of a name wins.
        let before: Vec<bool> = idx.iter().map(|&i| self.b.items[i].effective).collect();
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |_| 0);
        if idx.iter().zip(before).any(|(&i, was)| was && !self.b.items[i].effective) {
            self.b.warn(
                "skill-name-collision",
                "two skills share a name; Pi keeps the first one it finds and warns",
                None,
            );
        }
    }

    /// `collectSkillEntries`: a folder with a SKILL.md is one skill and the walk stops there; otherwise loose `.md`
    /// files count (a Pi folder's top level only, an Agent Skills folder's subfolders only) and subfolders are walked.
    fn collect_skills(&self, root: &Path, dir: &Path, mode: Mode, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > MAX_DEPTH {
            return;
        }
        let Ok(entries) = self.fs().read_dir(dir) else { return };
        let skill = dir.join("SKILL.md");
        if entries.iter().any(|e| e.name == "SKILL.md") && matches!(self.fs().kind(&skill), Ok(EntryKind::File)) {
            out.push(skill);
            return;
        }
        for e in entries {
            if e.name.starts_with('.') || e.name == "node_modules" {
                continue;
            }
            let p = dir.join(&e.name);
            match self.fs().kind(&p) {
                Ok(EntryKind::Dir) => self.collect_skills(root, &p, mode, depth + 1, out),
                Ok(EntryKind::File) if e.name.ends_with(".md") => {
                    let top = dir == root;
                    if (mode == Mode::Pi && top) || (mode == Mode::Agents && !top) {
                        out.push(p);
                    }
                }
                _ => {}
            }
        }
    }

    /// One skill: needs a non-empty description; its name is the front matter's, else its folder's. The listing
    /// (name, description and the file's path) loads at startup unless `disable-model-invocation` hides it.
    fn add_skill(&mut self, f: &Path, layer: &str, blocked: bool) -> Option<usize> {
        let text = self.fs().read_to_string(f).ok()?;
        let fm = frontmatter::parse(&text);
        let description = fm.str("description").filter(|d| !d.trim().is_empty())?.to_string();
        let name = fm
            .str("name")
            .map(str::to_string)
            .or_else(|| f.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()))?;
        let hidden = fm.str("disable-model-invocation") == Some("true");
        let body = char_len(text[fm.body_start..].trim());
        let listing = char_len(&name) + char_len(&description) + char_len(&f.to_string_lossy());
        let (when, chars) = if hidden { (LoadWhen::OnInvoke, 0) } else { (LoadWhen::Startup, listing) };
        let mut it =
            self.b.item(layer, &name, ItemDetail::Skill(SkillAttrs { body_chars: Some(body) }), when, calc(chars));
        it.origin.file = Some(self.b.display(f));
        it.load.content_hash = Some(sha256_hex(&text));
        if blocked {
            Build::disable(&mut it);
        }
        Some(self.b.push(it))
    }

    /// Plain path entries of a resource setting (skills, prompts), relative to the settings file's folder. Entries
    /// with glob or override patterns are not applied (see `settings`).
    fn setting_paths(&self, cfg: &Map<String, Value>, key: &str, base: &Path) -> Vec<PathBuf> {
        cfg.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|s| !s.is_empty() && !s.starts_with(['!', '+', '-']) && !s.contains(['*', '?']))
            .map(|s| match s.strip_prefix("~/") {
                Some(rest) => self.p.home.join(rest),
                None if Path::new(s).is_absolute() => PathBuf::from(s),
                None => base.join(s),
            })
            .collect()
    }

    /// Prompt templates (docs/prompt-templates.md): the top-level `.md` files of `.pi/prompts` (behind trust) and
    /// the agent directory's `prompts/`, settings paths and packages, in the skills' precedence; the file name is the
    /// command and the first of a name wins. Nothing loads until one is invoked, and then its body does.
    fn prompts(&mut self, packages: &[Pkg]) {
        let gated = !self.project_loads();
        let mut roots: Vec<(String, PathBuf, bool)> = Vec::new();
        for p in self.setting_paths(&self.project_file.clone(), "prompts", &self.p.project.clone()) {
            roots.push(("project".into(), p, gated));
        }
        roots.push(("project".into(), self.p.project.join("prompts"), gated));
        for p in self.setting_paths(&self.user_cfg.clone(), "prompts", &self.p.agent.clone()) {
            roots.push(("user".into(), p, false));
        }
        roots.push(("user".into(), self.p.agent.join("prompts"), false));
        for pkg in packages.iter().filter(|p| p.prompts) {
            let Some(dir) = &pkg.dir else { continue };
            for d in self.package_dirs(dir, "prompts") {
                roots.push((format!("plugin:{}", pkg.id), d, false));
            }
        }
        let mut idx = Vec::new();
        for (layer, root, blocked) in roots {
            let files = if matches!(self.fs().kind(&root), Ok(EntryKind::File)) {
                vec![root]
            } else {
                self.b.files_in(&root, ".md")
            };
            for f in files {
                let Ok(text) = self.fs().read_to_string(&f) else { continue };
                let fm = frontmatter::parse(&text);
                let name = f.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                let body = char_len(text[fm.body_start..].trim());
                let mut it =
                    self.b.item(&layer, &name, ItemDetail::Command(CommandAttrs {}), LoadWhen::OnInvoke, calc(body));
                it.origin.file = Some(self.b.display(&f));
                it.load.content_hash = Some(sha256_hex(&text));
                if blocked {
                    Build::disable(&mut it);
                }
                idx.push(self.b.push(it));
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |_| 0);
    }

    /// MCP servers (docs/mcp.md): `<agent>/mcp.json` and `.pi/mcp.json` (behind trust); a project entry replaces the
    /// user's of the same name, and one with no `command`, `url` or `type` only overrides `enabled`. The built-in
    /// `builtin:mcp` extension serves them, so `-builtin:mcp` in the extensions setting turns them all off. A
    /// server's `description` lists it in the system prompt.
    fn mcp(&mut self) {
        let off = |c: &Map<String, Value>| {
            c.get("extensions")
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().filter_map(Value::as_str).any(|s| s == "-builtin:mcp" || s == "!builtin:mcp"))
        };
        let builtin_off = off(&self.project_cfg) || off(&self.user_cfg);
        let user_file = self.p.agent.join("mcp.json");
        let project_file = self.p.project.join("mcp.json");
        let user = if self.fs().exists(&user_file) { self.b.json(&user_file).object() } else { Map::new() };
        let project = if self.fs().exists(&project_file) { self.b.json(&project_file).object() } else { Map::new() };
        let servers =
            |m: &Map<String, Value>| m.get("mcpServers").and_then(Value::as_object).cloned().unwrap_or_default();
        let (user, project) = (servers(&user), servers(&project));
        let project_loads = self.project_loads();

        let defines = |v: &Value| ["command", "url", "type"].iter().any(|k| v.get(k).is_some());
        let mut idx = Vec::new();
        for (name, cfg) in &project {
            if !defines(cfg) {
                continue; // An override of the user's server, applied below.
            }
            let shown = self.b.display(&project_file);
            let ix = self.declare_mcp(name, cfg, "project", &shown);
            if !project_loads {
                Build::disable(&mut self.b.items[ix]);
            }
            idx.push(ix);
        }
        for (name, cfg) in &user {
            let mut cfg = cfg.clone();
            if project_loads {
                if let Some(o) = project.get(name).filter(|v| !defines(v)) {
                    if let (Some(dst), Some(src)) = (cfg.as_object_mut(), o.as_object()) {
                        for k in ["enabled", "exposure", "toolExposure"] {
                            if let Some(v) = src.get(k) {
                                dst.insert(k.into(), v.clone());
                            }
                        }
                    }
                }
            }
            let shown = self.b.display(&user_file);
            idx.push(self.declare_mcp(name, &cfg, "user", &shown));
        }
        if builtin_off {
            for &i in &idx {
                if let ItemDetail::Mcp(m) = &mut self.b.items[i].detail {
                    m.state = McpState::Disabled;
                }
                Build::disable(&mut self.b.items[i]);
            }
            if !idx.is_empty() {
                self.b.warn(
                    "mcp-builtin-off",
                    "the extensions setting turns off builtin:mcp, so no MCP server connects",
                    None,
                );
            }
        }
        // A project server replaces the user's of the same name; server names that differ only in - and _ are one.
        self.b.resolve_clashes(&idx, |i| i.name.replace('-', "_"), |i| u32::from(i.origin.layer != "project"));
    }

    fn declare_mcp(&mut self, name: &str, cfg: &Value, layer: &str, file: &str) -> usize {
        let kind = cfg.get("type").and_then(Value::as_str);
        let (transport, target) = if let Some(u) = cfg.get("url").and_then(Value::as_str) {
            (if kind == Some("sse") { McpTransport::Sse } else { McpTransport::Http }, url_origin(u))
        } else if let Some(c) = cfg.get("command").and_then(Value::as_str) {
            (McpTransport::Stdio, Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned()))
        } else {
            (McpTransport::Unknown, None)
        };
        let disabled = cfg.get("enabled").and_then(Value::as_bool) == Some(false) || kind == Some("sse");
        let state = if disabled { McpState::Disabled } else { McpState::Active };
        // The description is the server's line in the system prompt; without one, the server's own instructions
        // supply it once connected (not measured).
        let desc = cfg.get("description").and_then(Value::as_str).map_or(0, |d| char_len(name) + char_len(d));
        let mut it = self.b.item(
            layer,
            name,
            ItemDetail::Mcp(McpAttrs { transport, target, state }),
            LoadWhen::Startup,
            calc(desc),
        );
        it.origin.file = Some(file.to_string());
        if disabled {
            Build::disable(&mut it);
        }
        if kind == Some("sse") {
            self.b.warn("mcp-sse-rejected", format!("MCP server {} uses SSE, which Pi rejects", printable(name)), None);
        }
        self.b.push(it)
    }
}

/// An npm spec's package name, without its version (`@scope/name@1.2` -> `@scope/name`).
fn npm_name(spec: &str) -> String {
    let (scope, rest) = match spec.strip_prefix('@') {
        Some(r) => ("@", r),
        None => ("", spec),
    };
    let name = rest.split('@').next().unwrap_or("");
    format!("{scope}{name}")
}

/// A git source's host and repository path, without scheme, credentials, `.git` or `@ref`.
fn git_host_path(url: &str) -> Option<(String, String)> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.rsplit_once('@').filter(|(a, _)| !a.contains('/')).map_or(rest, |(_, r)| r);
    let (host, path) = rest.split_once(['/', ':'])?;
    let path = path.split('@').next().unwrap_or("").trim_end_matches('/').trim_end_matches(".git");
    if host.is_empty() || path.is_empty() || path.split('/').any(|s| s == ".." || s.is_empty()) {
        return None;
    }
    Some((host.to_string(), path.to_string()))
}

/// A package id safe in item ids and layer names.
fn safe_id(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | '/') { c } else { '-' })
        .collect()
}
