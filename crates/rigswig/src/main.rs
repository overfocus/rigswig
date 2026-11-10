//! rigswig: a harness-agnostic, read-only explorer for coding-agent setups (docs/ROADMAP.md).

mod audits;
mod github;
mod mcp;
mod os_fs;
mod render;
mod ui;
mod writes;

use serde_json::Value;
use std::path::PathBuf;
use std::process::ExitCode;

use rigswig_core::adapter::{Adapter, Ctx, PickReason};
use rigswig_core::model::{Os, SessionScope};
use rigswig_core::read::redact::printable;

fn usage() -> String {
    format!(
        "rigswig {}

Usage:
  rigswig status [--repo <path>] [--tool <id>] [--no-session] [--json]
                                            what a coding-agent session in a repo loads, and where each piece came from
                                            (--tool picks the harness; by default the one with the most evidence, unless
                                            a fresh session artifact names this repo; --no-session keeps the pick
                                            evidence-only; machine-wide activity never moves it; when no repo evidence
                                            picks one, the reading is shown as a machine guess and installed harnesses
                                            are listed)
  rigswig check [--repo <path>] [--tool <id>] [--strict] [--max-tokens <n>] [--max-item-tokens <n>] [--no-warnings] [--json]
                                            for CI: exits 1 when a limit is passed (n may be 40000, 40k or 1m);
                                            --strict also fails on incomplete reads or ambiguous resolution, and
                                            refuses an adapter without that completeness contract; ordinary advisory
                                            warnings need --no-warnings to fail; always uses repository evidence
  rigswig audit [--repo <path>] [--tool <id>] [--json]
                                            ranked findings: what costs context and nothing here references, plugins the
                                            repo's files do not call for, instruction files over budget, repeated text,
                                            dead references and piled-up permission rules; each with a saving and a fix
  rigswig audit [--repo <path>] [--tool <id>] --usage [--days <n>|all] [--json]
                                            the same, with what past sessions here invoked: reads the harness's session
                                            logs (opt-in, this run only), as metadata — see `rigswig usage`
  rigswig audit --repos <path|glob>... [--tool <id>] [--json]
                                            fleet mode: each user-scope item against every repo (--repos '~/code/*')
  rigswig audit|status --github <owner/repo[@ref]> [--tool <id>] [--json]
                                            a public GitHub repo, downloaded as an archive into memory (never onto
                                            disk) and read with an empty home: the one network request rigswig makes,
                                            only when this flag is given
  rigswig audit|status --archive <repo.tar.gz> [--tool <id>] [--json]
                                            the same for an archive you downloaded yourself, with no network
  rigswig usage [--repo <path>] [--tool <id>] [--days <n>|all] [--json]
                                            what past sessions in a repo invoked (agents, skills, MCP servers), how many
                                            sessions, which models and how many tokens, over the last 30 days by
                                            default; read from the harness's own session logs as metadata only —
                                            ids, times, models, token counts and the names of what was called; never
                                            message text, tool inputs or results (docs/USAGE.md). Claude Code for now
  rigswig tools [--repo <path>]             the harnesses rigswig knows, and the evidence for each in this repo
  rigswig schema                            the JSON Schema of `status --json`

  rigswig ui [--repo <path>] [--tool <id>] [--no-session] [--port <n>] [--idle <minutes>] [--allow-writes] [--allow-github]
                                            the same as a local page (stops after --idle minutes, default 30); read-only
                                            unless --allow-writes, which lets it turn plugins and project MCP servers on
                                            and off, with a preview, a backup and undo; --allow-github lets the
                                            page audit public GitHub repos (a download for each one you start)

  rigswig compare <repo-a> <repo-b> [--tool <id>] [--json]
  rigswig compare <repo> --tools <id-a>,<id-b> [--json]
                                            what differs: two repos with one harness, or two harnesses on one repo
  rigswig mcp                               the five read-only surfaces as MCP tools over stdio; add it to a harness's
                                            MCP config as the command `rigswig` with the argument `mcp` (docs/MCP.md)

`status` and `check` take --without <what> (repeatable, or comma-separated): a plugin (name@marketplace), a
layer (layer:user), a kind and name (agent:*, skill:deploy) or an item id. The totals are recomputed without them.

rigswig never calls a model, makes no network request unless a run is given --github, writes nothing unless
`ui --allow-writes` is used, and opens session logs only for `usage` and `audit --usage` (metadata only).
",
        rigswig_core::VERSION
    )
}

fn fail(msg: &str) -> ExitCode {
    eprint!("rigswig: {}\n\n{}", printable(msg), usage());
    ExitCode::from(2)
}

fn this_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::Macos
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else if cfg!(windows) {
        Os::Windows
    } else {
        Os::Other
    }
}

/// The real path of `p`, in the form people and the rest of rigswig expect. On Windows, `canonicalize` returns a
/// verbatim path (`\\?\C:\repo`) in which `/` is not a separator, so joins like `.claude/settings.json` would find
/// nothing; a plain drive path is turned back into `C:\repo`. UNC and other verbatim paths are kept as they are.
fn real_path(p: &std::path::Path) -> std::io::Result<PathBuf> {
    let r = std::fs::canonicalize(p)?;
    if cfg!(windows) {
        let s = r.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                return Ok(PathBuf::from(rest));
            }
        }
    }
    Ok(r)
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).filter(|p| p.is_absolute())
}

