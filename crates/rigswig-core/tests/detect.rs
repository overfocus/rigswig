//! The detection rule every adapter keeps (`adapter::Detection`): evidence from the home folder alone scores at most
//! 50, so a harness that is only installed never becomes the default over one the repo is set up for.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rigswig_core::adapter::Ctx;
use rigswig_core::adapters;
use rigswig_core::fs::MemFs;
use rigswig_core::model::Os;

const HOME: &str = "/home/dev";
const REPO: &str = "/home/dev/work/plain";

/// A home folder with every harness installed and configured, and a repo with none of their files.
fn everything_installed() -> MemFs {
    MemFs::new()
        .dir(format!("{REPO}/.git"))
        .file(format!("{HOME}/.claude/settings.json"), "{}")
        .file(format!("{HOME}/.claude/CLAUDE.md"), "# me")
        .file(format!("{HOME}/.claude.json"), "{}")
        .file(format!("{HOME}/.codex/config.toml"), "model = \"m\"")
        .file(format!("{HOME}/.codex/AGENTS.md"), "# me")
        .file(format!("{HOME}/.hermes/config.yaml"), "model: m\n")
        .file(format!("{HOME}/.hermes/SOUL.md"), "me")
        .file(format!("{HOME}/.hermes/skills/s/SKILL.md"), "---\nname: s\n---\n")
        .file(format!("{HOME}/.ollama/models/manifests/registry.ollama.ai/library/m/latest"), "{\"layers\":[]}")
        .file(format!("{HOME}/.ollama/server.json"), "{}")
        .file(format!("{HOME}/.ollama/config.json"), "{}")
        .dir(format!("{HOME}/.ollama/launch"))
        .file(format!("{HOME}/.config/devin/config.json"), "{}")
        .file(format!("{HOME}/.config/devin/AGENTS.md"), "# me")
}

#[test]
fn home_folder_evidence_alone_never_scores_over_50() {
    let fs = everything_installed();
    for a in adapters::all() {
        // Every variable the adapter reads, set, as a configured machine would have them.
        let env: BTreeMap<String, String> = a
            .env_vars()
            .iter()
            .map(|k| ((*k).to_string(), "1".to_string()))
            .filter(|(k, _)| !k.contains("HOME") && !k.contains("DIR") && !k.contains("MODELS"))
            .collect();
        let ctx = Ctx { fs: &fs, env, home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
        let d = a.detect(&ctx);
        assert!(d.score <= 50, "{} scores {} from the home folder alone ({:?})", a.id(), d.score, d.evidence);
    }
}

#[test]
fn a_repo_set_up_for_a_harness_is_read_with_it() {
    let fs = everything_installed().file(format!("{REPO}/CLAUDE.md"), "# plain");
    let ctx =
        Ctx { fs: &fs, env: BTreeMap::new(), home: PathBuf::from(HOME), repo: PathBuf::from(REPO), os: Os::Linux };
    let all = adapters::all();
    // The default pick: the highest score, the first in the list on a tie.
    let best = all.iter().rev().max_by_key(|a| a.detect(&ctx).score).unwrap();
    assert_eq!(best.id(), "claude-code");
}
