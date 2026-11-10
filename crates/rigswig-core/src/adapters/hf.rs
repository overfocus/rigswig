//! The Hugging Face CLI adapter: what the `hf` CLI's skill manager governs on disk. Every rule it models is in
//! `docs/adapters/hf.md`, quoted from the installed CLI's own code and huggingface.co/docs (all read
//! 2026-11-06, against huggingface_hub 2.4.0).

use std::path::{Path, PathBuf};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::{Build, calc, char_len};
use crate::fs::is_within;
use crate::model::*;
use crate::read::frontmatter;
use crate::read::redact::{printable, sha256_hex};

pub const ID: &str = "hf";
pub const ADAPTER_VERSION: &str = "0.1.0";
/// The release the docs and the installed CLI this adapter follows describe (read 2026-11-06).
const TOOL_VERSION: &str = "2.4.0";

/// The skill whose only copy is written by the CLI itself, generated per release.
const DEFAULT_SKILL_ID: &str = "hf-cli";
/// The version stamp the CLI writes into a generated SKILL.md (`cli/_skills.py`): the contract for staleness.
const STAMP_PREFIX: &str = "Generated with `huggingface_hub v";

pub struct Hf;

struct Paths {
    /// The project skills root the CLI manages (`AGENTS_SKILLS_LOCAL_PATH`, a constant with no env override).
    agents_local: PathBuf,
    /// The user skills root (`AGENTS_SKILLS_GLOBAL_PATH`).
    agents_global: PathBuf,
    /// The project Claude mirror (`CLAUDE_SKILLS_LOCAL_PATH`), always `.claude/skills` beside the repo.
    claude_local: PathBuf,
    /// The user Claude mirror (`CLAUDE_SKILLS_GLOBAL_PATH`): `~/.claude/skills`, moved by `CLAUDE_CONFIG_DIR`.
    claude_global: PathBuf,
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let claude_root = ctx.var("CLAUDE_CONFIG_DIR").map_or_else(|| ctx.home.join(".claude"), PathBuf::from);
        Self {
            agents_local: ctx.repo.clone().join(".agents/skills"),
            agents_global: ctx.home.join(".agents/skills"),
            claude_local: ctx.repo.clone().join(".claude/skills"),
            claude_global: claude_root.join("skills"),
        }
    }
}

