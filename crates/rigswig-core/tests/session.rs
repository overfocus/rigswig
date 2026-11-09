//! The live-session probe and the default pick: claims, the freshness window, the repo-folder boundary, and the
//! promise that a probe is metadata only (it never opens a file).
//!
//! Fixtures are invented: a repo whose files belong to Claude Code, and a Hermes home that is or is not active.
//! No harness actually ran here.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use rigswig_core::adapter::{Adapter, Ctx, Detection, PickCtx, PickReason, SessionClaim};
use rigswig_core::fs::{DirEntry, EntryKind, Fs, MemFs};
use rigswig_core::model::{Os, SessionScope};

/// A tree and the pick's inputs: a repo that is set up for Claude Code, under a home the tests fill in.
struct World {
    adapters: Vec<Box<dyn Adapter>>,
    fs: MemFs,
}

impl World {
    fn new() -> Self {
        let fs = MemFs::new()
            .file("/home/u/repo/CLAUDE.md", "the repo's own instruction")
            .dir("/home/u/repo/.claude")
            .dir("/home/u/.claude");
        Self { adapters: rigswig_core::adapters::all(), fs }
    }

    /// The same repo, empty of any harness's repo-side files: only the user config folder exists.
    fn bare() -> Self {
        let fs = MemFs::new().dir("/home/u/.claude");
        Self { adapters: rigswig_core::adapters::all(), fs }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { fs: &self.fs, env: BTreeMap::new(), home: "/home/u".into(), repo: "/home/u/repo".into(), os: Os::Linux }
    }

    fn scored(&self) -> Vec<(&dyn Adapter, Detection)> {
        let ctx = self.ctx();
        self.adapters.iter().map(|a| (a.as_ref(), a.detect(&ctx))).collect()
    }

    fn pick(&self, tool: Option<&str>, session_enabled: bool, now: SystemTime) -> rigswig_core::adapter::Pick<'_> {
        let scored = self.scored();
        self.pick_over(&scored, tool, session_enabled, now)
    }

    /// The pick over a scored list the caller keeps, so the returned `Pick` borrows only the `World` itself.
    fn pick_over<'a>(
        &'a self,
        scored: &[(&'a dyn Adapter, Detection)],
        tool: Option<&str>,
        session_enabled: bool,
        now: SystemTime,
    ) -> rigswig_core::adapter::Pick<'a> {
        let base = PickCtx { fs: &self.fs, home: "/home/u".into(), repo: "/home/u/repo".into(), os: Os::Linux };
        let env_of = |_a: &dyn Adapter| BTreeMap::new();
        rigswig_core::adapter::pick(scored, &base, &env_of, tool, session_enabled, now).expect("a pick")
    }
}

/// The Hermes home's activity files, last touched at `stamp`.
fn hermes_home_active(w: &mut World, stamp: SystemTime) {
    w.fs.add_file("/home/u/.hermes/state.db-wal", "");
    w.fs.add_file("/home/u/.hermes/.hermes_history", "");
    w.fs.stamp("/home/u/.hermes/state.db-wal", stamp);
    w.fs.stamp("/home/u/.hermes/.hermes_history", stamp - Duration::from_secs(120));
}

/// One Claude Code session log under this repo's projects folder (the folder name is the repo's slug).
fn claude_session_in_repo(w: &mut World, stamp: SystemTime) {
    // A canary in the file's body: nothing may ever read it.
    w.fs.add_file("/home/u/.claude/projects/-home-u-repo/aaaa.jsonl", "not read: the probe must never open this");
    w.fs.stamp("/home/u/.claude/projects/-home-u-repo/aaaa.jsonl", stamp);
}

#[test]
fn fresh_machine_claim_beats_the_repos_own_files() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(60));
    let pick = w.pick(None, true, now);
    assert_eq!(pick.adapter.id(), "hermes", "a live hermes session outranks claude-code's repo files");
    assert_eq!(pick.reason, PickReason::SessionMachine);
    let claim = pick.session.expect("the pick followed the claim");
    assert!(claim.evidence.iter().any(|e| e.contains("Hermes home: state.db-wal")), "have {:#?}", claim.evidence);
}

