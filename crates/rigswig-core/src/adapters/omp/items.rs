//! Everything else a session gets from files: the system-prompt files, rules, skills, commands and prompt
//! templates, task agents, MCP servers and hooks. Each capability gathers its items in OMP's provider order (priority,
//! then registration), and within a provider in the order that provider emits them; the first of a key wins
//! (`capability/index.ts`). Sources are in `docs/adapters/omp.md`.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::adapters::build::{Build, calc, char_len};
use crate::fs::EntryKind;
use crate::model::*;
use crate::read::frontmatter::{self, FmValue};
use crate::read::redact::{printable, sha256_hex, url_origin};

use super::Run;

/// The skill listing shows a whitespace-collapsed preview of each description, cut at about this many characters
/// (`extensibility/skill-descriptions.ts`).
const SKILL_PREVIEW: usize = 100;

/// One file a capability found, before the first-of-a-key rule runs.
struct Found {
    provider: &'static str,
    scope: &'static str,
    path: PathBuf,
    name: String,
}

impl Run<'_, '_> {
    fn is_file(&self, f: &Path) -> bool {
        matches!(self.fs().kind(f), Ok(EntryKind::File))
    }

    /// `*.ext` files directly in `dir`, sorted, dot-files skipped.
    fn files(&self, dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            if e.name.starts_with('.') {
                continue;
            }
            let p = dir.join(&e.name);
            if exts.iter().any(|x| e.name.ends_with(x)) && self.is_file(&p) {
                out.push(p);
            }
        }
        out
    }

    /// `*.ext` files under `dir`, recursively, with their path relative to `dir`.
    fn files_deep(&self, dir: &Path, ext: &str) -> Vec<(PathBuf, String)> {
        self.b.walk(dir, ext)
    }

    fn agents_walk_dirs(&self) -> Vec<PathBuf> {
        self.project_ancestors().into_iter().map(|(d, _)| d).collect()
    }

    // ---------------------------------------------------------------------------------------------------------
    // System prompt files
    // ---------------------------------------------------------------------------------------------------------

    /// `SYSTEM.md` (a custom instruction block) and `SYSTEM_TEMPLATE.md` (a whole template): one is used, picked as
    /// project text, project template, user text, user template, each taken from the highest-priority provider
    /// (native, claude, agents, codex, gemini; Gemini's text file is `system.md`). `APPEND_SYSTEM.md`: the first that
    /// exists of the repo's `.omp`, `.claude`, `.codex`, `.gemini`, then the user's (opted-in) homes.
    pub(super) fn system_prompt(&mut self) {
        let mut cands: Vec<(&'static str, &'static str, &'static str, PathBuf)> = Vec::new();
        let nearest = self.nearest_omp().map(|(d, _)| d);
        let r = self.p.repo.clone();
        let h = self.p.home.clone();
        for (kind, native_name, gemini_name) in
            [("text", "SYSTEM.md", "system.md"), ("template", "SYSTEM_TEMPLATE.md", "SYSTEM_TEMPLATE.md")]
        {
            if let Some(d) = &nearest {
                cands.push(("native", "project", kind, d.join(native_name)));
            }
            cands.push(("native", "user", kind, self.p.agent.join(native_name)));
            cands.push(("claude", "project", kind, r.join(".claude").join(native_name)));
            cands.push(("claude", "user", kind, self.p.claude_user.join(native_name)));
            for d in self.agents_walk_dirs() {
                for sub in [".agent", ".agents"] {
                    cands.push(("agents", "project", kind, d.join(sub).join(native_name)));
                }
            }
            for sub in [".agent", ".agents"] {
                cands.push(("agents", "user", kind, h.join(sub).join(native_name)));
            }
            cands.push(("codex", "project", kind, r.join(".codex").join(native_name)));
            cands.push(("codex", "user", kind, self.p.codex_user.join(native_name)));
            cands.push(("gemini", "project", kind, r.join(".gemini").join(gemini_name)));
            cands.push(("gemini", "user", kind, h.join(".gemini").join(gemini_name)));
        }
        let mut picked: Option<String> = None;
        for (scope, kind) in [("project", "text"), ("project", "template"), ("user", "text"), ("user", "template")] {
            for (provider, s, k, f) in &cands {
                if *s != scope || *k != kind || !self.source_on(provider, scope) {
                    continue;
                }
                let Ok(text) = self.fs().read_to_string(f) else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                let ix = self.instruction_item(provider, scope, f, &text, LoadWhen::Startup);
                match &picked {
                    Some(w) => {
                        let w = w.clone();
                        Build::shadow(&mut self.b.items[ix], &w);
                    }
                    None => {
                        picked = Some(self.b.items[ix].id.clone());
                        self.b.not_measured.push(NotMeasured {
                            what: format!("{} replaces OMP's default instruction block", self.b.display(f)),
                            why: "the default block it removes is not measured either".into(),
                        });
                    }
                }
            }
        }

        let mut append: Vec<(&'static str, &'static str, PathBuf)> = vec![
            ("native", "project", r.join(".omp/APPEND_SYSTEM.md")),
            ("claude", "project", r.join(".claude/APPEND_SYSTEM.md")),
            ("codex", "project", r.join(".codex/APPEND_SYSTEM.md")),
            ("gemini", "project", r.join(".gemini/APPEND_SYSTEM.md")),
            ("native", "user", self.p.agent_plain.join("APPEND_SYSTEM.md")),
        ];
        for (provider, dir) in [
            ("claude", self.p.claude_user.clone()),
            ("codex", self.p.codex_user.clone()),
            ("gemini", h.join(".gemini")),
        ] {
            if self.source_on(provider, "user") {
                append.push((provider, "user", dir.join("APPEND_SYSTEM.md")));
            }
        }
        if let Some((provider, scope, f)) = append.into_iter().find(|(_, _, f)| self.is_file(f)) {
            if let Ok(text) = self.fs().read_to_string(&f) {
                self.instruction_item(provider, scope, &f, &text, LoadWhen::Startup);
            }
        }
    }

    fn instruction_item(&mut self, provider: &str, scope: &str, f: &Path, text: &str, when: LoadWhen) -> usize {
        let layer = self.layer(provider, scope);
        let shown = self.b.display(f);
        let mut it = self.b.item(
            &layer,
            &shown,
            ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to: Vec::new() }),
            when,
            calc(char_len(text)),
        );
        it.origin.file = Some(shown);
        it.load.content_hash = Some(sha256_hex(text));
        self.b.push(it)
    }

    // ---------------------------------------------------------------------------------------------------------
    // Rules
    // ---------------------------------------------------------------------------------------------------------

    /// Rule files from every provider, the first of a name winning. A rule with `alwaysApply: true` carries its whole
    /// body in every request; one with a `description` is a rulebook line (name, globs, description) whose body is
    /// read on demand; one with a TTSR condition fires mid-session; one with none of these never reaches the prompt
    /// (`capability/rule-buckets.ts`).
    pub(super) fn rules(&mut self) {
        let mut found: Vec<Found> = Vec::new();
        let exts = [".md", ".mdc"];
        let stem = |p: &Path, cut: &str| {
            p.file_name().map(|n| n.to_string_lossy().trim_end_matches(cut).to_string()).unwrap_or_default()
        };
        let add = |found: &mut Vec<Found>, provider, scope, path: PathBuf, name: String| {
            found.push(Found { provider, scope, path, name });
        };
        if let Some(d) = self.native_project_dir() {
            for f in self.files(&d.join("rules"), &exts) {
                let n = stem(&f, if f.extension().is_some_and(|e| e == "mdc") { ".mdc" } else { ".md" });
                add(&mut found, "native", "project", f, n);
            }
        }
        for f in self.files(&self.p.agent.join("rules"), &exts) {
            let n = stem(&f, if f.extension().is_some_and(|e| e == "mdc") { ".mdc" } else { ".md" });
            add(&mut found, "native", "user", f, n);
        }
        for d in self.agents_walk_dirs() {
            for sub in [".agent", ".agents"] {
                for f in self.files(&d.join(sub).join("rules"), &exts) {
                    let n = stem(&f, if f.extension().is_some_and(|e| e == "mdc") { ".mdc" } else { ".md" });
                    add(&mut found, "agents", "project", f, n);
                }
            }
        }
        for sub in [".agent", ".agents"] {
            for f in self.files(&self.p.home.join(sub).join("rules"), &exts) {
                let n = stem(&f, if f.extension().is_some_and(|e| e == "mdc") { ".mdc" } else { ".md" });
                add(&mut found, "agents", "user", f, n);
            }
        }
        // Cursor: the user's rules come first, so they win a name (`discovery/cursor.ts`).
        for f in self.files(&self.p.home.join(".cursor/rules"), &exts) {
            let n = stem(&f, if f.extension().is_some_and(|e| e == "mdc") { ".mdc" } else { ".md" });
            add(&mut found, "cursor", "user", f, n);
        }
        for f in self.files(&self.p.repo.join(".cursor/rules"), &exts) {
            let n = stem(&f, if f.extension().is_some_and(|e| e == "mdc") { ".mdc" } else { ".md" });
            add(&mut found, "cursor", "project", f, n);
        }
        let global_rules = self.p.home.join(".codeium/windsurf/memories/global_rules.md");
        if self.is_file(&global_rules) {
            add(&mut found, "windsurf", "user", global_rules, "global_rules".into());
        }
        for f in self.files(&self.p.repo.join(".windsurf/rules"), &[".md"]) {
            let n = stem(&f, ".md");
            add(&mut found, "windsurf", "project", f, n);
        }
        // Cline: the first `.clinerules` walking up to the filesystem root, a folder of rules or one file.
        if let Some(c) = self.p.repo.ancestors().map(|d| d.join(".clinerules")).find(|c| self.fs().exists(c)) {
            if self.is_file(&c) {
                add(&mut found, "cline", "project", c, "clinerules".into());
            } else {
                for f in self.files(&c, &[".md"]) {
                    let n = stem(&f, ".md");
                    add(&mut found, "cline", "project", f, n);
                }
            }
        }
        for (f, _) in self.files_deep(&self.p.repo.join(".github/instructions"), ".instructions.md") {
            let n = stem(&f, ".instructions.md");
            add(&mut found, "github", "project", f, n);
        }

        let disabled = self.settings.strs(&["disabledExtensions"]);
        let ttsr_off = self.settings.strs(&["ttsr", "disabledRules"]);
        let ttsr_on = self.settings.bool_or(&["ttsr", "enabled"], true);
        let mut idx = Vec::new();
        for f in found {
            if !self.source_on(f.provider, f.scope) {
                continue;
            }
            let Ok(text) = self.fs().read_to_string(&f.path) else { continue };
            let fm = frontmatter::parse(&text);
            if fm.str("enabled") == Some("false") {
                continue;
            }
            let body = text[fm.body_start..].trim();
            let mut globs: Vec<String> = match fm.fields.get("globs") {
                Some(FmValue::List(l)) => l.clone(),
                Some(FmValue::Str(s)) if !s.is_empty() => vec![s.clone()],
                _ => Vec::new(),
            };
            let mut always = fm.str("alwaysApply") == Some("true");
            let mut description = fm.str("description").map(str::to_string);
            if f.provider == "github" {
                match fm.str("applyTo") {
                    Some(a) => {
                        globs = a.split(',').map(|g| g.trim().to_string()).filter(|g| !g.is_empty()).collect();
                        if globs.iter().any(|g| ["*", "**", "**/*"].contains(&g.as_str())) {
                            always = true;
                        } else if description.is_none() {
                            description = Some(format!("GitHub Copilot instructions for {}", globs.join(", ")));
                        }
                    }
                    None => {
                        self.b.warn(
                            "rule-no-apply-to",
                            "a .github/instructions file has no applyTo; OMP lists it in the rulebook with no globs",
                            Some(&f.path),
                        );
                        if description.is_none() {
                            description = Some("GitHub Copilot instructions".into());
                        }
                    }
                }
            }
            let ttsr = ttsr_on && ["condition", "astCondition", "question"].iter().any(|k| fm.fields.contains_key(*k));
            let (when, chars) = if ttsr {
                (LoadWhen::OnMatch, char_len(body))
            } else if always {
                (LoadWhen::Startup, char_len(body))
            } else if let Some(d) = &description {
                (LoadWhen::Startup, char_len(&f.name) + char_len(d) + globs.iter().map(|g| char_len(g)).sum::<u64>())
            } else {
                (LoadWhen::Never, 0)
            };
            let layer = self.layer(f.provider, f.scope);
            let attrs = InstructionAttrs { imported_by: None, applies_to: globs };
            let mut it =
                self.b.item(&layer, &format!("rule:{}", f.name), ItemDetail::Instruction(attrs), when, calc(chars));
            it.origin.file = Some(self.b.display(&f.path));
            it.load.content_hash = Some(sha256_hex(&text));
            let off = disabled.iter().any(|d| *d == format!("rule:{}", f.name)) || ttsr_off.contains(&f.name);
            if off {
                Build::disable(&mut it);
            } else if when == LoadWhen::Never {
                Build::disable(&mut it);
                self.b.warn(
                    "rule-dropped",
                    format!(
                        "rule {} has no description and no alwaysApply, so OMP never puts it in the prompt",
                        printable(&f.name)
                    ),
                    Some(&f.path),
                );
            }
            idx.push(self.b.push(it));
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |_| 0);
    }

    // ---------------------------------------------------------------------------------------------------------
    // Skills
    // ---------------------------------------------------------------------------------------------------------

    /// Skills (`extensibility/skills.ts`): `<root>/<name>/SKILL.md`, one level, from `skills.customDirectories`
    /// first (they outrank provider skills), then every provider in order, then auto-learned skills. A second skill
    /// of a name that differs is not dropped: it loads as `<namespace>/<name>`; an identical copy is dropped.
    pub(super) fn skills(&mut self) {
        if !self.settings.bool_or(&["skills", "enabled"], true) {
            self.b.note_not_modelled("skills.enabled is false, so OMP loads no skills");
            return;
        }
        let s = |k: &str, d: bool| self.settings.bool_or(&["skills", k], d);
        let (claude_user, codex_user, claude_project) =
            (s("enableClaudeUser", false), s("enableCodexUser", false), s("enableClaudeProject", true));
        let (pi_user, pi_project, agents_user, agents_project) = (
            s("enablePiUser", true),
            s("enablePiProject", true),
            s("enableAgentsUser", true),
            s("enableAgentsProject", true),
        );

        // (provider, scope, root, description required)
        let mut roots: Vec<(&'static str, &'static str, PathBuf, bool)> = Vec::new();
        for d in self.settings.strs(&["skills", "customDirectories"]) {
            roots.push(("native", "user", super::expand(&self.p.home, &d), true));
        }
        if pi_project {
            for (d, _) in self.project_ancestors() {
                roots.push(("native", "project", d.join(".omp/skills"), true));
            }
        }
        if pi_user {
            roots.push(("native", "user", self.p.agent.join("skills"), true));
        }
        if claude_user || self.source_on("claude", "user") {
            roots.push(("claude", "user", self.p.claude_user.join("skills"), false));
        }
        if claude_project {
            for (d, _) in self.project_ancestors() {
                roots.push(("claude", "project", d.join(".claude/skills"), false));
            }
        }
        if agents_project {
            for (d, _) in self.project_ancestors() {
                for sub in [".agent", ".agents"] {
                    roots.push(("agents", "project", d.join(sub).join("skills"), false));
                }
            }
        }
        if agents_user {
            for sub in [".agent", ".agents"] {
                roots.push(("agents", "user", self.p.home.join(sub).join("skills"), false));
            }
        }
        if codex_user || self.source_on("codex", "user") {
            roots.push(("codex", "user", self.p.codex_user.join("skills"), false));
        }
        roots.push(("codex", "project", self.p.repo.join(".codex/skills"), false));
        if self.source_on("opencode", "user") {
            roots.push(("opencode", "user", self.p.home.join(".config/opencode/skills"), false));
        }
        roots.push(("opencode", "project", self.p.repo.join(".opencode/skills"), false));
        roots.push(("github", "project", self.p.repo.join(".github/skills"), true));
        roots.push(("managed-skills", "user", self.p.agent.join("managed-skills"), false));

        let disabled = self.settings.strs(&["disabledExtensions"]);
        let ignored = self.settings.strs(&["skills", "ignoredSkills"]);
        let included = self.settings.strs(&["skills", "includeSkills"]);
        let manual = self.settings.strs(&["skills", "manualSkills"]);
        // Bare name -> (winning item id, its text), and every name taken.
        let mut bare: Vec<(String, String, String)> = Vec::new();
        let mut taken: Vec<String> = Vec::new();
        let mut seen_real: Vec<PathBuf> = Vec::new();
        for (provider, scope, root, need_desc) in roots {
            // Disabled providers and opted-out foreign homes contribute nothing; the legacy toggles above already
            // admitted Claude's and Codex's user skills.
            if self.settings.strs(&["disabledProviders"]).iter().any(|p| p == provider) {
                continue;
            }
            for dir in self.b.dirs_in(&root) {
                let f = dir.join("SKILL.md");
                if !self.is_file(&f) {
                    continue;
                }
                let real = self.fs().canonicalize(&f).unwrap_or_else(|_| f.clone());
                if seen_real.contains(&real) {
                    continue;
                }
                seen_real.push(real);
                let Ok(text) = self.fs().read_to_string(&f) else { continue };
                let fm = frontmatter::parse(&text);
                if fm.str("enabled") == Some("false") {
                    continue;
                }
                let desc = fm.str("description").unwrap_or("").to_string();
                if need_desc && desc.trim().is_empty() {
                    continue;
                }
                let raw = fm
                    .str("name")
                    .map(str::to_string)
                    .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                if raw.contains(['/', '\\']) {
                    self.b.warn(
                        "skill-name-rejected",
                        "a skill name contains a slash, which OMP reserves; it is skipped",
                        Some(&f),
                    );
                    continue;
                }
                let hidden = ["hide", "disableModelInvocation", "disable-model-invocation"]
                    .iter()
                    .any(|k| fm.str(k) == Some("true"))
                    || manual.iter().any(|g| glob(g, &raw));
                let body = text[fm.body_start..].trim().to_string();
                let layer = self.layer(provider, scope);

                // The name it gets: the bare one, or `<namespace>/<name>` when a different skill holds it.
                let mut name = raw.clone();
                let mut dup_of: Option<String> = None;
                if let Some((_, id, t)) = bare.iter().find(|(n, _, _)| *n == raw) {
                    if *t == text {
                        dup_of = Some(id.clone());
                    } else {
                        let ns = namespace(&root, provider);
                        let base = format!("{ns}/{raw}");
                        name = base.clone();
                        let mut k = 2;
                        while taken.contains(&name) {
                            name = format!("{base}~{k}");
                            k += 1;
                        }
                        self.b.warn(
                            "skill-name-collision",
                            format!(
                                "two different skills are named {}; OMP keeps the first as {} and loads this one as {}",
                                printable(&raw),
                                printable(&raw),
                                printable(&name)
                            ),
                            Some(&f),
                        );
                    }
                }
                let preview = desc.split_whitespace().collect::<Vec<_>>().join(" ");
                let listing = char_len(&name) + (char_len(&preview).min(SKILL_PREVIEW as u64));
                let (when, chars) = if hidden { (LoadWhen::OnInvoke, 0) } else { (LoadWhen::Startup, listing) };
                let mut it = self.b.item(
                    &layer,
                    &name,
                    ItemDetail::Skill(SkillAttrs { body_chars: Some(char_len(&body)) }),
                    when,
                    calc(chars),
                );
                it.origin.file = Some(self.b.display(&f));
                it.load.content_hash = Some(sha256_hex(&text));
                let off = disabled.iter().any(|d| *d == format!("skill:{raw}") || *d == format!("skill:{name}"))
                    || ignored.iter().any(|g| glob(g, &raw) || glob(g, &name))
                    || (!included.is_empty() && !included.iter().any(|g| glob(g, &name)));
                if let Some(w) = &dup_of {
                    Build::shadow(&mut it, w);
                    self.b.push(it);
                    continue;
                }
                if off {
                    Build::disable(&mut it);
                    self.b.push(it);
                    continue;
                }
                let ix = self.b.push(it);
                let id = self.b.items[ix].id.clone();
                if name == raw {
                    bare.push((raw.clone(), id, text.clone()));
                }
                taken.push(name);
            }
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // Commands and prompt templates
    // ---------------------------------------------------------------------------------------------------------

    /// File slash commands (`extensibility/slash-commands.ts`), the first of a name winning; and the prompt templates
    /// OMP expands (`config/prompt-templates.ts`: the agent directory's `prompts/**`, then the repo's
    /// `.omp/prompts/**`, so the user's wins). Nothing of either reaches the model until one is typed.
    pub(super) fn commands(&mut self) {
        let mut found: Vec<Found> = Vec::new();
        let base = |p: &Path| p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(d) = self.native_project_dir() {
            for f in self.files(&d.join("commands"), &[".md"]) {
                let n = base(&f);
                found.push(Found { provider: "native", scope: "project", path: f, name: n });
            }
        }
        for f in self.files(&self.p.agent.join("commands"), &[".md"]) {
            let n = base(&f);
            found.push(Found { provider: "native", scope: "user", path: f, name: n });
        }
        let c = |k: &str, d: bool| self.settings.bool_or(&["commands", k], d);
        let (cl_user, cl_project, oc_user, oc_project) = (
            c("enableClaudeUser", false),
            c("enableClaudeProject", true),
            c("enableOpencodeUser", false),
            c("enableOpencodeProject", true),
        );
        if cl_user || self.source_on("claude", "user") {
            for (f, _) in self.files_deep(&self.p.claude_user.join("commands"), ".md") {
                let n = base(&f);
                found.push(Found { provider: "claude", scope: "user", path: f, name: n });
            }
        }
        if cl_project {
            for (f, _) in self.files_deep(&self.p.repo.join(".claude/commands"), ".md") {
                let n = base(&f);
                found.push(Found { provider: "claude", scope: "project", path: f, name: n });
            }
        }
        for d in self.agents_walk_dirs() {
            for sub in [".agent", ".agents"] {
                for f in self.files(&d.join(sub).join("commands"), &[".md"]) {
                    let n = base(&f);
                    found.push(Found { provider: "agents", scope: "project", path: f, name: n });
                }
            }
        }
        for sub in [".agent", ".agents"] {
            for f in self.files(&self.p.home.join(sub).join("commands"), &[".md"]) {
                let n = base(&f);
                found.push(Found { provider: "agents", scope: "user", path: f, name: n });
            }
        }
        for (scope, dir) in
            [("user", self.p.codex_user.join("commands")), ("project", self.p.repo.join(".codex/commands"))]
        {
            for f in self.files(&dir, &[".md"]) {
                let n = self.fm_name(&f).unwrap_or_else(|| base(&f));
                found.push(Found { provider: "codex", scope, path: f, name: n });
            }
        }
        if oc_user && self.source_on("opencode", "user") {
            for f in self.files(&self.p.home.join(".config/opencode/commands"), &[".md"]) {
                let n = self.fm_name(&f).unwrap_or_else(|| base(&f));
                found.push(Found { provider: "opencode", scope: "user", path: f, name: n });
            }
        }
        if oc_project {
            for f in self.files(&self.p.repo.join(".opencode/commands"), &[".md"]) {
                let n = self.fm_name(&f).unwrap_or_else(|| base(&f));
                found.push(Found { provider: "opencode", scope: "project", path: f, name: n });
            }
        }
        let disabled = self.settings.strs(&["disabledExtensions"]);
        let mut idx = Vec::new();
        for f in found {
            if !self.source_on(f.provider, f.scope) {
                continue;
            }
            let off = disabled.iter().any(|d| *d == format!("slash-command:{}", f.name));
            if let Some(ix) = self.command_item(f.provider, f.scope, &f.path, &f.name, off) {
                idx.push(ix);
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |_| 0);

        // Prompt templates, the first of a name winning: user before project.
        let mut idx = Vec::new();
        let mut roots = vec![("user", self.p.agent.join("prompts"))];
        if let Some(d) = self.native_project_dir() {
            roots.push(("project", d.join("prompts")));
        }
        for (scope, dir) in roots {
            for (f, _) in self.files_deep(&dir, ".md") {
                let n = format!("prompt:{}", base(&f));
                let off = disabled.contains(&n);
                if let Some(ix) = self.command_item("native", scope, &f, &n, off) {
                    idx.push(ix);
                }
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |_| 0);
    }

    fn fm_name(&self, f: &Path) -> Option<String> {
        let text = self.fs().read_to_string(f).ok()?;
        frontmatter::parse(&text).str("name").map(str::to_string)
    }

    fn command_item(&mut self, provider: &str, scope: &str, f: &Path, name: &str, off: bool) -> Option<usize> {
        let text = self.fs().read_to_string(f).ok()?;
        let fm = frontmatter::parse(&text);
        let layer = self.layer(provider, scope);
        let body = char_len(text[fm.body_start..].trim());
        let mut it = self.b.item(&layer, name, ItemDetail::Command(CommandAttrs {}), LoadWhen::OnInvoke, calc(body));
        it.origin.file = Some(self.b.display(f));
        it.load.content_hash = Some(sha256_hex(&text));
        if off {
            Build::disable(&mut it);
        }
        Some(self.b.push(it))
    }

    // ---------------------------------------------------------------------------------------------------------
    // Task agents
    // ---------------------------------------------------------------------------------------------------------

    /// Task agents (`task/discovery.ts`): the nearest `.omp/agents` walking up to the filesystem root, then the
    /// user's `agents/` (profile-aware, not moved by `PI_CODING_AGENT_DIR`), then plugins and the bundled agents. The
    /// first of a name wins. Each needs `name` and `description`; the task tool lists name and description up front.
    pub(super) fn agents(&mut self) {
        let mut dirs: Vec<(&str, PathBuf)> = Vec::new();
        if let Some(d) = self.p.repo.ancestors().map(|d| d.join(".omp/agents")).find(|d| self.fs().read_dir(d).is_ok())
        {
            dirs.push(("project", d));
        }
        dirs.push(("user", self.p.agent_plain.join("agents")));
        let off = self.settings.strs(&["task", "disabledAgents"]);
        let mut idx = Vec::new();
        for (scope, dir) in dirs {
            if !self.source_on("native", scope) {
                continue;
            }
            let mut files = self.files(&dir, &[".md"]);
            files.sort();
            for f in files {
                let Ok(text) = self.fs().read_to_string(&f) else { continue };
                let fm = frontmatter::parse(&text);
                let (Some(name), Some(desc)) = (fm.str("name"), fm.str("description")) else {
                    self.b.warn("agent-incomplete", "a task agent without name or description is skipped", Some(&f));
                    continue;
                };
                if name == "main" || name == "sub" {
                    self.b.warn("agent-name-reserved", "main and sub are reserved agent names", Some(&f));
                    continue;
                }
                let tools = match fm.fields.get("tools").map(FmValue::as_list) {
                    Some(t) if !t.is_empty() => ToolAccess::Only(t),
                    _ => ToolAccess::All,
                };
                let model = fm.fields.get("model").map(FmValue::as_list).and_then(|m| m.first().cloned());
                let layer = self.layer("native", scope);
                let chars = char_len(name) + char_len(desc);
                let mut it = self.b.item(
                    &layer,
                    name,
                    ItemDetail::Agent(AgentAttrs { tools, model }),
                    LoadWhen::Startup,
                    calc(chars),
                );
                it.origin.file = Some(self.b.display(&f));
                it.load.content_hash = Some(sha256_hex(&text));
                if off.iter().any(|o| o == name) {
                    Build::disable(&mut it);
                }
                idx.push(self.b.push(it));
            }
        }
        self.b.resolve_clashes(&idx, |i| i.name.clone(), |_| 0);
    }

    // ---------------------------------------------------------------------------------------------------------
    // MCP servers
    // ---------------------------------------------------------------------------------------------------------

    /// MCP servers from every provider, in order; the first definition of a name wins, and a disabled one still
    /// claims it (`mcp/config.ts`). `mcp.enableProjectConfig: false` drops every project-level server first;
    /// `disabledServers` and `enabledServers` are top-level lists in the agent directory's `mcp.json`.
    pub(super) fn mcp(&mut self) {
        let r = self.p.repo.clone();
        let h = self.p.home.clone();
        let a = self.p.agent.clone();
        // (provider, scope, file, how to read it)
        let mut files: Vec<(&'static str, &'static str, PathBuf, Shape)> = vec![
            ("native", "project", r.join(".omp/mcp.json"), Shape::McpServers),
            ("native", "project", r.join(".omp/.mcp.json"), Shape::McpServers),
            ("native", "user", a.join("mcp.json"), Shape::McpServers),
            ("native", "user", a.join(".mcp.json"), Shape::McpServers),
        ];
        // Claude: the first of each pair that has servers.
        let first_with =
            |s: &Self, list: &[PathBuf]| list.iter().find(|f| !s.servers_of(f, Shape::McpServers).is_empty()).cloned();
        if let Some(f) = first_with(self, &[r.join(".claude/.mcp.json"), r.join(".claude/mcp.json")]) {
            files.push(("claude", "project", f, Shape::McpServers));
        }
        if let Some(f) = first_with(self, &[self.p.claude_json.clone(), self.p.claude_user.join("mcp.json")]) {
            files.push(("claude", "user", f, Shape::McpServers));
        }
        files.push(("codex", "project", r.join(".codex/config.toml"), Shape::CodexToml));
        files.push(("codex", "user", self.p.codex_user.join("config.toml"), Shape::CodexToml));
        files.push(("gemini", "project", r.join(".gemini/settings.json"), Shape::McpServers));
        files.push(("gemini", "user", h.join(".gemini/settings.json"), Shape::McpServers));
        for f in [h.join(".config/opencode/opencode.json"), h.join(".config/opencode/opencode.jsonc")] {
            files.push(("opencode", "user", f, Shape::OpenCode));
        }
        for f in [
            r.join("opencode.json"),
            r.join("opencode.jsonc"),
            r.join(".opencode/opencode.json"),
            r.join(".opencode/opencode.jsonc"),
        ] {
            files.push(("opencode", "project", f, Shape::OpenCode));
        }
        files.push(("cursor", "project", r.join(".cursor/mcp.json"), Shape::McpServers));
        files.push(("cursor", "user", h.join(".cursor/mcp.json"), Shape::McpServers));
        files.push(("windsurf", "project", r.join(".windsurf/mcp_config.json"), Shape::McpServers));
        files.push(("windsurf", "user", h.join(".codeium/windsurf/mcp_config.json"), Shape::McpServers));
        files.push(("vscode", "project", r.join(".vscode/mcp.json"), Shape::VsCode));
        files.push(("mcp-json", "project", r.join("mcp.json"), Shape::McpServers));
        files.push(("mcp-json", "project", r.join(".mcp.json"), Shape::McpServers));

        let project_on = self.settings.bool_or(&["mcp", "enableProjectConfig"], true);
        let control =
            if self.fs().exists(&a.join("mcp.json")) { self.b.json(&a.join("mcp.json")).object() } else { Map::new() };
        let list = |k: &str| -> Vec<String> {
            control
                .get(k)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        };
        let (off_list, on_list) = (list("disabledServers"), list("enabledServers"));
        let disabled_ext = self.settings.strs(&["disabledExtensions"]);

        let mut idx = Vec::new();
        for (provider, scope, f, shape) in files {
            if !self.source_on(provider, scope) || (scope == "project" && !project_on) || !self.is_file(&f) {
                continue;
            }
            if shape == Shape::VsCode {
                let o = self.b.jsonc(&f).object();
                if o.contains_key("servers") && o.get("mcp").is_none_or(|m| m.get("servers").is_none()) {
                    self.b.warn(
                        "vscode-mcp-not-read",
                        ".vscode/mcp.json lists servers at the top level; OMP reads only mcp.servers there, so it loads none of them",
                        Some(&f),
                    );
                }
            }
            for (name, cfg) in self.servers_of(&f, shape) {
                let ix = self.mcp_item(provider, scope, &f, &name, &cfg, shape);
                let off = (cfg.get("enabled").and_then(Value::as_bool) == Some(false) && !on_list.contains(&name))
                    || off_list.contains(&name)
                    || disabled_ext.iter().any(|d| *d == format!("mcp:{name}"));
                if off {
                    if let ItemDetail::Mcp(m) = &mut self.b.items[ix].detail {
                        m.state = McpState::Disabled;
                    }
                    // Off, but still the first of its name: later definitions stay shadowed.
                    self.b.items[ix].effective = false;
                    self.b.items[ix].load.when = LoadWhen::Never;
                }
                idx.push(ix);
            }
        }
        // The first definition of a name wins, disabled or not.
        let mut first: Vec<(String, String)> = Vec::new();
        for &i in &idx {
            let name = self.b.items[i].name.clone();
            match first.iter().find(|(n, _)| *n == name) {
                Some((_, w)) => {
                    let w = w.clone();
                    if self.b.items[i].effective || self.b.items[i].shadowed_by.is_none() {
                        Build::shadow(&mut self.b.items[i], &w);
                    }
                }
                None => first.push((name, self.b.items[i].id.clone())),
            }
        }
    }

    /// The servers a file declares, by its shape; names and the fields rigswig needs, never secrets.
    fn servers_of(&self, f: &Path, shape: Shape) -> Vec<(String, Value)> {
        let read = |f: &Path| -> Map<String, Value> {
            match shape {
                Shape::CodexToml => crate::read::json::read_toml(self.fs(), f).object(),
                _ => crate::read::json::read_jsonc(self.fs(), f).object(),
            }
        };
        if !self.is_file(f) {
            return Vec::new();
        }
        let o = read(f);
        let map = match shape {
            Shape::McpServers => o.get("mcpServers").and_then(Value::as_object).cloned(),
            Shape::CodexToml => o.get("mcp_servers").and_then(Value::as_object).cloned(),
            Shape::OpenCode => o.get("mcp").and_then(Value::as_object).cloned(),
            Shape::VsCode => o.get("mcp").and_then(|m| m.get("servers")).and_then(Value::as_object).cloned(),
        };
        map.unwrap_or_default().into_iter().filter(|(_, v)| v.is_object()).collect()
    }

    fn mcp_item(&mut self, provider: &str, scope: &str, f: &Path, name: &str, cfg: &Value, shape: Shape) -> usize {
        let kind = match shape {
            Shape::VsCode => cfg.get("transport"),
            _ => cfg.get("type"),
        }
        .and_then(Value::as_str);
        let url = cfg.get("url").and_then(Value::as_str);
        let command = match cfg.get("command") {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(a)) => a.first().and_then(Value::as_str).map(str::to_string),
            _ => None,
        };
        let transport = match (kind, url, &command) {
            (Some("sse"), _, _) => McpTransport::Sse,
            (Some("stdio" | "local"), _, _) => McpTransport::Stdio,
            (Some("http" | "remote" | "streamable-http"), _, _) => McpTransport::Http,
            (_, Some(_), _) => McpTransport::Http,
            (_, _, Some(_)) => McpTransport::Stdio,
            _ => McpTransport::Unknown,
        };
        let target = match transport {
            McpTransport::Http | McpTransport::Sse => url.and_then(url_origin),
            McpTransport::Stdio => {
                command.as_deref().and_then(|c| Path::new(c).file_name()).map(|n| n.to_string_lossy().into_owned())
            }
            McpTransport::Unknown => None,
        };
        let layer = self.layer(provider, scope);
        let mut it = self.b.item(
            &layer,
            name,
            ItemDetail::Mcp(McpAttrs { transport, target, state: McpState::Active }),
            LoadWhen::Startup,
            calc(0),
        );
        it.origin.file = Some(self.b.display(f));
        self.b.push(it)
    }

    // ---------------------------------------------------------------------------------------------------------
    // Hooks
    // ---------------------------------------------------------------------------------------------------------

    /// Hook files: `hooks/pre/*` and `hooks/post/*` under the repo's `.omp` and `.claude` and the user's homes, and
    /// Codex's `hooks/(pre|post)-<tool>.(ts|js)`. The tool is the file name without its extension. Only `.ts` and
    /// `.js` files run (as extension modules); others are found and never run. Claude Code's settings.json hooks are
    /// not read by OMP at all.
    pub(super) fn hooks(&mut self) {
        let mut dirs: Vec<(&'static str, &'static str, PathBuf)> = Vec::new();
        if let Some(d) = self.native_project_dir() {
            dirs.push(("native", "project", d.join("hooks")));
        }
        dirs.push(("native", "user", self.p.agent.join("hooks")));
        dirs.push(("claude", "user", self.p.claude_user.join("hooks")));
        dirs.push(("claude", "project", self.p.repo.join(".claude/hooks")));
        let mut idx = Vec::new();
        for (provider, scope, dir) in dirs {
            if !self.source_on(provider, scope) {
                continue;
            }
            for (phase, event) in [("pre", "before-tool"), ("post", "after-tool")] {
                let d = dir.join(phase);
                for e in self.fs().read_dir(&d).unwrap_or_default() {
                    if e.name.starts_with('.') || e.kind == EntryKind::Dir {
                        continue;
                    }
                    let tool =
                        Path::new(&e.name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    idx.push(self.hook_item(provider, scope, &d.join(&e.name), phase, event, &tool));
                }
            }
        }
        for (scope, dir) in [("user", self.p.codex_user.join("hooks")), ("project", self.p.repo.join(".codex/hooks"))] {
            if !self.source_on("codex", scope) {
                continue;
            }
            for f in self.files(&dir, &[".ts", ".js"]) {
                let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let Some((phase, tool)) = stem.split_once('-') else { continue };
                let event = match phase {
                    "pre" => "before-tool",
                    "post" => "after-tool",
                    _ => continue,
                };
                let (phase, tool) = (phase.to_string(), tool.to_string());
                idx.push(self.hook_item("codex", scope, &f, &phase, event, &tool));
            }
        }
        let disabled = self.settings.strs(&["disabledExtensions"]);
        for &i in &idx {
            if disabled.iter().any(|d| d.starts_with("hook:") && d.ends_with(&format!(":{}", self.b.items[i].name))) {
                Build::disable(&mut self.b.items[i]);
            }
        }
        if self.fs().exists(&self.p.repo.join(".claude/settings.json")) {
            let o = self.b.jsonc(&self.p.repo.join(".claude/settings.json")).object();
            if o.contains_key("hooks") {
                self.b.warn(
                    "claude-hooks-ignored",
                    "the repo's .claude/settings.json declares hooks; OMP does not run Claude Code's hooks",
                    None,
                );
            }
        }
    }

    fn hook_item(&mut self, provider: &str, scope: &str, f: &Path, phase: &str, event: &str, tool: &str) -> usize {
        let runs = f.extension().is_some_and(|e| e == "ts" || e == "js");
        let layer = self.layer(provider, scope);
        let file = f.file_name().map(|n| n.to_string_lossy().into_owned());
        let attrs = HookAttrs {
            event: event.into(),
            native_event: phase.into(),
            matcher: Some(printable(tool)),
            handler: HookHandler::Command,
            script: file.clone(),
        };
        let name = format!("{phase}:{}", file.unwrap_or_default());
        let mut it = self.b.item(&layer, &name, ItemDetail::Hook(attrs), LoadWhen::Never, calc(0));
        it.origin.file = Some(self.b.display(f));
        if !runs {
            Build::disable(&mut it);
            self.b.warn("hook-not-run", "a hook file that is not .ts or .js: OMP finds it but never runs it", Some(f));
        }
        self.b.push(it)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `{"mcpServers": {...}}`
    McpServers,
    /// Codex's `[mcp_servers.<name>]`
    CodexToml,
    /// OpenCode's `{"mcp": {...}}` (`type: local` or `remote`)
    OpenCode,
    /// VS Code's file, read by OMP from `mcp.servers`
    VsCode,
}

/// A colliding skill's namespace: the path segment before `skills/`, or the provider id when that segment is a
/// dot-folder (`.claude/skills/x` -> `claude/x`; `~/.omp/agent/skills/x` -> `agent/x`).
fn namespace(root: &Path, provider: &str) -> String {
    let seg = root.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if seg.is_empty() || seg.starts_with('.') { provider.to_string() } else { seg }
}

/// Glob match for skill filters: `*` within one segment, `**` across segments, `?` one character.
fn glob(pat: &str, s: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some('*') if p.get(1) == Some(&'*') => (0..=s.len()).any(|i| go(&p[2..], &s[i..])),
            Some('*') => (0..=s.len()).take_while(|&i| i == 0 || s[i - 1] != '/').any(|i| go(&p[1..], &s[i..])),
            Some('?') => !s.is_empty() && go(&p[1..], &s[1..]),
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = s.chars().collect();
    go(&p, &s)
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("third-party-*", "third-party-pdf"));
        assert!(!glob("*", "a/b"));
        assert!(glob("**", "a/b"));
        assert!(glob("second/*", "second/calendar"));
        assert!(glob("pd?", "pdf"));
    }
}