/// The user's private folders, `~/.rigswig/private.json` (`{ "private": ["~/notes", "/srv/secret"] }`): rigswig
/// refuses them as a repo, and fleet mode skips them. Empty unless the user writes the file.
fn private_folders(home: &std::path::Path) -> Vec<PathBuf> {
    #[derive(serde::Deserialize)]
    struct File {
        private: Vec<String>,
    }
    let Ok(text) = std::fs::read_to_string(home.join(".rigswig/private.json")) else { return Vec::new() };
    let Ok(f) = serde_json::from_str::<File>(&text) else {
        eprintln!(
            "rigswig: ~/.rigswig/private.json is not {{\"private\": [paths]}}; refusing every repo until it is fixed"
        );
        return vec![PathBuf::from("/")];
    };
    f.private
        .iter()
        .map(|p| match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        })
        .map(|p| real_path(&p).unwrap_or(p))
        .collect()
}

/// An error when `repo` (real) is inside a private folder.
fn refuse_private(repo: &std::path::Path, home: &std::path::Path) -> Result<(), String> {
    if private_folders(home).iter().any(|p| repo.starts_with(p)) {
        return Err("that folder is listed in ~/.rigswig/private.json, so rigswig does not read it".into());
    }
    Ok(())
}

#[derive(PartialEq)]
enum Mode {
    Status,
    Tools,
    Ui,
}

/// A repo that is not on this machine: GitHub (a network request, only when asked) or an archive file (offline).
pub enum Remote {
    Github(github::Spec),
    Archive(PathBuf),
}

impl Remote {
    fn from_flag(flag: &str, value: &str) -> Result<Self, String> {
        match flag {
            "--github" => github::Spec::parse(value).map(Remote::Github),
            _ => Ok(Remote::Archive(PathBuf::from(value))),
        }
    }
}

/// The remote repo as an in-memory tree: the tree, the repo's folder in it, a label, and what unpacking did.
fn load_remote(
    r: &Remote,
    downloaded: &mut dyn FnMut(u64),
) -> Result<(rigswig_core::fs::MemFs, PathBuf, String, String), String> {
    let (bytes, label, name) = match r {
        Remote::Github(spec) => (github::download(spec, github::LIMITS, downloaded)?, spec.label(), spec.repo.clone()),
        Remote::Archive(p) => {
            let real = real_path(p).map_err(|_| format!("no such archive: {}", p.display()))?;
            if let Some(h) = home() {
                refuse_private(&real, &h)?;
            }
            let file = std::fs::File::open(&real).map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(file, github::LIMITS.max_compressed + 1), &mut bytes)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            let file_name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = file_name.trim_end_matches(".tar.gz").trim_end_matches(".tgz").to_string();
            let name: String =
                stem.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '-' }).collect();
            let name = if name.is_empty() || name.chars().all(|c| c == '.') { "archive".to_string() } else { name };
            (bytes, file_name, name)
        }
    };
    let root = PathBuf::from(github::ROOT).join(&name);
    let (fs, rep) = github::unpack(&bytes[..], &root, github::LIMITS)?;
    let summary = format!(
        "{label} was read in memory with an empty home folder, so your own setup is not part of it: {} files, {} links inside the repo kept, {} other links and special files dropped, {} kept by name only (binary or over 1 MB)",
        rep.files, rep.links_kept, rep.links_dropped, rep.kept_empty
    );
    Ok((fs, root, label, summary))
}

/// On stderr, before the one request rigswig makes.
fn say_download(r: &Remote) {
    if let Remote::Github(spec) = r {
        eprintln!("rigswig: downloading {} from codeload.github.com (the only request rigswig makes)", spec.label());
    }
}

/// Run `f` with the harness for a remote repo: `tool`, or the one with the most evidence in it. Returns its result,
/// the label and the unpacking summary.
fn with_remote<R>(
    r: &Remote,
    tool: Option<&str>,
    f: impl FnOnce(&dyn Adapter, &Ctx) -> R,
) -> Result<(R, String, String), String> {
    say_download(r);
    let (fs, root, label, summary) = load_remote(r, &mut |_| {})?;
    let adapters = rigswig_core::adapters::all();
    // Nothing from this machine: no environment variables, an invented home, the archive's own files only.
    let ctx = Ctx {
        fs: &fs,
        env: std::collections::BTreeMap::new(),
        home: PathBuf::from(github::HOME),
        repo: root,
        os: this_os(),
    };
    let a = match tool {
        Some(t) => {
            adapters.iter().find(|a| a.id() == t).ok_or(format!("no harness called {t} (see `rigswig tools`)"))?
        }
        None => adapters.iter().rev().max_by_key(|a| a.detect(&ctx).score).expect("an adapter"),
    };
    Ok((f(a.as_ref(), &ctx), label, summary))
}

