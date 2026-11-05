//! What every harness adapter implements, and what it is given.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::fs::Fs;
use crate::model::{Harness, Os};

/// Everything an adapter may look at. It gets nothing else: no process environment, no clock, no network.
pub struct Ctx<'a> {
    pub fs: &'a dyn Fs,
    /// Only the variables the caller chose to pass (e.g. `ANTHROPIC_MODEL`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`).
    pub env: BTreeMap<String, String>,
    pub home: PathBuf,
    pub repo: PathBuf,
    pub os: Os,
}

impl Ctx<'_> {
    pub fn var(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }
}

/// How sure an adapter is that its tool is configured for this repo or user, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    /// 0 (no sign of the tool) to 100 (its own config files are present).
    pub score: u8,
    /// The files or settings that were found, as short labels.
    pub evidence: Vec<String>,
}

/// One harness: Claude Code, Codex, Gemini CLI, ...
pub trait Adapter {
    /// The tool id used in item ids and `tool.id`.
    fn id(&self) -> &'static str;
    /// The environment variables this adapter reads, so the caller can pass exactly those.
    fn env_vars(&self) -> &'static [&'static str];
    fn detect(&self, ctx: &Ctx) -> Detection;
    /// Read and resolve everything. Never fails: what cannot be read becomes a warning or a coverage note.
    fn resolve(&self, ctx: &Ctx) -> Harness;

    /// Plan a change (write mode): what applying it would do. Planning reads only; the binary applies a plan. The
    /// default refuses: an adapter opts in kind by kind.
    fn plan(&self, _ctx: &Ctx, _change: &crate::change::Change) -> Result<crate::change::Plan, String> {
        Err(format!("rigswig cannot change {} settings yet", self.id()))
    }
}
