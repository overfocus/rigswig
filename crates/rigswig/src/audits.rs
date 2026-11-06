//! Running audits: one repo, or a fleet of them, with progress. The CLI runs them in the foreground; the page starts one
//! as a job on its own thread (`/api/audit/start`) and polls it (`/api/audit/poll`) to draw the progress bar.
//!
//! Audits only read, so the page may run them without write mode. One job at a time; a new one replaces a finished one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rigswig_core::audit::{self, Audit, Fleet, Progress};

use crate::{Remote, expand, home, load_remote, private_folders, real_path, this_os, with_harness};

/// Audit one repo; `progress` hears every stage, from reading the harness to ranking.
pub fn repo(path: &Path, tool: Option<&str>, progress: &mut dyn FnMut(Progress)) -> Result<(Audit, u8), String> {
    with_harness(path, tool, |a, ctx, score| {
        progress(Progress { stage: "Reading the harness".into(), fraction: 0.0 });
        let (signals, note) = audit::user_signals(ctx);
        let h = a.resolve(ctx);
        let mut out = audit::audit_with(ctx, &h, &audit::Options { signals }, &mut |p| {
            progress(Progress { stage: p.stage, fraction: 0.1 + 0.9 * p.fraction })
        });
        out.notes.extend(note);
        (out, score)
    })
}

/// Audit a repo that is not here (GitHub or an archive file): in memory, with an empty home and no environment. The
/// download is most of the wait, so it has the first 40% of the bar, by bytes received (GitHub sends no total).
pub fn remote(r: &Remote, tool: Option<&str>, progress: &mut dyn FnMut(Progress)) -> Result<Audit, String> {
    let what = match r {
        Remote::Github(spec) => format!("Downloading {} from codeload.github.com", spec.label()),
        Remote::Archive(p) => {
            format!("Reading {}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
        }
    };
    progress(Progress { stage: what.clone(), fraction: 0.0 });
    let (fs, root, label, summary) = load_remote(r, &mut |bytes| {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        progress(Progress { stage: format!("{what} ({mb:.1} MB)"), fraction: 0.4 * (1.0 - (-mb / 8.0).exp()) })
    })?;
    progress(Progress { stage: "Unpacked in memory; reading the harness".into(), fraction: 0.45 });
    let adapters = rigswig_core::adapters::all();
    // Nothing from this machine: no environment variables, an invented home, the archive's own files only.
    let ctx = rigswig_core::adapter::Ctx {
        fs: &fs,
        env: std::collections::BTreeMap::new(),
        home: PathBuf::from(crate::github::HOME),
        repo: root,
        os: this_os(),
    };
    let a = match tool {
        Some(t) => {
            adapters.iter().find(|a| a.id() == t).ok_or(format!("no harness called {t} (see `rigswig tools`)"))?
        }
        None => adapters.iter().max_by_key(|a| a.detect(&ctx).score).expect("an adapter"),
    };
    let h = a.resolve(&ctx);
    // The built-in signals only: ~/.rigswig/signals.json belongs to this machine.
    let mut out = audit::audit_with(&ctx, &h, &audit::Options::default(), &mut |p| {
        progress(Progress { stage: p.stage, fraction: 0.5 + 0.5 * p.fraction })
    });
    out.repo = rigswig_core::read::redact::printable(&label);
    out.notes.insert(0, summary);
    Ok(out)
}

/// Fleet mode: every repo's audit and the fleet built from them.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetRun {
    pub fleet: Fleet,
    /// Each repo's own audit, in the order of `fleet.repos`.
    pub audits: Vec<Audit>,
    /// Folders with no harness of their own.
    pub skipped: Vec<String>,
    /// Folders left out because `~/.rigswig/private.json` lists them (not named).
    pub private_skipped: usize,
    /// Repos that could not be read.
    pub errors: Vec<String>,
}

pub fn fleet(patterns: &[String], tool: Option<&str>, progress: &mut dyn FnMut(Progress)) -> Result<FleetRun, String> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for p in patterns {
        paths.extend(expand(p)?);
    }
    paths.sort();
    paths.dedup();
    let private = home().map(|h| private_folders(&h)).unwrap_or_default();
    let before = paths.len();
    paths.retain(|p| {
        let r = real_path(p).unwrap_or_else(|_| p.clone());
        !private.iter().any(|x| r.starts_with(x))
    });
    let private_skipped = before - paths.len();
    let n = paths.len().max(1);
    let (mut audits, mut skipped, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    for (i, p) in paths.iter().enumerate() {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let shown = rigswig_core::read::redact::printable(&name);
        let mut each = |x: Progress| {
            progress(Progress {
                stage: format!("{shown} ({} of {}): {}", i + 1, paths.len(), x.stage),
                fraction: (i as f64 + x.fraction) / n as f64,
            })
        };
        match repo(p, tool, &mut each) {
            // A repo is a git checkout or a folder with the harness's own files; the user config folder alone (which
            // every folder sees) does not make one.
            Ok((a, score)) if score > 50 || p.join(".git").exists() => audits.push(a),
            Ok(_) => skipped.push(shown),
            Err(e) => errors.push(format!("{shown}: {e}")),
        }
    }
    if audits.is_empty() {
        return Err("none of those folders shows a harness rigswig knows".into());
    }
    let fleet = audit::fleet(&audits);
    Ok(FleetRun { fleet, audits, skipped, private_skipped, errors })
}