fn status(args: &[String], mode: Mode) -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut json = false;
    let mut tool: Option<String> = None;
    let mut port: u16 = 0;
    let mut idle_minutes: u64 = 30;
    let mut removals: Vec<String> = Vec::new();
    let mut allow_writes = false;
    let mut allow_github = false;
    let mut no_session = false;
    let mut remote: Option<Remote> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--no-session" => no_session = true,
            "--repo" => match it.next() {
                Some(p) if !p.starts_with("--") => repo = Some(PathBuf::from(p)),
                _ => return fail("--repo needs a path"),
            },
            flag @ ("--github" | "--archive") if mode == Mode::Status => match it.next() {
                Some(v) if !v.starts_with("--") => match Remote::from_flag(flag, v) {
                    Ok(r) => remote = Some(r),
                    Err(e) => return fail(&e),
                },
                _ => return fail(&format!("{flag} needs a value")),
            },
            "--tool" => match it.next() {
                Some(t) if !t.starts_with("--") => tool = Some(t.clone()),
                _ => return fail("--tool needs a harness id (see `rigswig tools`)"),
            },
            "--without" if mode == Mode::Status => match it.next() {
                Some(w) if !w.starts_with("--") => {
                    removals.extend(w.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()))
                }
                _ => return fail("--without needs what to take out"),
            },
            "--allow-writes" if mode == Mode::Ui => allow_writes = true,
            "--allow-github" if mode == Mode::Ui => {
                if !cfg!(feature = "github") {
                    return fail("this rigswig was built without the github feature, so the page cannot download");
                }
                allow_github = true
            }
            "--port" if mode == Mode::Ui => match it.next().and_then(|p| p.parse().ok()) {
                Some(p) => port = p,
                None => return fail("--port needs a number"),
            },
            "--idle" if mode == Mode::Ui => match it.next().and_then(|p| p.parse().ok()).filter(|m| *m > 0) {
                Some(m) => idle_minutes = m,
                None => return fail("--idle needs a number of minutes"),
            },
            other => return fail(&format!("unknown option: {other}")),
        }
    }
    if let Some(r) = &remote {
        if repo.is_some() {
            return fail("use --repo for a folder here, or --github / --archive for one that is not, not both");
        }
        let (mut h, label, summary) = match with_remote(r, tool.as_deref(), |a, ctx| a.resolve(ctx)) {
            Ok(x) => x,
            Err(e) => return fail(&e),
        };
        h.target.repo = printable(&label);
        if !removals.is_empty() {
            h = rigswig_core::whatif::without(&h, &removals).0;
        }
        eprintln!("rigswig: {}", printable(&summary));
        if json {
            println!("{}", serde_json::to_string_pretty(&h).expect("the model serialises"));
        } else {
            print!("{}", render::text(&h));
        }
        return ExitCode::SUCCESS;
    }
    let repo = match repo.map_or_else(std::env::current_dir, Ok).and_then(|p| real_path(&p)) {
        Ok(r) => r,
        Err(_) => return fail("that repo folder does not exist"),
    };
    let Some(home) = home() else { return fail("the home folder is not set") };
    if let Err(e) = refuse_private(&repo, &home) {
        return fail(&e);
    }

    let adapters = rigswig_core::adapters::all();
    let ctx_for = |a: &dyn Adapter| {
        let env = a.env_vars().iter().filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v))).collect();
        Ctx { fs: &os_fs::OsFs, env, home: home.clone(), repo: repo.clone(), os: this_os() }
    };
    let scored: Vec<(&dyn Adapter, rigswig_core::adapter::Detection)> =
        adapters.iter().map(|a| (a.as_ref(), a.detect(&ctx_for(a.as_ref())))).collect();

    if mode == Mode::Tools {
        match detections(&repo) {
            Ok(list) => {
                for row in list {
                    let ev =
                        if row.evidence.is_empty() { "no sign of it here".into() } else { row.evidence.join(", ") };
                    let live = row
                        .session
                        .map(|c| {
                            let scope = match c.scope {
                                SessionScope::Repo => "live session (this repo)",
                                SessionScope::Machine => "live session (this machine)",
                            };
                            format!("  · {scope}")
                        })
                        .unwrap_or_default();
                    println!("{:<12} {:>3}  {}{}", row.id, row.score, ev, printable(&live));
                }
            }
            Err(e) => return fail(&e),
        }
        return ExitCode::SUCCESS;
    }

    // The default pick, shared by the page and the interactive reading: `--tool` is explicit; otherwise the harness
    // with the most evidence — unless a fresh session artifact names this repo. Machine-wide activity never moves
    // the pick, and `--no-session` keeps the evidence-only order.
    let pick = match rigswig_core::adapter::pick(
        &scored,
        &rigswig_core::adapter::PickCtx { fs: &os_fs::OsFs, home: home.clone(), repo: repo.clone(), os: this_os() },
        &env_of,
        tool.as_deref(),
        !no_session,
        std::time::SystemTime::now(),
    ) {
        Ok(p) => p,
        Err(e) => return fail(&e),
    };

    if mode == Mode::Ui {
        if let Some(t) = &tool {
            if !scored.iter().any(|(a, _)| a.id() == t) {
                return fail(&format!("no harness called {t} (see `rigswig tools`)"));
            }
        }
        let default = tool.clone().unwrap_or_else(|| pick.adapter.id().to_string());
        let choices: Vec<ui::ToolChoice> = scored
            .iter()
            .map(|(a, d)| ui::ToolChoice {
                id: a.id().into(),
                score: d.score,
                evidence: d.evidence.iter().map(|e| printable(e)).collect(),
            })
            .collect();
        let render = |t: Option<&str>, vs: Option<&str>| {
            let find = |id: &str| scored.iter().find(|(a, _)| a.id() == id).map(|(a, _)| *a);
            let a = find(t.unwrap_or(&default))?;
            let mut h = a.resolve(&ctx_for(a));
            // The page's default harness follows the same pick the status bar would; the claim rides along only
            // when the pick followed a live session, never on an explicitly named tool.
            if t.is_none() {
                h = with_session(h, pick.session.as_ref());
            }
            let other = match vs {
                Some(v) => Some(find(v)?).map(|b| b.resolve(&ctx_for(b))),
                None => None,
            };
            let comparison = other.as_ref().map(|o| rigswig_core::compare::compare(&h, o));
            let tools = choices
                .iter()
                .map(|c| ui::ToolChoice { id: c.id.clone(), score: c.score, evidence: c.evidence.clone() })
                .collect();
            let page = ui::PageData {
                harness: &h,
                other: other.as_ref(),
                comparison,
                tools,
                version: rigswig_core::VERSION,
                writes: allow_writes,
                github: allow_github,
            };
            Some(ui::embed_json(&page))
        };
        // Write mode: plan, apply and undo, with the changes applied in this run kept for undo (newest last).
        let journal: std::sync::Mutex<Vec<writes::Applied>> = std::sync::Mutex::new(Vec::new());
        let backups = home.join(".rigswig/backups");
        let act = |route: &str, t: Option<&str>, body: &str| -> Result<String, (u16, String)> {
            let id = t.unwrap_or(&default);
            let a = scored
                .iter()
                .find(|(a, _)| a.id() == id)
                .map(|(a, _)| *a)
                .ok_or((404, "no such harness".to_string()))?;
            let ctx = ctx_for(a);
            let bad = |e: serde_json::Error| (400, format!("bad request: {e}"));
            match route {
                "/api/plan" => {
                    let change: rigswig_core::change::Change = serde_json::from_str(body).map_err(bad)?;
                    let plan = a.plan(&ctx, &change).map_err(|e| (409, e))?;
                    Ok(serde_json::to_string(&plan).expect("serialises"))
                }
                "/api/apply" => {
                    #[derive(serde::Deserialize)]
                    #[serde(rename_all = "camelCase", deny_unknown_fields)]
                    struct Apply {
                        change: rigswig_core::change::Change,
                        before_hash: Option<String>,
                    }
                    let req: Apply = serde_json::from_str(body).map_err(bad)?;
                    let plan = a.plan(&ctx, &req.change).map_err(|e| (409, e))?;
                    let done = writes::apply(&plan, req.before_hash.as_deref(), &backups).map_err(|e| (409, e))?;
                    let out = serde_json::json!({
                        "applied": done.file,
                        "backup": done.backup.as_ref().map(|b| printable(&b.to_string_lossy())),
                        "tokensAfter": plan.tokens_after,
                        "takesEffect": plan.takes_effect,
                    });
                    journal.lock().expect("journal").push(done);
                    Ok(out.to_string())
                }
                "/api/undo" => {
                    let mut j = journal.lock().expect("journal");
                    let last = j.pop().ok_or((409, "nothing to undo in this run of the page".to_string()))?;
                    match writes::undo(&last) {
                        Ok(()) => Ok(serde_json::json!({ "undone": last.file }).to_string()),
                        Err(e) => {
                            j.push(last);
                            Err((409, e))
                        }
                    }
                }
                _ => Err((404, "no such action".into())),
            }
        };
        let idle = std::time::Duration::from_secs(idle_minutes * 60);
        let act_ref: Option<&ui::Act> = if allow_writes { Some(&act) } else { None };
        // Audits read only, so they run without write mode: one at a time, on their own thread, polled by the page.
        let audit_jobs = audits::Jobs::new(allow_github);
        let jobs = |route: &str, t: Option<&str>, body: &str| -> Result<String, (u16, String)> {
            let id = t.unwrap_or(&default);
            if !scored.iter().any(|(a, _)| a.id() == id) {
                return Err((404, "no such harness".into()));
            }
            audit_jobs.route(route, &repo, id, body)
        };
        let started = ui::serve(port, idle, &render, act_ref, Some(&jobs), &|url| {
            if allow_writes {
                println!("rigswig ui (WRITE MODE, local only): {url}");
                println!(
                    "The page can turn plugins and project MCP servers on and off; backups go to ~/.rigswig/backups."
                );
            } else {
                println!("rigswig ui (read-only, local only): {url}");
            }
            if allow_github {
                println!(
                    "GitHub audits are on: each one the page starts downloads that public repo from codeload.github.com."
                );
            }
            println!("Stops after {idle_minutes} minutes without a request, or with Ctrl-C.");
        });
        return match started {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&format!("could not start the page: {e}")),
        };
    }

    // The pick is made; the reading explains why. `--tool` is explicit; the machine-guess case (nothing in the
    // repo points at any harness we read) prints the ranked evidence before the reading.
    let guess = pick.reason == PickReason::Guess;
    if guess && !json {
        let read: Vec<_> = scored.iter().filter(|(_, d)| d.score > 0).collect();
        if read.is_empty() {
            println!("Nothing points at a harness rigswig reads here.");
            println!(
                "rigswig status --tool <id> reads one directly: {}",
                scored.iter().map(|(a, _)| a.id()).collect::<Vec<_>>().join(", ")
            );
            return ExitCode::SUCCESS;
        }
        println!("Picked by this machine's evidence (the repo is set up for no harness we read):");
        let mut ranked = read.clone();
        ranked.sort_by_key(|(_, d)| std::cmp::Reverse(d.score));
        for (a, d) in &ranked {
            let ev = if d.evidence.is_empty() {
                String::new()
            } else {
                d.evidence.iter().map(|e| printable(e)).collect::<Vec<_>>().join(", ")
            };
            if a.id() == pick.adapter.id() {
                println!("   {:<11} {:>2}  {}  (reading this one)", a.id(), d.score, ev);
            } else {
                println!("   {:<11} {:>2}  {}", a.id(), d.score, ev);
            }
        }
        println!("Rerun with --tool <id> to read a different one.");
    }
    let mut h = pick.adapter.resolve(&ctx_for(pick.adapter));
    h = with_session(h, pick.session.as_ref());
    if !removals.is_empty() {
        let (w, counts) = rigswig_core::whatif::without(&h, &removals);
        for (pattern, n) in counts {
            if n == 0 {
                eprintln!("rigswig: --without {}: nothing in effect matches", printable(&pattern));
            }
        }
        h = w;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&h).expect("the model serialises"));
    } else {
        print!("{}", render::text(&h));
        // The guess case's ranked list already names every installed harness; the quiet pick's footer
        // lists the others it edged out.
        let others: Vec<&str> =
            scored.iter().filter(|(a, d)| a.id() != pick.adapter.id() && d.score > 0).map(|(a, _)| a.id()).collect();
        if !others.is_empty() && !guess {
            println!("  Also configured here: {} (rigswig status --tool <id>)", others.join(", "));
        }
    }
    ExitCode::SUCCESS
}

