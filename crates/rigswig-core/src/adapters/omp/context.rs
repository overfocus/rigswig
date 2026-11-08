//! Context files and sticky rules (docs/context-files.md; capability/context-file.ts, discovery/helpers.ts
//! `loadStandaloneContextFiles`, discovery/builtin.ts `findNearestProjectConfigDir`).
//!
//! Every provider offers candidates; then OMP keeps one user file across all providers and one project file per
//! directory depth, the higher-priority provider winning each slot. Extra names (`contextFiles.extra`) load beside
//! the slot winner. A farther file whose paragraphs a closer file contains is dropped. Order: the user file, then
//! the farthest project file down to the closest.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::adapters::build::{Build, calc, char_len};
use crate::model::*;
use crate::read::imports::imports_of;
use crate::read::redact::sha256_hex;

use super::Run;

/// How many `@` hops an import chain follows (docs/context-files.md: five).
const IMPORT_MAX_HOPS: usize = 5;

#[derive(Clone)]
struct Candidate {
    provider: &'static str,
    path: PathBuf,
    text: String,
    /// `None` for a user file.
    depth: Option<usize>,
    additive: bool,
}

impl Run<'_, '_> {
    pub(super) fn context_files(&mut self) {
        let mut cands: Vec<Candidate> = Vec::new();
        self.user_context(&mut cands);
        self.project_context(&mut cands);
        self.extra_context(&mut cands);

        // Disabled single files (`disabledExtensions: context-file:<level>:<basename>`) drop before the slots are
        // decided, so the file they would have shadowed loads in their place.
        let disabled = self.settings.strs(&["disabledExtensions"]);
        let mut kept: Vec<Candidate> = Vec::new();
        for c in cands {
            let level = if c.depth.is_some() { "project" } else { "user" };
            let base = c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if disabled.iter().any(|d| *d == format!("context-file:{level}:{base}")) {
                let ix = self.push_context(&c, LoadWhen::Never);
                Build::disable(&mut self.b.items[ix]);
                continue;
            }
            kept.push(c);
        }

        // The slots: one user file, one project file per depth; the higher-priority provider wins, then the earlier
        // registered one (the order `cands` was filled in).
        let slot = |c: &Candidate| -> Option<String> {
            if c.additive {
                return None;
            }
            Some(c.depth.map_or_else(|| "user".to_string(), |d| format!("project:{d}")))
        };
        let mut winners: Vec<usize> = Vec::new();
        let mut losers: Vec<(usize, usize)> = Vec::new();
        for (i, c) in kept.iter().enumerate() {
            let Some(s) = slot(c) else {
                winners.push(i);
                continue;
            };
            let rival = winners.iter().copied().find(|&w| slot(&kept[w]).as_deref() == Some(s.as_str()));
            match rival {
                None => winners.push(i),
                Some(w) if super::priority(c.provider) > super::priority(kept[w].provider) => {
                    winners.retain(|&x| x != w);
                    winners.push(i);
                    losers.push((w, i));
                }
                Some(w) => losers.push((i, w)),
            }
        }
        // A loser that lost first and then saw its winner lose too is shadowed by the final winner.
        let final_of = |mut w: usize| {
            while let Some(&(_, next)) = losers.iter().find(|(l, _)| *l == w) {
                w = next;
            }
            w
        };

        // Load order: the user file first, then farthest to closest (extras after the slot winner at their depth).
        winners.sort_by_key(|&i| {
            let c = &kept[i];
            (c.depth.is_some(), std::cmp::Reverse(c.depth.unwrap_or(0)), c.additive)
        });

        // Containment: drop a farther file whose paragraphs a later (closer) file holds contiguously.
        let mut dropped: Vec<(usize, usize)> = Vec::new();
        for (a, &i) in winners.iter().enumerate() {
            let p = paragraphs(&kept[i].text);
            if p.is_empty() {
                continue;
            }
            if let Some(&j) = winners[a + 1..].iter().find(|&&j| contains_run(&paragraphs(&kept[j].text), &p)) {
                dropped.push((i, j));
            }
        }

        let mut ids: Vec<Option<String>> = vec![None; kept.len()];
        for &i in &winners {
            if dropped.iter().any(|(d, _)| *d == i) {
                continue;
            }
            let ix = self.push_context(&kept[i], LoadWhen::Startup);
            ids[i] = Some(self.b.items[ix].id.clone());
            let mut seen = BTreeSet::new();
            let id = self.b.items[ix].id.clone();
            let layer = self.b.items[ix].origin.layer.clone();
            self.imports(&kept[i].path, &kept[i].text, &layer, &id, 1, &mut seen);
        }
        for &(i, by) in &dropped {
            let ix = self.push_context(&kept[i], LoadWhen::Never);
            if let Some(w) = ids[by].clone() {
                Build::shadow(&mut self.b.items[ix], &w);
            } else {
                Build::disable(&mut self.b.items[ix]);
            }
            self.b.warn(
                "context-contained",
                "a context file's text is wholly repeated in a closer one, so OMP drops the farther copy",
                Some(&kept[i].path.clone()),
            );
        }
        for &(i, _) in &losers {
            let w = final_of(i);
            let ix = self.push_context(&kept[i], LoadWhen::Never);
            match ids[w].clone() {
                Some(id) => Build::shadow(&mut self.b.items[ix], &id),
                None => Build::disable(&mut self.b.items[ix]),
            }
        }
    }