/// One audit job, as the page polls it.
#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: u64,
    /// `running`, `done` or `failed`.
    pub state: &'static str,
    /// `repo` or `fleet`.
    pub mode: &'static str,
    pub stage: String,
    pub fraction: f64,
    pub elapsed_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip)]
    started: Option<Instant>,
}

/// The page's audit jobs: at most one, shared with the thread running it.
#[derive(Clone, Default)]
pub struct Jobs {
    current: Arc<Mutex<Option<Job>>>,
    next: Arc<Mutex<u64>>,
    /// The page was started with `--allow-github`: a job may download a public repo from GitHub.
    allow_github: bool,
}

/// What a job audits.
enum Work {
    Repo,
    Fleet(Vec<String>),
    Github(crate::github::Spec),
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Start {
    mode: String,
    #[serde(default)]
    repos: Vec<String>,
    /// For `github`: `owner/repo[@ref]` or a github.com address.
    #[serde(default)]
    repo: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Poll {
    id: u64,
}

/// At most this many folder patterns in one fleet job.
const MAX_PATTERNS: usize = 32;

impl Jobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Job>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `/api/audit/start` and `/api/audit/poll`. `repo` is the page's repo; `tool` the harness it shows.
    pub fn route(&self, route: &str, repo: &Path, tool: &str, body: &str) -> Result<String, (u16, String)> {
        let bad = |e: serde_json::Error| (400, format!("bad request: {e}"));
        match route {
            "/api/audit/start" => {
                let req: Start = serde_json::from_str(body).map_err(bad)?;
                let (mode, work): (&'static str, Work) = match req.mode.as_str() {
                    "repo" if req.repos.is_empty() && req.repo.is_none() => ("repo", Work::Repo),
                    "repo" => return Err((400, "a repo audit takes no repos".into())),
                    "fleet" => {
                        let ok = req.repo.is_none()
                            && !req.repos.is_empty()
                            && req.repos.len() <= MAX_PATTERNS
                            && req.repos.iter().all(|r| !r.trim().is_empty() && r.len() <= 1024);
                        if !ok {
                            return Err((400, format!("a fleet audit needs 1 to {MAX_PATTERNS} folders or patterns")));
                        }
                        ("fleet", Work::Fleet(req.repos))
                    }
                    "github" => {
                        if !self.allow_github {
                            return Err((
                                403,
                                "this page does not download: start it with rigswig ui --allow-github to audit GitHub repos"
                                    .into(),
                            ));
                        }
                        let input = req
                            .repo
                            .filter(|_| req.repos.is_empty())
                            .ok_or((400, "a GitHub audit needs one repo".to_string()))?;
                        let spec = crate::github::Spec::parse(&input).map_err(|e| (400, e))?;
                        ("github", Work::Github(spec))
                    }
                    _ => return Err((400, "mode is repo, fleet or github".into())),
                };
                Ok(serde_json::to_string(&self.start(mode, work, repo.to_path_buf(), tool.to_string())?)
                    .expect("serialises"))
            }
            "/api/audit/poll" => {
                let req: Poll = serde_json::from_str(body).map_err(bad)?;
                let cur = self.lock();
                match cur.as_ref() {
                    Some(j) if j.id == req.id => {
                        let mut j = j.clone();
                        j.elapsed_ms = j.started.map_or(0, |s| s.elapsed().as_millis() as u64);
                        if j.state == "running" {
                            j.result = None;
                        }
                        Ok(serde_json::to_string(&j).expect("serialises"))
                    }
                    _ => Err((404, "no such audit".into())),
                }
            }
            _ => Err((404, "no such action".into())),
        }
    }

    pub fn new(allow_github: bool) -> Self {
        Self { allow_github, ..Self::default() }
    }

    fn start(&self, mode: &'static str, work: Work, repo: PathBuf, tool: String) -> Result<Job, (u16, String)> {
        let id = {
            let mut n = self.next.lock().unwrap_or_else(|e| e.into_inner());
            *n += 1;
            *n
        };
        let job = Job {
            id,
            state: "running",
            mode,
            stage: "Starting".into(),
            fraction: 0.0,
            elapsed_ms: 0,
            result: None,
            error: None,
            started: Some(Instant::now()),
        };
        {
            let mut cur = self.lock();
            if cur.as_ref().is_some_and(|j| j.state == "running") {
                return Err((409, "an audit is already running".into()));
            }
            *cur = Some(job.clone());
        }
        let me = self.clone();
        std::thread::spawn(move || {
            let update = |f: &mut dyn FnMut(&mut Job)| {
                if let Some(j) = me.lock().as_mut().filter(|j| j.id == id) {
                    f(j);
                }
            };
            let mut progress = |p: Progress| {
                update(&mut |j| {
                    j.stage = p.stage.clone();
                    j.fraction = j.fraction.max(p.fraction.clamp(0.0, 1.0));
                })
            };
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &work {
                Work::Repo => repo_json(&repo, &tool, &mut progress),
                Work::Fleet(repos) => {
                    fleet(repos, Some(&tool), &mut progress).map(|f| serde_json::to_value(&f).expect("serialises"))
                }
                // The harness the repo itself shows the most evidence of, not the page's.
                Work::Github(spec) => remote(&Remote::Github(spec.clone()), None, &mut progress)
                    .map(|a| serde_json::to_value(&a).expect("serialises")),
            }));
            let outcome = run.unwrap_or_else(|_| Err("the audit stopped on an internal error".into()));
            update(&mut |j| match &outcome {
                Ok(v) => {
                    j.state = "done";
                    j.stage = "Done".into();
                    j.fraction = 1.0;
                    j.result = Some(v.clone());
                }
                Err(e) => {
                    j.state = "failed";
                    j.error = Some(rigswig_core::read::redact::printable(e));
                }
            });
        });
        Ok(job)
    }
}