// Picking a harness: the one with the most evidence; on a tie, the first in `adapters::all()` (Claude Code, then
// Codex). `max_by_key` keeps the last of equal maximums, so every pick runs over the list reversed.

/// Run `f` with `tool` (or the default pick) and its context for `repo`. `f` also gets the adapter's detection
/// score. `session`: whether a live session's claim moves the default pick (never an explicit `--tool`).
fn with_harness<R>(
    repo: &std::path::Path,
    tool: Option<&str>,
    session: bool,
    f: impl FnOnce(&dyn Adapter, &Ctx, u8) -> R,
) -> Result<(R, Option<rigswig_core::adapter::SessionClaim>), String> {
    let repo = real_path(repo).map_err(|_| format!("no such folder: {}", repo.display()))?;
    let home = home().ok_or("the home folder is not set")?;
    refuse_private(&repo, &home)?;
    let adapters = rigswig_core::adapters::all();
    let ctx_for = |a: &dyn Adapter| {
        let env = a.env_vars().iter().filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v))).collect();
        Ctx { fs: &os_fs::OsFs, env, home: home.clone(), repo: repo.clone(), os: this_os() }
    };
    let scored: Vec<(&dyn Adapter, rigswig_core::adapter::Detection)> =
        adapters.iter().map(|a| (a.as_ref(), a.detect(&ctx_for(a.as_ref())))).collect();
    let base =
        rigswig_core::adapter::PickCtx { fs: &os_fs::OsFs, home: home.clone(), repo: repo.clone(), os: this_os() };
    let pick = rigswig_core::adapter::pick(&scored, &base, &env_of, tool, session, std::time::SystemTime::now())?;
    let ctx = ctx_for(pick.adapter);
    let r = f(pick.adapter, &ctx, pick.score);
    Ok((r, pick.session))
}