#[test]
fn stale_claims_fall_back_to_the_repos_files() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(16 * 60)); // outside the 15-minute window
    let pick = w.pick(None, true, now);
    assert_eq!(pick.adapter.id(), "claude-code");
    assert_eq!(pick.reason, PickReason::Evidence);
    assert!(pick.session.is_none());
}

#[test]
fn a_stale_artifact_is_observed_but_never_fresh() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(16 * 60));
    let hermes = w.adapters.iter().find(|a| a.id() == "hermes").unwrap();
    // The probe observes what is there; `fresh` is the pick's judgment, and it says no.
    let claim = hermes.session(&w.ctx()).expect("the artifacts are seen");
    assert_eq!(claim.latest, Some(now - Duration::from_secs(16 * 60)), "have {claim:?}");
    assert!(!claim.fresh(now), "a claim outside the window is never fresh");
}

#[test]
fn no_session_keeps_every_pick_evidence_only() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(60));
    let pick = w.pick(None, false, now);
    assert_eq!(pick.adapter.id(), "claude-code");
    assert_eq!(pick.reason, PickReason::Evidence);
    assert!(pick.session.is_none());
}

#[test]
fn an_explicit_tool_never_carries_a_claim() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(60));
    let pick = w.pick(Some("claude-code"), true, now);
    assert_eq!(pick.adapter.id(), "claude-code");
    assert_eq!(pick.reason, PickReason::Explicit);
    assert!(pick.session.is_none());
}

#[test]
fn a_repo_scoped_claim_beats_a_machine_scoped_one() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(60));
    claude_session_in_repo(&mut w, now - Duration::from_secs(30));
    let pick = w.pick(None, true, now);
    assert_eq!(pick.adapter.id(), "claude-code");
    assert_eq!(pick.reason, PickReason::SessionRepo);
    let claim = pick.session.expect("the pick followed the claim");
    assert!(claim.evidence.iter().any(|e| e.contains("projects/-home-u-repo:")), "have {:#?}", claim.evidence);
}

#[test]
fn a_sibling_repo_is_not_this_repos() {
    // The projects folder of a sibling whose name merely extends the repo's own: `repo2` after `repo`.
    let now = SystemTime::now();
    let mut w = World::new();
    w.fs.add_file("/home/u/.claude/projects/-home-u-repo2/bbbb.jsonl", "");
    w.fs.stamp("/home/u/.claude/projects/-home-u-repo2/bbbb.jsonl", now - Duration::from_secs(30));
    let claude = w.adapters.iter().find(|a| a.id() == "claude-code").unwrap();
    assert_eq!(claude.session(&w.ctx()), None, "repo2's sessions must not claim repo");
}

#[test]
fn a_subfolders_sessions_are_this_repos() {
    let now = SystemTime::now();
    let mut w = World::new();
    // The repo's slug, one dash further: `-home-u-repo-crates-core`.
    w.fs.add_file("/home/u/.claude/projects/-home-u-repo-crates-core/cccc.jsonl", "");
    w.fs.stamp("/home/u/.claude/projects/-home-u-repo-crates-core/cccc.jsonl", now - Duration::from_secs(30));
    let claude = w.adapters.iter().find(|a| a.id() == "claude-code").unwrap();
    let claim = claude.session(&w.ctx()).expect("a session under the repo's tree claims the repo");
    assert_eq!(claim.scope, SessionScope::Repo);
}