fn repo_json(repo: &Path, tool: &str, progress: &mut dyn FnMut(Progress)) -> Result<serde_json::Value, String> {
    repo_audit(repo, tool, progress).map(|a| serde_json::to_value(&a).expect("serialises"))
}

fn repo_audit(repo: &Path, tool: &str, progress: &mut dyn FnMut(Progress)) -> Result<Audit, String> {
    self::repo(repo, Some(tool), progress).map(|(a, _)| a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_check_their_input_before_starting_anything() {
        let j = Jobs::default();
        let repo = Path::new("/nowhere");
        let code = |r: Result<String, (u16, String)>| r.err().map(|e| e.0);
        assert_eq!(code(j.route("/api/audit/start", repo, "claude-code", "{}")), Some(400), "mode is required");
        assert_eq!(code(j.route("/api/audit/start", repo, "claude-code", r#"{"mode":"all"}"#)), Some(400));
        assert_eq!(code(j.route("/api/audit/start", repo, "claude-code", r#"{"mode":"fleet"}"#)), Some(400));
        assert_eq!(
            code(j.route("/api/audit/start", repo, "claude-code", r#"{"mode":"fleet","repos":["  "]}"#)),
            Some(400)
        );
        let many = serde_json::json!({ "mode": "fleet", "repos": vec!["~/x"; MAX_PATTERNS + 1] }).to_string();
        assert_eq!(code(j.route("/api/audit/start", repo, "claude-code", &many)), Some(400));
        assert_eq!(
            code(j.route("/api/audit/start", repo, "claude-code", r#"{"mode":"repo","repos":["~/x"]}"#)),
            Some(400)
        );
        assert_eq!(code(j.route("/api/audit/start", repo, "claude-code", r#"{"mode":"repo","extra":1}"#)), Some(400));
        assert_eq!(code(j.route("/api/audit/poll", repo, "claude-code", r#"{"id":7}"#)), Some(404), "no such job");
        assert_eq!(code(j.route("/api/audit/other", repo, "claude-code", "{}")), Some(404));
        assert_eq!(
            code(j.route("/api/audit/start", repo, "claude-code", r#"{"mode":"github","repo":"o/r"}"#)),
            Some(403),
            "a page started without --allow-github never downloads"
        );
        let gh = Jobs::new(true);
        for bad in [
            r#"{"mode":"github"}"#,
            r#"{"mode":"github","repo":"https://evil.example/o/r"}"#,
            r#"{"mode":"github","repo":"o/r@../x"}"#,
            r#"{"mode":"github","repo":"o/r","repos":["~/x"]}"#,
            r#"{"mode":"repo","repo":"o/r"}"#,
        ] {
            assert_eq!(code(gh.route("/api/audit/start", repo, "claude-code", bad)), Some(400), "{bad}");
        }
        assert!(j.lock().is_none() && gh.lock().is_none(), "nothing was started");
    }

    #[test]
    fn a_failed_job_reports_its_error_when_polled() {
        let j = Jobs::default();
        let started =
            j.route("/api/audit/start", Path::new("/no/such/repo/anywhere"), "claude-code", r#"{"mode":"repo"}"#);
        let id = serde_json::from_str::<serde_json::Value>(&started.unwrap()).unwrap()["id"].as_u64().unwrap();
        let mut last = serde_json::Value::Null;
        for _ in 0..200 {
            last = serde_json::from_str(
                &j.route("/api/audit/poll", Path::new("/"), "claude-code", &format!(r#"{{"id":{id}}}"#)).unwrap(),
            )
            .unwrap();
            if last["state"] != "running" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(last["state"], "failed", "{last}");
        assert!(last["error"].as_str().unwrap().contains("no such folder"), "{last}");
        assert!(last.get("result").is_none());
    }
}
