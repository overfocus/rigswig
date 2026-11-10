//! The DeepSeek Harness adapter. Every rule it models is in `docs/adapters/dsh.md`, with the sources it comes
//! from (all read 2026-11-06).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::fs::EntryKind;
use crate::model::*;
use crate::read::frontmatter;
use crate::read::redact::{printable, sha256_hex};

pub const ID: &str = "dsh";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release the sources this adapter follows describe (v0.1.0-rc, developer preview; read 2026-11-06).
const TOOL_VERSION: &str = "0.1.0-rc";
/// The base candidate files the `agent-instructions` plugin reads, in its configured order.
const BASE_CANDIDATES: &[&str] = &["AGENTS.md", "CLAUDE.md"];
/// The additive local overlays, which load after the base files of the same directory.
const OVERLAY_CANDIDATES: &[&str] = &["AGENTS.local.md", "CLAUDE.local.md"];
/// Heavy directories the lazy walk does not enter.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "vendor", "build", "coverage", ".venv", "venv"];
/// The skill roots' documented ranks, folded to a same-name priority (lower wins).
const RANK_PROJECT_DSH: u32 = 0;
const RANK_PROJECT_AGENTS: u32 = 1;
const RANK_USER_DSH: u32 = 2;
const RANK_USER_AGENTS: u32 = 3;

struct Paths {
    home: PathBuf,
    agents_home: PathBuf,
    project: PathBuf,
    repo: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        Self {
            home: ctx.var("DSH_HOME").map_or_else(|| ctx.home.join(".dsh"), PathBuf::from),
            agents_home: ctx.home.join(".agents/skills"),
            project: ctx.repo.join(".dsh"),
            repo: ctx.repo.clone(),
        }
    }
}

pub struct Dsh;

impl Adapter for Dsh {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["DSH_HOME"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.project, ".dsh/", 100),
            (p.repo.join(".agents/skills"), ".agents/skills/", 60),
            (p.home, "DeepSeek Harness folder", 45),
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
            "the harness home ($DSH_HOME, default ~/.dsh)",
            Some(b.display(&p.home)),
            has(&p.home.join("AGENTS.md")) || has(&p.home.join("skills")),
            LayerMode::Default,
        );
        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's .dsh directory and its AGENTS.md-compatible chain",
            Some(b.display(&p.project)),
            has(&p.project)
                || !b.files_in(&p.project.join("skills"), ".md").is_empty()
                || !b.dirs_in(&p.project.join("skills")).is_empty()
                || !b.dirs_in(&p.repo.join(".agents/skills")).is_empty()
                || has(&p.repo.join("AGENTS.md"))
                || has(&p.repo.join("CLAUDE.md")),
            LayerMode::Default,
        );

        let mut run = Run { b: &mut b, p };
        run.instructions();
        run.skills();
        run.b.link_packages();

        run.b.not_measured.push(NotMeasured {
            what: "the system prompt, the built-in tools and the model".into(),
            why: "they come with DeepSeek Harness, not from any file".into(),
        });
        run.b.not_measured_common();

        for note in [
            "the Cordis composition: profiles, bundles and patches mount every capability (the persona, tools, the mcp-client, hooks, approvals, permission presets and the sandbox), so settings-level facts live in composition rows, and rigswig reads the loader's documented behaviour rather than a composed tree",
            "each instruction chain's byte budget (maxBytes, 65,536 per chain in the dsh-base default): broader files are omitted before the most specific file is truncated at render time, so the measured characters are the files', not the rendered message's",
            "customSkillDirs and bundled-skill roots (composition-configured skill sources), the packaged providers (skill-badge, skill-office), and remote providers, which need a filesystem provider present",
            "which skills a session's scope chain and invocation policy (model-invocable, human-invocable) expose",
            "skills' references/ and scripts/ siblings, which the local provider locates but the model catalog never shows",
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
}

impl Run<'_, '_> {
    fn fs(&self) -> &dyn crate::fs::Fs {
        self.b.fs()
    }

    /// The instruction chain: the user-global file every session, then every existing candidate file from the
    /// project root down, in broad-to-specific order. Within one directory the base candidates load first and
    /// the overlays after; a candidate whose trimmed content equals an earlier one in the same directory
    /// renders once (the earlier wins), and a warning says the double-up. Below the root a file joins when the
    /// agent first touches its directory, so it is listed on-invoke.
    fn instructions(&mut self) {
        let mut seen = BTreeSet::new();
        let global = self.p.home.join("AGENTS.md");
        if let Some(text) = self.doc_text(&global) {
            let name = self.b.display(&global);
            let mut it = self.b.item(
                "user",
                &name,
                ItemDetail::Instruction(InstructionAttrs::default()),
                LoadWhen::Startup,
                calc(char_len(text.trim())),
            );
            it.origin.file = Some(name.clone());
            it.load.content_hash = Some(sha256_hex(&text));
            self.b.push(it);
            seen.insert(global);
        }

        let repo = self.p.repo.clone();
        let mut bodies = Vec::new();
        self.chain_dir(&repo, LoadWhen::Startup, &mut seen, &mut bodies);
        self.walk_chain(&repo, 0, &mut seen);
    }