impl Adapter for Hf {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        // `CLAUDE_CONFIG_DIR` moves the CLI's Claude mirror. `HF_HOME` moves only the CLI's state root, whose
        // stamps and registry cache are modelled nowhere, so it is not declared; the skills roots have no env.
        &["CLAUDE_CONFIG_DIR"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let has = |f: &Path| ctx.fs.exists(f);
        let checks = [
            (p.agents_local, ".agents/skills/", 100),
            (p.claude_local, ".claude/skills/", 60),
            (p.agents_global, "agents skills (global)", 45),
            (ctx.home.join(".hf-cli"), "hf CLI install dir", 45),
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

        b.add_layer(
            "project",
            LayerKind::Project,
            "the repo's skills store (`hf skills add` without --global lands here)",
            Some(b.display(&p.agents_local)),
            has(&p.agents_local),
            LayerMode::Default,
        );
        b.add_layer(
            "user",
            LayerKind::User,
            "the user skills root (`hf skills add -g`)",
            Some(b.display(&p.agents_global)),
            has(&p.agents_global),
            LayerMode::Default,
        );
        b.add_layer(
            "imports_project",
            LayerKind::Project,
            "the repo's Claude skills mirror the CLI symlinks into",
            None,
            has(&p.claude_local),
            LayerMode::Default,
        );
        b.add_layer(
            "imports_user",
            LayerKind::User,
            "the user's Claude skills mirror the CLI symlinks into",
            Some(b.display(&p.claude_global)),
            has(&p.claude_global),
            LayerMode::Default,
        );

        // The skills stores: every `<name>/SKILL.md` is a skill; the canonical copy wins its Claude mirror.
        let canonical = [(p.agents_local.clone(), "project"), (p.agents_global.clone(), "user")];
        let mut idx = Vec::new();
        // A store skill's content hash, so a mirror can twin even the Windows copy fallback (same bytes).
        let mut store_content: Vec<(String, String)> = Vec::new();
        for (store, layer) in &canonical {
            for d in b.dirs_in(store) {
                if let Some((ix, hash)) = add_skill(&mut b, &d, layer) {
                    store_content.push((b.items[ix].name.clone(), hash));
                    idx.push(ix);
                }
            }
        }
        // The Claude mirrors: the CLI links relative symlinks, or on Windows copies each skill in, so a mirror
        // entry counts when its real target lives inside a store OR its bytes twin a store skill. Anything
        // else there is another tool's skill and belongs to the Claude Code adapter.
        for (mirror, twin_layer) in
            [(p.claude_local.clone(), "imports_project"), (p.claude_global.clone(), "imports_user")]
        {
            for d in b.dirs_in(&mirror) {
                let links = ctx
                    .fs
                    .canonicalize(&d)
                    .is_ok_and(|real| canonical.iter().any(|(store, _)| is_within(&real, store)));
                let twinned = links
                    || ctx.fs.read_to_string(&d.join("SKILL.md")).ok().is_some_and(|text| {
                        store_content.iter().any(|(_, store_hash)| *store_hash == sha256_hex(&text))
                    });
                if twinned {
                    if let Some((ix, _)) = add_skill(&mut b, &d, twin_layer) {
                        idx.push(ix);
                    }
                }
            }
        }
        // The canonical store beats the mirror twin on the same name; the mirror stays listed.
        b.resolve_clashes(
            &idx,
            |i| i.name.clone(),
            |i| {
                let f = i.origin.file.as_deref().unwrap_or_default();
                if f.contains("/.claude") || f.starts_with(".claude") { 1 } else { 0 }
            },
        );

        // The staleness audit, the CLI's own contract (`_GENERATED_VERSION_RE` in cli/_skills.py): a generated
        // hf-cli SKILL.md must carry "Generated with `huggingface_hub v<installed>`". Global takes precedence
        // in the CLI's hint, and each copy is warned individually here.
        let installed = installed_version(ctx);
        for (store, scope) in [(p.agents_global.clone(), "global"), (p.agents_local.clone(), "project")] {
            let skill_md = store.join(DEFAULT_SKILL_ID).join("SKILL.md");
            let content = match b.text(&skill_md) {
                Ok(text) => text,
                Err(_) => continue, // no generated skill in this root
            };
            match (stamp_of(&content), installed.clone()) {
                (Some(generated), Some(installed_version)) if generated == installed_version => {} // fresh
                (generated, installed_version) => {
                    let message = match (generated, installed_version) {
                        (Some(generated), Some(installed_version)) => {
                            format!(
                                "the {scope} `{DEFAULT_SKILL_ID}` skill was generated by huggingface_hub \
                                 v{generated} but the CLI on disk is v{installed_version}; run \
                                 `hf skills update {DEFAULT_SKILL_ID}` (add -g for the global copy)"
                            )
                        }
                        (Some(generated), None) => {
                            format!(
                                "the {scope} `{DEFAULT_SKILL_ID}` skill was generated by huggingface_hub \
                                 v{generated} but no installed version was found on disk (looked for a \
                                 `huggingface_hub-*.dist-info`)"
                            )
                        }
                        (None, _) => {
                            format!("the {scope} `{DEFAULT_SKILL_ID}` skill has no generated-version stamp")
                        }
                    };
                    b.warn("hf-cli-skill-stale", message, Some(&skill_md));
                }
            }
        }

        b.link_packages();
        b.not_measured.push(NotMeasured {
            what: "the CLI commands themselves and the generated skill's body".into(),
            why: "the hf-cli skill's SKILL.md is the CLI's own command reference, regenerated on install".into(),
        });
        b.not_measured_common();

        for note in [
            "the generated auth state and the token files it guards: credentials, always outside the model",
            "the 24-hour update-check stamps (`~/.cache/huggingface/.check_for_update_done`, `.check_for_skill_update_done`): mtime-only reminder state",
            "the cached harness registry (`~/.cache/huggingface/.agent_harnesses.json`): the CLI's runtime detection of which agent runs it, mirrored from the Hub every 24h — telemetry, not configuration",
            "marketplace skill update status (`hf skills update` reaches the Hub): a networked check this read-only tool does not run; each installed skill's local copy is what is modelled",
            "skills installed with `--dest` into a custom directory and the agentsmd AGENTS.md fallback bundles: outside the two standards roots and the two Claude mirrors",
            "which skills a session's trust state disables",
        ] {
            b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        b.finish(ADAPTER_VERSION, Some(TOOL_VERSION), printable(&repo_label))
    }
}

/// One skill folder on disk (the Agent-Skills layout: `<name>/SKILL.md`), named by its frontmatter or folder.
/// `rank` is the source's priority (canonical store 0, mirror 1).
fn add_skill(b: &mut Build, dir: &Path, layer: &str) -> Option<(usize, String)> {
    let f = dir.join("SKILL.md");
    let Ok(text) = b.text(&f) else { return None };
    let fm = frontmatter::parse(&text);
    let body = text[fm.body_start..].trim();
    if body.is_empty() {
        return None;
    }
    let name = fm
        .str("name")
        .map(str::to_string)
        .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    if name.is_empty() {
        return None;
    }
    let listing = char_len(&name) + char_len(fm.str("description").unwrap_or(""));
    let mut it = b.item(
        layer,
        &name,
        ItemDetail::Skill(SkillAttrs { body_chars: Some(char_len(body)) }),
        LoadWhen::OnInvoke,
        calc(listing),
    );
    it.origin.file = Some(b.display(&f));
    let hash = sha256_hex(&text);
    it.load.content_hash = Some(hash.clone());
    Some((b.push(it), hash))
}

/// The generated-version stamp inside a generated SKILL.md, up to the closing backtick.
fn stamp_of(content: &str) -> Option<String> {
    let at = content.find(STAMP_PREFIX)?;
    let rest = &content[at + STAMP_PREFIX.len()..];
    let end = rest.find('`')?;
    let version = rest[..end].trim();
    (!version.is_empty() && !version.contains(char::is_whitespace)).then(|| version.to_string())
}

/// The installed huggingface_hub version, from the `*.dist-info` folder name next to the CLI. The standalone
/// installer's venv first (`~/.hf-cli/venv`), then pipx and uv tool venvs; nothing found means unknown.
fn installed_version(ctx: &Ctx) -> Option<String> {
    let stores =
        [ctx.home.join(".hf-cli/venv"), ctx.home.join(".local/pipx/venvs"), ctx.home.join(".local/share/uv/tools")];
    for venv_root in stores {
        // Each venv root may hold many venvs (pipx, uv); the standalone install is a single venv dir.
        for venv in if venv_root == ctx.home.join(".hf-cli/venv") {
            vec![venv_root.clone()]
        } else {
            ctx.fs
                .read_dir(&venv_root)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.kind == crate::fs::EntryKind::Dir)
                .map(|e| venv_root.join(&e.name))
                .collect::<Vec<_>>()
        } {
            for py in ctx.fs.read_dir(&venv.join("lib")).unwrap_or_default() {
                if py.kind != crate::fs::EntryKind::Dir {
                    continue;
                }
                let site = venv.join("lib").join(&py.name).join("site-packages");
                for entry in ctx.fs.read_dir(&site).unwrap_or_default() {
                    let name = entry.name.as_str();
                    if let Some(version) =
                        name.strip_prefix("huggingface_hub-").and_then(|v| v.strip_suffix(".dist-info"))
                    {
                        return Some(version.to_string());
                    }
                }
            }
        }
    }
    None
}
