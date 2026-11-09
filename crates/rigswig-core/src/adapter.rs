//! What every harness adapter implements, and what it is given.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::fs::Fs;
use crate::model::{Harness, Os, SessionScope};

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
    /// 0 (no sign of the tool) to 100 (its own config files are in the repo). Evidence from the home folder alone
    /// (a config folder, a model store, environment variables) scores at most 50: above 50 means the repo itself has
    /// the harness's files. The harness picked by default is the highest score, the first in `adapters::all()` on a
    /// tie, so a harness that is only installed never wins over one the repo is set up for.
    /// `tests/detect.rs` holds every adapter to this.
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

    /// What past sessions in `ctx.repo` invoked, from the harness's own session logs, since `since` (an ISO 8601
    /// date or time; `None` for every log there is). Opt-in per run: only `rigswig usage` and `audit --usage` call
    /// it, and what it may read is metadata ([`crate::usage`]). The default refuses: an adapter opts in.
    fn usage(&self, _ctx: &Ctx, _since: Option<&str>) -> Result<crate::usage::Usage, String> {
        Err(format!("rigswig does not read {} session logs yet", self.id()))
    }

    /// What session activity this harness shows right now, as file-system metadata only: folder names, kinds and
    /// modification times, never a file opened and never a log's contents. The claim only observes — [`pick`]
    /// decides, against the caller's clock, because an adapter gets no clock. The default: no session signal.
    fn session(&self, _ctx: &Ctx) -> Option<SessionClaim> {
        None
    }
}

/// How long after its newest observed activity a session claim still counts as live: longer than a quiet stretch
/// inside one session (a long model turn), shorter than "last night".
pub const SESSION_WINDOW: Duration = Duration::from_secs(15 * 60);

/// What a probe saw about one harness's session activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClaim {
    pub scope: SessionScope,
    /// The newest modification time seen, `None` when nothing was seen at all.
    pub latest: Option<SystemTime>,
    /// What was seen, as short labels — no session ids, no log text.
    pub evidence: Vec<String>,
}

impl SessionClaim {
    /// `now` within [`SESSION_WINDOW`] of the newest activity seen. A file stamped slightly ahead of the clock
    /// (skew) still counts: a live session never hides behind a rounded-up mtime.
    pub fn fresh(&self, now: SystemTime) -> bool {
        self.latest.is_some_and(|t| t > now || now.duration_since(t).is_ok_and(|d| d <= SESSION_WINDOW))
    }
}

/// Why the pick chose its harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickReason {
    /// `--tool` named it; no session claim is reported for an explicit pick.
    Explicit,
    /// A repo-scoped live claim won: session artifacts naming this repo, freshly touched.
    SessionRepo,
    /// A machine-scoped live claim won: the harness is active on this machine, somewhere.
    SessionMachine,
    /// The repo's own files point at it.
    Evidence,
    /// Nothing in the repo points at any harness we read (the winner scores at most 50).
    Guess,
}

/// The shared inputs a default pick builds a probing `Ctx` from. The caller owns each adapter's environment
/// (`env_of` filters the process's variables exactly as `with_harness` does); the pick never reads the
/// environment itself, so tests keep full control.
pub struct PickCtx<'a> {
    pub fs: &'a dyn Fs,
    pub home: PathBuf,
    pub repo: PathBuf,
    pub os: Os,
}

/// One default pick: which adapter, and why.
pub struct Pick<'a> {
    pub adapter: &'a dyn Adapter,
    pub score: u8,
    pub reason: PickReason,
    /// The winner's fresh session claim, when the pick followed one.
    pub session: Option<SessionClaim>,
}

/// The default pick. Order of authority: `--tool`; then a fresh repo-scoped claim; then a fresh machine-scoped
/// one; then the repo's own evidence, ties to the first of `adapters::all()` (the list runs reversed, since
/// `max_by_key` keeps the last of equal maximums); a winner that only home-folder evidence supported (score at
/// most 50) is a machine guess. `session_enabled` false keeps every pick evidence-only. Each fresh claim picks
/// its harness by newest activity, ties by detection score, then list order — on an exact tie, the first named.
pub fn pick<'a>(
    scored: &[(&'a dyn Adapter, Detection)],
    base: &PickCtx<'a>,
    env_of: &dyn Fn(&dyn Adapter) -> BTreeMap<String, String>,
    tool: Option<&str>,
    session_enabled: bool,
    now: SystemTime,
) -> Result<Pick<'a>, String> {
    if let Some(t) = tool {
        let (a, d) = scored
            .iter()
            .find(|(a, _)| a.id() == t)
            .ok_or_else(|| format!("no harness called {t} (see `rigswig tools`)"))?;
        return Ok(Pick { adapter: *a, score: d.score, reason: PickReason::Explicit, session: None });
    }
    if session_enabled {
        let claims: Vec<(&'a dyn Adapter, u8, SessionClaim)> = scored
            .iter()
            .filter_map(|(a, d)| {
                let ctx =
                    Ctx { fs: base.fs, env: env_of(*a), home: base.home.clone(), repo: base.repo.clone(), os: base.os };
                let c = a.session(&ctx)?;
                c.fresh(now).then_some((*a, d.score, c))
            })
            .collect();
        for (scope, reason) in
            [(SessionScope::Repo, PickReason::SessionRepo), (SessionScope::Machine, PickReason::SessionMachine)]
        {
            if let Some((a, score, c)) = claims
                .iter()
                .filter(|(_, _, c)| c.scope == scope)
                .rev()
                .max_by_key(|(_, score, c)| (c.latest, *score))
                .map(|(a, s, c)| (*a, *s, c.clone()))
            {
                return Ok(Pick { adapter: a, score, reason, session: Some(c) });
            }
        }
    }
    let (a, d) = scored.iter().rev().max_by_key(|(_, d)| d.score).expect("at least one adapter");
    let reason = if d.score <= 50 { PickReason::Guess } else { PickReason::Evidence };
    Ok(Pick { adapter: *a, score: d.score, reason, session: None })
}
