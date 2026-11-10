//! The Claude Code adapter. Every rule it models is in `docs/adapters/claude-code.md`, with the docs it comes from.

mod change;
mod files;
mod hooks;
mod mcp;
mod plugins;
mod settings;
mod usage;

use std::path::{Path, PathBuf};

use crate::adapter::{Adapter, Ctx, Detection, SessionClaim};
use crate::adapters::build::Build;
use crate::fs::EntryKind;
use crate::model::{Harness, NotMeasured, Os, SessionScope};

pub const ID: &str = "claude-code";
pub const ADAPTER_VERSION: &str = "0.1.0";

pub struct ClaudeCode;

/// Where Claude Code keeps things for this user and repo.
pub(crate) struct Paths {
    pub cfg: PathBuf,
    pub claude_json: PathBuf,
    pub system: PathBuf,
    pub repo: PathBuf,
}

/// The system folder for managed settings, managed MCP and the managed CLAUDE.md.
pub fn system_dir(os: Os) -> PathBuf {
    match os {
        Os::Macos => PathBuf::from("/Library/Application Support/ClaudeCode"),
        Os::Windows => PathBuf::from(r"C:\Program Files\ClaudeCode"),
        Os::Linux | Os::Other => PathBuf::from("/etc/claude-code"),
    }
}

/// The projects-folder name of a working directory: every character that is not an ASCII letter or digit becomes
/// `-` (**inferred** — the folder names Claude Code writes; the docs never say). `/Users/me/my repo` becomes
/// `-Users-me-my-repo`.
pub fn project_slug(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Whether a projects-folder entry is this repo's sessions: the repo's slug itself, or the slug with one more
/// dash in front (a subfolder's sessions). A sibling whose name extends the repo's (`my-repo2` after `my-repo`)
/// is not matched, so a claim always errs towards "not this repo".
fn folder_is_repo(folder: &str, slug: &str) -> bool {
    match folder.strip_prefix(slug) {
        Some("") => true,
        Some(rest) => rest.starts_with('-'),
        None => false,
    }
}

/// The newest `.jsonl` under one session folder, down to two subfolder levels (<session>/<subfolder>/file is the
/// real shape: a session's log beside its `subagents/`), and how many there are. Only names, kinds and
/// modification times: no file is opened.
fn jsonl_activity(fs: &dyn crate::fs::Fs, root: &Path) -> (Option<std::time::SystemTime>, usize) {
    let mut latest = None;
    let mut n = 0;
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        for e in fs.read_dir(&dir).unwrap_or_default() {
            let p = dir.join(&e.name);
            match e.kind {
                EntryKind::File if e.name.ends_with(".jsonl") => {
                    n += 1;
                    if let Ok(t) = fs.modified(&p) {
                        latest = latest.max(Some(t));
                    }
                }
                EntryKind::Dir if depth < 2 => stack.push((p, depth + 1)),
                _ => {}
            }
        }
    }
    (latest, n)
}

impl Paths {
    fn new(ctx: &Ctx) -> Self {
        let (cfg, claude_json) = match ctx.var("CLAUDE_CONFIG_DIR") {
            Some(d) => {
                let d = ctx.home.join(Path::new(d).strip_prefix("~/").unwrap_or(Path::new(d)));
                (d.clone(), d.join(".claude.json"))
            }
            None => (ctx.home.join(".claude"), ctx.home.join(".claude.json")),
        };
        Self { cfg, claude_json, system: system_dir(ctx.os), repo: ctx.repo.clone() }
    }
}

impl Adapter for ClaudeCode {
    fn strict_ready(&self) -> bool {
        true
    }

    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["CLAUDE_CONFIG_DIR", "ANTHROPIC_MODEL", "CLAUDE_CODE_DISABLE_AUTO_MEMORY", "CLAUDE_CODE_PROJECT_DIR_NAME"]
    }

    fn detect(&self, ctx: &Ctx) -> Detection {
        let p = Paths::new(ctx);
        let checks = [
            (p.repo.join(".claude"), ".claude/", 100),
            (p.repo.join("CLAUDE.md"), "CLAUDE.md", 100),
            (p.repo.join(".mcp.json"), ".mcp.json", 60),
            (p.cfg.clone(), "user config folder", 50),
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

    /// The session look: the session-log files under the config's `projects/` folder (`docs/USAGE.md`, "What is
    /// read"). Claude Code names a session folder after the repo it runs in and rewrites the logs as a session
    /// works, so a fresh `.jsonl` under this repo's folder (or a subfolder's — one dash in front) is a
    /// repo-scoped claim. Metadata only: no file is ever opened.
    fn session(&self, ctx: &Ctx) -> Option<SessionClaim> {
        let p = Paths::new(ctx);
        let projects = p.cfg.join("projects");
        if !ctx.fs.exists(&projects) {
            return None;
        }
        let slug = project_slug(&ctx.repo);
        let mut latest = None;
        let mut files = 0;
        for e in ctx.fs.read_dir(&projects).unwrap_or_default() {
            if e.kind != EntryKind::Dir || !folder_is_repo(&e.name, &slug) {
                continue;
            }
            let (l, n) = jsonl_activity(ctx.fs, &projects.join(&e.name));
            latest = latest.max(l);
            files += n;
        }
        latest?;
        Some(SessionClaim {
            scope: SessionScope::Repo,
            latest: Some(latest.unwrap()),
            evidence: vec![format!("projects/{slug}: {files} session file{}", if files == 1 { "" } else { "s" })],
        })
    }

    fn plan(&self, ctx: &Ctx, change: &crate::change::Change) -> Result<crate::change::Plan, String> {
        change::plan(ctx, change)
    }

    fn usage(&self, ctx: &Ctx, since: Option<&str>) -> Result<crate::usage::Usage, String> {
        usage::usage(ctx, since)
    }

    fn resolve(&self, ctx: &Ctx) -> Harness {
        let p = Paths::new(ctx);
        let mut b = Build::new(ctx, ID);
        b.detected_by = self.detect(ctx).evidence;

        let scopes = settings::layers(&mut b, &p);
        settings::model(&mut b, &scopes);
        settings::scalars(&mut b, &scopes);
        settings::permissions(&mut b, &scopes);

        let plugins = plugins::discover(&mut b, &p, &scopes);

        let subs = files::subfolders(&mut b, &p.repo);
        files::instructions(&mut b, &p, &subs);
        files::memory_index(&mut b, &p);
        files::agents_skills_commands(&mut b, &p, &plugins, &subs);
        hooks::all(&mut b, &scopes, &plugins);
        mcp::all(&mut b, &p, &scopes, &plugins);
        plugins::link_contributions(&mut b, &plugins);

        b.not_measured.push(NotMeasured {
            what: "the system prompt and built-in tool definitions".into(),
            why: "they come with Claude Code, not from any file".into(),
        });
        b.not_measured_common();
        for note in [
            "command-line flags (--model, --settings, --agents, --mcp-config, --add-dir, --plugin-dir)",
            "server-delivered and MDM managed settings (macOS profiles, Windows registry)",
            "folder trust: an untrusted folder ignores the project's own MCP approvals",
            "claude.ai connectors and bundled skills",
            "hooks, MCP servers and skills declared in agent or skill front matter",
            "output styles, LSP servers, monitors, workflows and themes",
            "settings files with comments (JSONC)",
        ] {
            b.note_not_modelled(note);
        }
        let repo_label = ctx.repo.file_name().map_or_else(|| ".".into(), |n| n.to_string_lossy().into_owned());
        b.finish(ADAPTER_VERSION, Some("2.1.277"), crate::read::redact::printable(&repo_label))
    }
}