#[test]
fn a_session_folders_subagents_count_too() {
    // The real shape: <session>/ beside its .jsonl, and one file per subagent in <session>/subagents/.
    let now = SystemTime::now();
    let mut w = World::new();
    w.fs.add_file("/home/u/.claude/projects/-home-u-repo/sess-1.jsonl", "");
    w.fs.add_file("/home/u/.claude/projects/-home-u-repo/sess-2/subagents/dddd.jsonl", "");
    w.fs.stamp("/home/u/.claude/projects/-home-u-repo/sess-1.jsonl", now - Duration::from_secs(30));
    w.fs.stamp("/home/u/.claude/projects/-home-u-repo/sess-2/subagents/dddd.jsonl", now - Duration::from_secs(30));
    let claude = w.adapters.iter().find(|a| a.id() == "claude-code").unwrap();
    let claim = claude.session(&w.ctx()).expect("the session files count, however deep");
    assert_eq!(claim.evidence[0], "projects/-home-u-repo: 2 session files");
}

#[test]
fn a_clock_behind_the_files_is_never_stale() {
    // An mtime one minute ahead of the pick's clock (a skewed machine or a rounded-up stamp) must still claim.
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now + Duration::from_secs(60));
    let pick = w.pick(None, true, now);
    assert_eq!(pick.adapter.id(), "hermes", "have reason {:?}", pick.reason);
}

#[test]
fn with_nothing_anywhere_the_pick_is_the_old_machine_guess() {
    let now = SystemTime::now();
    let w = World::bare();
    let pick = w.pick(None, true, now);
    assert_eq!(pick.reason, PickReason::Guess, "the pick rests on the user config folder alone (score 50)");
    assert_eq!(pick.adapter.id(), "claude-code", "the first of adapters::all() on a tie");
    assert!(pick.session.is_none());
}

/// A file system that refuses to be read: the probe must pass on names, kinds and times alone.
struct NoOpens<'a> {
    inner: &'a MemFs,
}

impl Fs for NoOpens<'_> {
    fn read_to_string(&self, _: &Path) -> std::io::Result<String> {
        panic!("a session probe must never open a file");
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<DirEntry>> {
        self.inner.read_dir(path)
    }
    fn kind(&self, path: &Path) -> std::io::Result<EntryKind> {
        self.inner.kind(path)
    }
    fn canonicalize(&self, path: &Path) -> std::io::Result<std::path::PathBuf> {
        self.inner.canonicalize(path)
    }
    fn modified(&self, path: &Path) -> std::io::Result<SystemTime> {
        self.inner.modified(path)
    }
}

#[test]
fn the_probes_never_open_a_file() {
    let now = SystemTime::now();
    let mut w = World::new();
    hermes_home_active(&mut w, now - Duration::from_secs(60));
    claude_session_in_repo(&mut w, now - Duration::from_secs(30));
    let no_opens = NoOpens { inner: &w.fs };
    let env: BTreeMap<String, String> = BTreeMap::new();
    let c = Ctx { fs: &no_opens, env, home: "/home/u".into(), repo: "/home/u/repo".into(), os: Os::Linux };
    let claude = w.adapters.iter().find(|a| a.id() == "claude-code").unwrap();
    let hermes = w.adapters.iter().find(|a| a.id() == "hermes").unwrap();
    let claim = claude.session(&c).expect("the claude probe works on names and times only");
    assert!(claim.fresh(now));
    let claim = hermes.session(&c).expect("the hermes probe works on names and times only");
    assert!(claim.fresh(now));
}

#[test]
fn a_claim_with_no_time_is_never_fresh() {
    let c = SessionClaim { scope: SessionScope::Machine, latest: None, evidence: vec!["x".into()] };
    assert!(!c.fresh(SystemTime::now()));
}

#[test]
fn the_world_helper_sees_claudes_repo_evidence() {
    // Sanity for the fixture itself: claude-code's repo evidence is 100 here.
    let w = World::new();
    let scored = w.scored();
    let (claude, d) = scored.iter().find(|(a, _)| a.id() == "claude-code").expect("claude in the list");
    assert_eq!(claude.id(), "claude-code");
    assert_eq!(d.score, 100, "{}", d.evidence.join(", "));
}