/// The environment one adapter's probe reads, filtered exactly as its resolver's is.
fn env_of(a: &dyn Adapter) -> std::collections::BTreeMap<String, String> {
    a.env_vars().iter().filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v))).collect()
}

/// Put a live-session claim on a harness — only the one the default pick followed, never an explicit `--tool`.
fn with_session(
    mut h: rigswig_core::model::Harness,
    session: Option<&rigswig_core::adapter::SessionClaim>,
) -> rigswig_core::model::Harness {
    if let Some(c) = session {
        h.tool.session = Some(rigswig_core::model::Session { scope: c.scope, evidence: c.evidence.clone() });
    }
    h
}

/// Resolve `tool` (or the default pick) for `repo`; the result carries the live-session claim when the pick
/// followed one.
fn resolve(repo: &std::path::Path, tool: Option<&str>, session: bool) -> Result<rigswig_core::model::Harness, String> {
    let (h, claim) = with_harness(repo, tool, session, |a, ctx, _| a.resolve(ctx))?;
    Ok(with_session(h, claim.as_ref()))
}

/// Resolve a harness for `check`, refusing an adapter which has not opted into the incomplete-read contract when
/// `strict` was requested. Like every automated read, this never follows live-session activity.
pub(crate) fn resolve_check(
    repo: &std::path::Path,
    tool: Option<&str>,
    strict: bool,
) -> Result<rigswig_core::model::Harness, String> {
    let (out, _) = with_harness(repo, tool, false, |a, ctx, _| {
        if strict && !a.strict_ready() {
            Err(format!("{} does not support --strict completeness checking yet", a.id()))
        } else {
            Ok(a.resolve(ctx))
        }
    })?;
    out
}