    /// One directory's candidates; `bodies` carries this directory's trimmed texts for the per-directory dedup.
    fn chain_dir(&mut self, dir: &Path, when: LoadWhen, seen: &mut BTreeSet<PathBuf>, bodies: &mut Vec<String>) {
        for names in [BASE_CANDIDATES, OVERLAY_CANDIDATES] {
            for name in names {
                let f = dir.join(name);
                let Some(text) = self.doc_text(&f) else { continue };
                let trimmed = text.trim().to_string();
                if bodies.contains(&trimmed) {
                    self.b.warn(
                        "instruction-duplicate",
                        "this file's content duplicates an earlier candidate in the same directory, and DeepSeek Harness renders it once",
                        Some(f.as_path()),
                    );
                    continue;
                }
                let name = self.b.display(&f);
                let mut it = self.b.item(
                    "project",
                    &name,
                    ItemDetail::Instruction(InstructionAttrs::default()),
                    when,
                    calc(char_len(&trimmed)),
                );
                it.origin.file = Some(name.clone());
                it.load.content_hash = Some(sha256_hex(&text));
                self.b.push(it);
                bodies.push(trimmed);
                seen.insert(f);
            }
        }
    }

    /// Subdirectories join the chain lazily (the agent discovers them on file touch).
    fn walk_chain(&mut self, dir: &Path, depth: usize, seen: &mut BTreeSet<PathBuf>) {
        if depth > 6 {
            return;
        }
        for e in self.fs().read_dir(dir).unwrap_or_default() {
            if e.kind != EntryKind::Dir || e.name.starts_with('.') || SKIP_DIRS.contains(&e.name.as_str()) {
                continue;
            }
            let child = dir.join(&e.name);
            let mut bodies = Vec::new();
            self.chain_dir(&child, LoadWhen::OnInvoke, seen, &mut bodies);
            self.walk_chain(&child, depth + 1, seen);
        }
    }

    /// A file's text, if it exists and is not empty.
    fn doc_text(&mut self, file: &Path) -> Option<String> {
        let text = self.b.text(file).ok()?;
        (!text.trim().is_empty()).then_some(text)
    }

    /// Skills from the local discovery roots in the documented rank order; a duplicate name wins outright at
    /// the higher rank. The shipped layouts are a directory bundle (`<name>/SKILL.md`, no recursion beneath
    /// the name) and a flat `<name>.md`.
    fn skills(&mut self) {
        // Rank 300 (Config.customSkillDirs) and 600 (bundled) are composition-configured; not modelled.
        let roots: [(&str, PathBuf, u32); 4] = [
            ("project", self.p.project.join("skills"), RANK_PROJECT_DSH),
            ("project", self.p.repo.join(".agents/skills"), RANK_PROJECT_AGENTS),
            ("user", self.p.home.join("skills"), RANK_USER_DSH),
            // Rank 500: <agentsHome>/skills; its default location is inferred (~/.agents).
            ("user", self.p.agents_home.clone(), RANK_USER_AGENTS),
        ];
        let mut idx = Vec::new();
        // Same-name priority, keyed by each item's origin file: the nearest layer wins a duplicate name
        // outright, and the documented rank decides only within one layer (project ranks run first).
        let priorities: std::collections::BTreeMap<String, u32> = roots
            .iter()
            .flat_map(|(layer, dir, rank)| {
                let mut out = Vec::new();
                for f in self.b.files_in(dir, ".md") {
                    out.push((self.b.display(&f), *rank));
                }
                let _ = layer;
                out
            })
            .collect();
        for (layer, dir, _rank) in roots {
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
            |i| i.origin.file.as_deref().and_then(|f| priorities.get(f)).copied().unwrap_or(9),
        );
    }

    /// One skill: invoked by name, so the listing (name + description) is measured at startup and the body
    /// only when the skill runs.
    fn add_skill(&mut self, f: &Path, layer: &str) -> Option<usize> {
        let Ok(text) = self.b.text(f) else { return None };
        let fm = frontmatter::parse(&text);
        let body = text[fm.body_start..].trim();
        if body.is_empty() {
            return None;
        }
        let name = if f.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            f.parent().and_then(Path::file_name)?.to_string_lossy().into_owned()
        } else {
            f.file_stem()?.to_string_lossy().into_owned()
        };
        let description = fm.str("description").unwrap_or("");
        let listing = char_len(&name) + char_len(description);
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
}