    fn push_context(&mut self, c: &Candidate, when: LoadWhen) -> usize {
        let scope = if c.depth.is_some() { "project" } else { "user" };
        let layer = self.layer(c.provider, scope);
        let shown = self.b.display(&c.path);
        let mut it = self.b.item(
            &layer,
            &shown,
            ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to: Vec::new() }),
            when,
            calc(char_len(&c.text)),
        );
        it.origin.file = Some(shown);
        it.load.content_hash = Some(sha256_hex(&c.text));
        self.b.push(it)
    }

    /// `@path` imports, expanded inline in OMP; listed here as items of their own, imported by the file. Relative
    /// paths resolve from the importing file; `~` is the home. rigswig follows them only into the repo and the home.
    fn imports(&mut self, file: &Path, text: &str, layer: &str, by: &str, hop: usize, seen: &mut BTreeSet<PathBuf>) {
        if hop > IMPORT_MAX_HOPS {
            return;
        }
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        for imp in imports_of(text) {
            let imp = imp.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'']);
            let target = if imp == "~" {
                self.p.home.clone()
            } else if let Some(rest) = imp.strip_prefix("~/") {
                self.p.home.join(rest)
            } else if Path::new(imp).is_absolute() {
                PathBuf::from(imp)
            } else {
                dir.join(imp)
            };
            let Ok(real) = self.fs().canonicalize(&target) else { continue };
            if !crate::fs::is_within(&real, &self.p.repo) && !crate::fs::is_within(&real, &self.p.home) {
                self.b.warn(
                    "import-outside",
                    "an @import outside the repo and the home folder was not followed",
                    Some(file),
                );
                continue;
            }
            if !seen.insert(real.clone()) {
                continue;
            }
            let Ok(body) = self.fs().read_to_string(&real) else { continue };
            let shown = self.b.display(&target);
            let mut it = self.b.item(
                layer,
                &shown,
                ItemDetail::Instruction(InstructionAttrs { imported_by: Some(by.to_string()), applies_to: Vec::new() }),
                LoadWhen::Startup,
                calc(char_len(&body)),
            );
            it.origin.file = Some(shown);
            it.load.content_hash = Some(sha256_hex(&body));
            let ix = self.b.push(it);
            let id = self.b.items[ix].id.clone();
            self.imports(&real, &body, layer, &id, hop + 1, seen);
        }
    }

    /// A file's text when it exists and is not empty (empty files contribute nothing and claim no slot).
    fn nonempty(&self, f: &Path) -> Option<String> {
        if !matches!(self.fs().kind(f), Ok(crate::fs::EntryKind::File)) {
            return None;
        }
        self.fs().read_to_string(f).ok().filter(|t| !t.is_empty())
    }

    /// User-level candidates, in registration order within a priority: native, then the default-on `.agent`/`.agents`
    /// homes, then the foreign homes the user opted into (`enabledProviders`).
    fn user_context(&mut self, out: &mut Vec<Candidate>) {
        let h = self.p.home.clone();
        let list: Vec<(&'static str, PathBuf)> = vec![
            ("native", self.p.agent.join("AGENTS.md")),
            ("claude", self.p.claude_user.join("CLAUDE.md")),
            ("agents", h.join(".agent/AGENTS.md")),
            ("agents", h.join(".agents/AGENTS.md")),
            ("codex", self.p.codex_user.join("AGENTS.md")),
            ("gemini", h.join(".gemini/GEMINI.md")),
            ("opencode", h.join(".config/opencode/AGENTS.md")),
            ("github", self.p.copilot_user.join("copilot-instructions.md")),
        ];
        for (provider, f) in list {
            if !self.source_on(provider, "user") {
                continue;
            }
            if let Some(text) = self.nonempty(&f) {
                out.push(Candidate { provider, path: f, text, depth: None, additive: false });
            }
        }
    }

    /// Project-level candidates: native's nearest non-empty `.omp/`; the cwd-only `.claude/`, `.gemini/` and
    /// `.github/` files; `.agent(s)/AGENTS.md` up the project walk; and the standalone `AGENTS.md` / `CLAUDE.md` walk.
    fn project_context(&mut self, out: &mut Vec<Candidate>) {
        if self.source_on("native", "project") {
            if let Some((dir, depth)) = self.nearest_omp() {
                if let Some(text) = self.nonempty(&dir.join("AGENTS.md")) {
                    out.push(Candidate {
                        provider: "native",
                        path: dir.join("AGENTS.md"),
                        text,
                        depth: Some(depth),
                        additive: false,
                    });
                }
            }
        }
        let cwd = self.p.repo.clone();
        for (provider, f) in [
            ("claude", cwd.join(".claude/CLAUDE.md")),
            ("gemini", cwd.join(".gemini/GEMINI.md")),
            ("github", cwd.join(".github/copilot-instructions.md")),
        ] {
            if self.source_on(provider, "project") {
                if let Some(text) = self.nonempty(&f) {
                    out.push(Candidate { provider, path: f, text, depth: Some(0), additive: false });
                }
            }
        }
        if self.source_on("agents", "project") {
            for (dir, depth) in self.project_ancestors() {
                for sub in [".agent", ".agents"] {
                    let f = dir.join(sub).join("AGENTS.md");
                    if let Some(text) = self.nonempty(&f) {
                        out.push(Candidate { provider: "agents", path: f, text, depth: Some(depth), additive: false });
                    }
                }
            }
        }
        for (provider, name) in [("agents-md", "AGENTS.md"), ("claude-md", "CLAUDE.md")] {
            if !self.source_on(provider, "project") {
                continue;
            }
            for (dir, depth) in self.standalone_walk() {
                let f = dir.join(name);
                if let Some(text) = self.nonempty(&f) {
                    out.push(Candidate { provider, path: f, text, depth: Some(depth), additive: false });
                }
            }
        }
    }

    /// `contextFiles.extra`: each name on the standalone walk, plus the agent directory unless the list comes from
    /// the project's config. Additive: they take no slot.
    fn extra_context(&mut self, out: &mut Vec<Candidate>) {
        if !self.source_on("custom-context", "project") {
            return;
        }
        let Some((names, from)) = self.settings.strs_with_layer(&["contextFiles", "extra"]) else { return };
        let builtin = ["AGENTS.md", "CLAUDE.md", "GEMINI.md", "copilot-instructions.md"];
        let names: Vec<String> =
            names.into_iter().filter(|n| !n.contains(['/', '\\']) && !builtin.contains(&n.as_str())).collect();
        for n in &names {
            if from != "project" {
                let f = self.p.agent.join(n);
                if let Some(text) = self.nonempty(&f) {
                    out.push(Candidate { provider: "custom-context", path: f, text, depth: None, additive: true });
                }
            }
            for (dir, depth) in self.standalone_walk() {
                let f = dir.join(n);
                if let Some(text) = self.nonempty(&f) {
                    out.push(Candidate {
                        provider: "custom-context",
                        path: f,
                        text,
                        depth: Some(depth),
                        additive: true,
                    });
                }
            }
        }
    }

    /// The standalone walk (`loadStandaloneContextFiles`): from the cwd up to the repo root; past it to (not
    /// including) the home when the repo is nested under the home; without a repo, up to and including the home for a
    /// cwd under it, else to the filesystem root. Folders whose name starts with `.` hold no standalone file.
    pub(super) fn standalone_walk(&self) -> Vec<(PathBuf, usize)> {
        let home = self.p.home.clone();
        let cwd = self.p.repo.clone();
        let root = self.repo_root.clone();
        let cwd_under_home = cwd.starts_with(&home);
        let repo_is_home = root.as_ref().is_some_and(|r| *r == home);
        let repo_under_home = root.as_ref().is_some_and(|r| r.starts_with(&home) && *r != home);
        let scan_to_home = root.is_some() && cwd_under_home && repo_under_home;
        let boundary: Option<PathBuf> = if scan_to_home {
            Some(home.clone())
        } else {
            root.clone().or_else(|| cwd_under_home.then(|| home.clone()))
        };
        let include_boundary = match &root {
            None => cwd_under_home,
            Some(_) => boundary.as_ref() != Some(&home) || repo_is_home,
        };
        let mut out = Vec::new();
        for (depth, d) in cwd.ancestors().enumerate() {
            let at_boundary = boundary.as_deref() == Some(d);
            let at_home = scan_to_home && d == home;
            let dotted = d.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if !(at_home || (at_boundary && !include_boundary) || dotted) {
                out.push((d.to_path_buf(), depth));
            }
            if at_boundary {
                break;
            }
        }
        out
    }

    /// The project walk for config folders (`getProjectAncestorDirs`): the cwd up to the repo root (or the home),
    /// skipping the home itself, which is the user's config root and never a project.
    pub(super) fn project_ancestors(&self) -> Vec<(PathBuf, usize)> {
        let stop = self.repo_root.clone().unwrap_or_else(|| self.p.home.clone());
        let mut out = Vec::new();
        for (depth, d) in self.p.repo.ancestors().enumerate() {
            if d != self.p.home {
                out.push((d.to_path_buf(), depth));
            }
            if d == stop {
                break;
            }
        }
        out
    }

    /// The nearest non-empty `.omp/` on the project walk, and its depth: it alone holds native project `AGENTS.md`,
    /// `RULES.md` and `SYSTEM.md`, even when it lacks the file.
    pub(super) fn nearest_omp(&self) -> Option<(PathBuf, usize)> {
        self.project_ancestors().into_iter().find_map(|(d, depth)| {
            let o = d.join(".omp");
            let non_empty = self.fs().read_dir(&o).is_ok_and(|e| !e.is_empty());
            non_empty.then_some((o, depth))
        })
    }

    /// `RULES.md` at the agent directory and the nearest non-empty project `.omp/`: always-apply rules whose whole
    /// body rides in every request (rule names `RULES` and `RULES@project`).
    pub(super) fn sticky_rules(&mut self) {
        let mut files: Vec<(&str, PathBuf)> = Vec::new();
        if self.source_on("native", "project") {
            if let Some((dir, _)) = self.nearest_omp() {
                files.push(("project", dir.join("RULES.md")));
            }
        }
        if self.source_on("native", "user") {
            files.push(("user", self.p.agent.join("RULES.md")));
        }
        for (scope, f) in files {
            let Some(text) = self.nonempty(&f) else { continue };
            let layer = self.layer("native", scope);
            let shown = self.b.display(&f);
            let mut it = self.b.item(
                &layer,
                &shown,
                ItemDetail::Instruction(InstructionAttrs { imported_by: None, applies_to: Vec::new() }),
                LoadWhen::Startup,
                calc(char_len(&text)),
            );
            it.origin.file = Some(shown);
            it.load.content_hash = Some(sha256_hex(&text));
            self.b.push(it);
        }
    }
}

/// A file's paragraphs, normalized: split at blank lines, each trimmed with its whitespace runs collapsed.
fn paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n").map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|p| !p.is_empty()).collect()
}

/// Whether `hay` holds `needle` as one contiguous run.
fn contains_run(hay: &[String], needle: &[String]) -> bool {
    !needle.is_empty() && hay.len() >= needle.len() && hay.windows(needle.len()).any(|w| w == needle)
}