/// `~/x` and `*` in the last part of a path: `~/code/*` is every folder in `~/code`.
fn expand(pattern: &str) -> Result<Vec<PathBuf>, String> {
    let p = match pattern.strip_prefix("~/") {
        Some(rest) => home().ok_or("the home folder is not set")?.join(rest),
        None => PathBuf::from(pattern),
    };
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains('*') {
        return Ok(vec![p]);
    }
    if p.parent().is_some_and(|d| d.to_string_lossy().contains('*')) {
        return Err(format!("only the last part of a path may use *: {pattern}"));
    }
    let dir = p.parent().map(PathBuf::from).unwrap_or_default();
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|_| format!("no such folder: {}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.starts_with('.') && rigswig_core::whatif::glob(&name, &n)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    Ok(out)
}

/// `rigswig audit`: findings for one repo, or fleet mode across many.
/// The `--days` value: a number of days, or `all`.
fn parse_days(v: &str) -> Result<Option<u64>, String> {
    if v == "all" {
        return Ok(None);
    }
    v.parse().ok().filter(|d| *d > 0).map(Some).ok_or("--days needs a number of days, or all".into())
}

/// The window's start for the last `days` days, as an ISO 8601 date (UTC); `None` for every log there is. A
/// `--days` older than the calendar begins is every log there is too, not an overflow.
fn since(days: Option<u64>) -> Option<String> {
    let days = days?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    // `--days` is any u64; a window it puts before the calendar begins is every log there is.
    let start = ((now / 86_400) as i64).checked_sub(i64::try_from(days).unwrap_or(i64::MAX))?;
    // 719_468 days before 1970-01-01 is the year 0 the calendar function starts at.
    if start < -719_468 {
        return None;
    }
    let (y, m, d) = civil_from_days(start);
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm, public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Said on stderr before any session log is opened.
fn say_usage(since: Option<&str>) {
    eprintln!(
        "rigswig: reading session logs for this repo{} — metadata only: ids, times, models, token counts and the names of what was called; never message text, tool inputs or results",
        since.map_or_else(String::new, |s| format!(" since {s}"))
    );
}

fn usage_cmd(args: &[String]) -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut tool: Option<String> = None;
    let mut json = false;
    let mut days: Option<u64> = Some(30);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value =
            |name: &str| it.next().filter(|v| !v.starts_with("--")).cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--json" => json = true,
            "--repo" => match value("--repo") {
                Ok(v) => repo = Some(PathBuf::from(v)),
                Err(e) => return fail(&e),
            },
            "--tool" => match value("--tool") {
                Ok(v) => tool = Some(v),
                Err(e) => return fail(&e),
            },
            "--days" => match value("--days").and_then(|v| parse_days(&v)) {
                Ok(d) => days = d,
                Err(e) => return fail(&e),
            },
            other => return fail(&format!("unknown option: {other}")),
        }
    }
    let here = repo.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let since = since(days);
    say_usage(since.as_deref());
    let u = match with_harness(&here, tool.as_deref(), false, |a, ctx, _| a.usage(ctx, since.as_deref())) {
        Ok((Ok(u), _)) => u,
        Ok((Err(e), _)) | Err(e) => return fail(&e),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&u).expect("serialises"));
    } else {
        print!("{}", render::usage_text(&u));
    }
    ExitCode::SUCCESS
}

fn audit(args: &[String]) -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut repos: Vec<String> = Vec::new();
    let mut tool: Option<String> = None;
    let mut json = false;
    let mut with_usage = false;
    let mut days: Option<u64> = Some(30);
    let mut remote: Option<Remote> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value =
            |name: &str| it.next().filter(|v| !v.starts_with("--")).cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--json" => json = true,
            "--no-session" => {} // retained for compatibility: audits always ignore session activity
            "--repo" => match value("--repo") {
                Ok(v) => repo = Some(PathBuf::from(v)),
                Err(e) => return fail(&e),
            },
            flag @ ("--github" | "--archive") => match value(flag).and_then(|v| Remote::from_flag(flag, &v)) {
                Ok(r) if remote.is_none() => remote = Some(r),
                Ok(_) => return fail("one --github or --archive per run"),
                Err(e) => return fail(&e),
            },
            "--repos" => match value("--repos") {
                Ok(v) => repos.push(v),
                Err(e) => return fail(&e),
            },
            "--tool" => match value("--tool") {
                Ok(v) => tool = Some(v),
                Err(e) => return fail(&e),
            },
            "--usage" => with_usage = true,
            "--days" => match value("--days").and_then(|v| parse_days(&v)) {
                Ok(d) => days = d,
                Err(e) => return fail(&e),
            },
            // A shell that expands `--repos ~/code/*` passes the folders as separate words.
            other if !other.starts_with("--") && !repos.is_empty() => repos.push(other.to_string()),
            other => return fail(&format!("unknown option: {other}")),
        }
    }
    if repo.is_some() && !repos.is_empty() {
        return fail("use --repo for one repo or --repos for many, not both");
    }
    if with_usage && (remote.is_some() || !repos.is_empty()) {
        return fail(
            "--usage reads this machine's session logs for one repo here: not with --github, --archive or --repos",
        );
    }
    if !with_usage && args.iter().any(|a| a == "--days") {
        return fail("--days sets the --usage window: add --usage");
    }
    if let Some(r) = &remote {
        if repo.is_some() || !repos.is_empty() {
            return fail("--github and --archive audit one repo that is not here: not with --repo or --repos");
        }
        say_download(r);
        let a = match audits::remote(r, tool.as_deref(), &mut |_| {}) {
            Ok(a) => a,
            Err(e) => return fail(&e),
        };
        if json {
            println!("{}", serde_json::to_string_pretty(&a).expect("serialises"));
        } else {
            print!("{}", render::audit_text(&a));
        }
        return ExitCode::SUCCESS;
    }
    let quiet = &mut |_: rigswig_core::audit::Progress| {};
    if repos.is_empty() {
        let here = repo.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let since = since(days);
        if with_usage {
            say_usage(since.as_deref());
        }
        let a = match audits::repo(&here, tool.as_deref(), false, with_usage.then_some(since.as_deref()), quiet) {
            Ok((a, _)) => a,
            Err(e) => return fail(&e),
        };
        if json {
            println!("{}", serde_json::to_string_pretty(&a).expect("serialises"));
        } else {
            print!("{}", render::audit_text(&a));
        }
        return ExitCode::SUCCESS;
    }

    let run = match audits::fleet(&repos, tool.as_deref(), quiet) {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    for e in &run.errors {
        eprintln!("rigswig: {}", printable(e));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&run.fleet).expect("serialises"));
    } else {
        print!("{}", render::fleet_text(&run.fleet));
        if !run.skipped.is_empty() {
            println!("  Skipped (no harness found): {}", printable(&run.skipped.join(", ")));
        }
        if run.private_skipped > 0 {
            println!("  Skipped {} private folder(s) (~/.rigswig/private.json)", run.private_skipped);
        }
    }
    ExitCode::SUCCESS
}

