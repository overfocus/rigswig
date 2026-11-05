//! The Claude Code adapter. Every rule it models is in `docs/adapters/claude-code.md`, with the docs it comes from.

mod files;
mod hooks;
mod mcp;
mod plugins;
mod settings;

use std::path::{Path, PathBuf};

use crate::adapter::{Adapter, Ctx, Detection};
use crate::adapters::build::Build;
use crate::model::{Harness, NotMeasured, Os};

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
    fn id(&self) -> &'static str {
        ID
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["CLAUDE_CONFIG_DIR", "ANTHROPIC_MODEL"]
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

    fn resolve(&self, ctx: &Ctx) -> Harness {
        let p = Paths::new(ctx);
        let mut b = Build::new(ctx, ID);
        b.detected_by = self.detect(ctx).evidence;

        let scopes = settings::layers(&mut b, &p);
        settings::model(&mut b, &scopes);
        settings::scalars(&mut b, &scopes);
        settings::permissions(&mut b, &scopes);

        let plugins = plugins::discover(&mut b, &p, &scopes);

        files::instructions(&mut b, &p);
        files::agents_skills_commands(&mut b, &p, &plugins);
        hooks::all(&mut b, &scopes, &plugins);
        mcp::all(&mut b, &p, &scopes, &plugins);
        plugins::link_contributions(&mut b, &plugins);

        b.not_measured.push(NotMeasured {
            what: "the system prompt and built-in tool definitions".into(),
            why: "they come with Claude Code, not from any file".into(),
        });
        let servers = b.items.iter().filter(|i| i.effective && i.detail.kind() == "mcp").count();
        if servers > 0 {
            b.not_measured.push(NotMeasured {
                what: format!("tool listings of {servers} MCP server{}", if servers == 1 { "" } else { "s" }),
                why: "rigswig reads server names, never connects to them".into(),
            });
        }
        for note in [
            "command-line flags (--model, --settings, --agents, --mcp-config, --add-dir, --plugin-dir)",
            "server-delivered and MDM managed settings (macOS profiles, Windows registry)",
            "folder trust: an untrusted folder ignores the project's own MCP approvals",
            "claude.ai connectors and bundled skills",
            "CLAUDE.md files in subfolders, which load when files there are used",
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