/// `check`'s verdict rows as one JSON object (`tool`, `repo`, `passed`, `checks`), shared with the MCP `check`.
pub(crate) fn check_json(
    h: &rigswig_core::model::Harness,
    max_tokens: Option<u64>,
    max_item: Option<u64>,
    no_warnings: bool,
    strict: bool,
) -> Value {
    // (name, passed, detail)
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let errs = rigswig_core::check::contract_errors(h);
    results.push((
        "contract".into(),
        errs.is_empty(),
        if errs.is_empty() { "the output keeps the contract".into() } else { errs.join("; ") },
    ));
    if strict {
        let ws: Vec<String> = h
            .warnings
            .iter()
            .filter(|w| w.is_incomplete())
            .map(|w| {
                let file = w.file.as_deref().map(|f| format!(" ({f})")).unwrap_or_default();
                format!("{}: {}{file}", printable(&w.code), printable(&w.message))
            })
            .collect();
        let detail =
            if ws.is_empty() { "no incomplete reads or ambiguous resolution reported".into() } else { ws.join("; ") };
        results.push(("strict".into(), ws.is_empty(), detail));
    }
    let t = &h.context.tokens;
    if let Some(max) = max_tokens {
        results.push((
            "max-tokens".into(),
            t.value <= max,
            format!("~{} estimated tokens from startup files ({}..{}); limit {max}", t.value, t.low, t.high),
        ));
    }
    if let Some(max) = max_item {
        let over: Vec<String> = h
            .items
            .iter()
            .filter(|i| {
                i.effective
                    && i.load.when == rigswig_core::model::LoadWhen::Startup
                    && i.load.chars.value.div_ceil(4) > max
            })
            .map(|i| format!("{} {} (~{})", i.detail.kind(), printable(&i.name), i.load.chars.value.div_ceil(4)))
            .collect();
        let detail = if over.is_empty() {
            format!("no item over {max} tokens")
        } else {
            format!("over {max} tokens: {}", over.join(", "))
        };
        results.push(("max-item-tokens".into(), over.is_empty(), detail));
    }
    if no_warnings {
        let ws: Vec<String> =
            h.warnings.iter().filter(|w| w.code != "what-if").map(|w| printable(&w.message)).collect();
        let detail = if ws.is_empty() { "no warnings".into() } else { ws.join("; ") };
        results.push(("no-warnings".into(), ws.is_empty(), detail));
    }
    let passed = results.iter().all(|(_, ok, _)| *ok);
    let rows: Vec<Value> =
        results.iter().map(|(n, ok, d)| serde_json::json!({ "check": n, "passed": ok, "detail": d })).collect();
    serde_json::json!({ "tool": h.tool.id, "repo": h.target.repo, "passed": passed, "checks": rows })
}

/// One row of the `tools` surface: the detection for one harness, and any fresh live-session claim it makes.
pub(crate) struct ToolRow {
    pub id: String,
    pub score: u8,
    pub evidence: Vec<String>,
    pub session: Option<rigswig_core::adapter::SessionClaim>,
}

/// Every harness with its detection for one repo: the `rigswig tools` surface, text and MCP alike.
pub(crate) fn detections(repo: &std::path::Path) -> Result<Vec<ToolRow>, String> {
    let repo = real_path(repo).map_err(|_| format!("no such folder: {}", repo.display()))?;
    let home = home().ok_or("the home folder is not set")?;
    refuse_private(&repo, &home)?;
    let adapters = rigswig_core::adapters::all();
    let ctx_for = |a: &dyn Adapter| {
        let env = a.env_vars().iter().filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v))).collect();
        Ctx { fs: &os_fs::OsFs, env, home: home.clone(), repo: repo.clone(), os: this_os() }
    };
    Ok(adapters
        .iter()
        .map(|a| {
            let ctx = ctx_for(a.as_ref());
            let d = a.detect(&ctx);
            let now = std::time::SystemTime::now();
            let session = a.session(&ctx).filter(|c| c.fresh(now));
            ToolRow {
                id: a.id().to_string(),
                score: d.score,
                evidence: d.evidence.iter().map(|e| printable(e)).collect(),
                session,
            }
        })
        .collect())
}

/// `rigswig check`: limits for CI. Exit 0 when every limit holds, 1 when one does not, 2 on a usage error.
fn check(args: &[String]) -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut tool: Option<String> = None;
    let mut max_tokens: Option<u64> = None;
    let mut max_item: Option<u64> = None;
    let mut no_warnings = false;
    let mut strict = false;
    let mut json = false;
    let mut removals: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value =
            |name: &str| it.next().filter(|v| !v.starts_with("--")).cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--json" => json = true,
            "--no-warnings" => no_warnings = true,
            "--strict" => strict = true,
            "--no-session" => {} // retained for compatibility: checks always ignore session activity
            "--repo" => match value("--repo") {
                Ok(v) => repo = Some(PathBuf::from(v)),
                Err(e) => return fail(&e),
            },
            "--tool" => match value("--tool") {
                Ok(v) => tool = Some(v),
                Err(e) => return fail(&e),
            },
            "--without" => match value("--without") {
                Ok(v) => removals.extend(v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty())),
                Err(e) => return fail(&e),
            },
            "--max-tokens" | "--max-item-tokens" => {
                let name = a.clone();
                let Some(n) = value(&name).ok().and_then(|v| rigswig_core::whatif::parse_tokens(&v)) else {
                    return fail(&format!("{name} needs a number of tokens, like 40000 or 40k"));
                };
                if name == "--max-tokens" { max_tokens = Some(n) } else { max_item = Some(n) }
            }
            other => return fail(&format!("unknown option: {other}")),
        }
    }
    let here = repo.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let mut h = match resolve_check(&here, tool.as_deref(), strict) {
        Ok(h) => h,
        Err(e) => return fail(&e),
    };
    if !removals.is_empty() {
        h = rigswig_core::whatif::without(&h, &removals).0;
    }

    let out = check_json(&h, max_tokens, max_item, no_warnings, strict);
    let passed = out["passed"].as_bool().unwrap_or(false);
    if json {
        println!("{}", serde_json::to_string_pretty(&out).expect("serialises"));
    } else {
        println!("rigswig check: {} in {}", printable(&h.tool.id), printable(&h.target.repo));
        for row in out["checks"].as_array().expect("the rows") {
            println!(
                "  {} {:<16} {}",
                if row["passed"].as_bool().unwrap_or(false) { "PASS" } else { "FAIL" },
                row["check"].as_str().unwrap_or_default(),
                row["detail"].as_str().unwrap_or_default()
            );
        }
    }
    if passed { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn compare(args: &[String]) -> ExitCode {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut tool: Option<String> = None;
    let mut tools: Option<(String, String)> = None;
    let mut json = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--tool" => match it.next() {
                Some(t) if !t.starts_with("--") => tool = Some(t.clone()),
                _ => return fail("--tool needs a harness id"),
            },
            "--tools" => match it.next().and_then(|t| t.split_once(',')) {
                Some((x, y)) => tools = Some((x.trim().into(), y.trim().into())),
                None => return fail("--tools needs two harness ids, like claude-code,codex"),
            },
            other if other.starts_with("--") => return fail(&format!("unknown option: {other}")),
            path => paths.push(PathBuf::from(path)),
        }
    }
    let (a, b) = match (paths.as_slice(), &tools) {
        ([one], Some((x, y))) => (resolve(one, Some(x), false), resolve(one, Some(y), false)),
        ([], Some((x, y))) => {
            let here = std::env::current_dir().unwrap_or_default();
            (resolve(&here, Some(x), false), resolve(&here, Some(y), false))
        }
        ([one, two], None) => (resolve(one, tool.as_deref(), false), resolve(two, tool.as_deref(), false)),
        _ => return fail("compare needs two repos, or one repo and --tools <a>,<b>"),
    };
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return fail(&e),
    };
    let c = rigswig_core::compare::compare(&a, &b);
    if json {
        println!("{}", serde_json::to_string_pretty(&c).expect("serialises"));
    } else {
        print!("{}", render::compare_text(&c));
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    // A closed pipe ends the process here (exit 141), as on every unix CLI; std leaves SIGPIPE ignored, so a
    // `rigswig ... | head` panics mid-print on the first full stdout buffer instead.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("{}", rigswig_core::VERSION);
            ExitCode::SUCCESS
        }
        None | Some("--help" | "-h") => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some("schema") => {
            println!("{}", serde_json::to_string_pretty(&rigswig_core::json_schema()).expect("a schema serialises"));
            ExitCode::SUCCESS
        }
        Some("status") => status(&args[1..], Mode::Status),
        Some("tools") => status(&args[1..], Mode::Tools),
        Some("ui") => status(&args[1..], Mode::Ui),
        Some("mcp") => mcp::serve(&args[1..]),
        Some("compare") => compare(&args[1..]),
        Some("check") => check(&args[1..]),
        Some("audit") => audit(&args[1..]),
        Some("usage") => usage_cmd(&args[1..]),
        Some(other) => fail(&format!("unknown or unimplemented command: {other}")),
    }
}

#[cfg(test)]
mod days {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(civil_from_days(100_000), (2243, 10, 17));
        assert_eq!(civil_from_days(-719_162), (1, 1, 1));
    }

    /// A huge `--days` is every log there is — `since` must answer `None`, not overflow.
    #[test]
    fn a_window_older_than_the_calendar_is_none() {
        assert_eq!(since(Some(u64::MAX)), None);
        assert_eq!(since(Some(9_223_372_036_854_775_808)), None);
        assert_eq!(since(Some(1_000_000)), None);
        assert_eq!(since(Some(30)).map(|s| s.len()), Some(10), "an ordinary window still names a date");
        assert_eq!(since(None), None);
    }
}
