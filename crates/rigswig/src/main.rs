//! rigswig: a harness-agnostic, read-only explorer for coding-agent setups (docs/ROADMAP.md).

mod os_fs;
mod render;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use rigswig_core::adapter::{Adapter, Ctx};
use rigswig_core::model::Os;
use rigswig_core::read::redact::printable;

fn usage() -> String {
    format!(
        "rigswig {}

Usage:
  rigswig status [--repo <path>] [--tool <id>] [--json]
                                            what a coding-agent session in a repo loads, and where each piece came from
                                            (--tool picks the harness; by default the one with the most evidence)
  rigswig tools [--repo <path>]             the harnesses rigswig knows, and the evidence for each in this repo
  rigswig schema                            the JSON Schema of `status --json`

  rigswig ui [--repo <path>] [--tool <id>] [--port <n>] [--idle <minutes>]
                                            the same as a local, read-only page (stops after --idle minutes, default 30)

  rigswig compare <repo-a> <repo-b> [--tool <id>] [--json]
  rigswig compare <repo> --tools <id-a>,<id-b> [--json]
                                            what differs: two repos with one harness, or two harnesses on one repo

rigswig never makes a network request, calls a model, or writes a file.
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

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).filter(|p| p.is_absolute())
}

#[derive(PartialEq)]
enum Mode {
    Status,
    Tools,
    Ui,
}

fn status(args: &[String], mode: Mode) -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut json = false;
    let mut tool: Option<String> = None;
    let mut port: u16 = 0;
    let mut idle_minutes: u64 = 30;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--repo" => match it.next() {
                Some(p) if !p.starts_with("--") => repo = Some(PathBuf::from(p)),
                _ => return fail("--repo needs a path"),
            },
            "--tool" => match it.next() {
                Some(t) if !t.starts_with("--") => tool = Some(t.clone()),
                _ => return fail("--tool needs a harness id (see `rigswig tools`)"),
            },
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
    let repo = match repo.map_or_else(std::env::current_dir, Ok).and_then(std::fs::canonicalize) {
        Ok(r) => r,
        Err(_) => return fail("that repo folder does not exist"),
    };
    let Some(home) = home() else { return fail("the home folder is not set") };

    let adapters = rigswig_core::adapters::all();
    let ctx_for = |a: &dyn Adapter| {
        let env = a.env_vars().iter().filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v))).collect();
        Ctx { fs: &os_fs::OsFs, env, home: home.clone(), repo: repo.clone(), os: this_os() }
    };
    let scored: Vec<(&dyn Adapter, rigswig_core::adapter::Detection)> =
        adapters.iter().map(|a| (a.as_ref(), a.detect(&ctx_for(a.as_ref())))).collect();

    if mode == Mode::Tools {
        for (a, d) in &scored {
            let ev = if d.evidence.is_empty() { "no sign of it here".into() } else { d.evidence.join(", ") };
            println!("{:<12} {:>3}  {}", a.id(), d.score, printable(&ev));
        }
        return ExitCode::SUCCESS;
    }

    if mode == Mode::Ui {
        if let Some(t) = &tool {
            if !scored.iter().any(|(a, _)| a.id() == t) {
                return fail(&format!("no harness called {t} (see `rigswig tools`)"));
            }
        }
        let default = tool
            .clone()
            .unwrap_or_else(|| scored.iter().max_by_key(|(_, d)| d.score).expect("an adapter").0.id().to_string());
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
            let h = a.resolve(&ctx_for(a));
            let other = match vs {
                Some(v) => Some(find(v)?).map(|b| b.resolve(&ctx_for(b))),
                None => None,
            };
            let comparison = other.as_ref().map(|o| rigswig_core::compare::compare(&h, o));
            let tools = choices
                .iter()
                .map(|c| ui::ToolChoice { id: c.id.clone(), score: c.score, evidence: c.evidence.clone() })
                .collect();
            let page =
                ui::PageData { harness: &h, other: other.as_ref(), comparison, tools, version: rigswig_core::VERSION };
            Some(ui::embed_json(&page))
        };
        let idle = std::time::Duration::from_secs(idle_minutes * 60);
        let started = ui::serve(port, idle, &render, &|url| {
            println!("rigswig ui (read-only, local only): {url}");
            println!("Stops after {idle_minutes} minutes without a request, or with Ctrl-C.");
        });
        return match started {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => fail(&format!("could not start the page: {e}")),
        };
    }

    let chosen = match &tool {
        Some(t) => match scored.iter().find(|(a, _)| a.id() == t) {
            Some(s) => s,
            None => return fail(&format!("no harness called {t} (see `rigswig tools`)")),
        },
        None => scored.iter().max_by_key(|(_, d)| d.score).expect("at least one adapter"),
    };
    let h = chosen.0.resolve(&ctx_for(chosen.0));
    if json {
        println!("{}", serde_json::to_string_pretty(&h).expect("the model serialises"));
    } else {
        print!("{}", render::text(&h));
        let others: Vec<&str> =
            scored.iter().filter(|(a, d)| a.id() != chosen.0.id() && d.score > 0).map(|(a, _)| a.id()).collect();
        if !others.is_empty() {
            println!("  Also configured here: {} (rigswig status --tool <id>)", others.join(", "));
        }
    }
    ExitCode::SUCCESS
}

/// Resolve `tool` (or the adapter with the most evidence) for `repo`.
fn resolve(repo: &std::path::Path, tool: Option<&str>) -> Result<rigswig_core::model::Harness, String> {
    let repo = std::fs::canonicalize(repo).map_err(|_| format!("no such folder: {}", repo.display()))?;
    let home = home().ok_or("the home folder is not set")?;
    let adapters = rigswig_core::adapters::all();
    let ctx_for = |a: &dyn Adapter| {
        let env = a.env_vars().iter().filter_map(|k| std::env::var(k).ok().map(|v| ((*k).to_string(), v))).collect();
        Ctx { fs: &os_fs::OsFs, env, home: home.clone(), repo: repo.clone(), os: this_os() }
    };
    let a = match tool {
        Some(t) => {
            adapters.iter().find(|a| a.id() == t).ok_or(format!("no harness called {t} (see `rigswig tools`)"))?
        }
        None => adapters.iter().max_by_key(|a| a.detect(&ctx_for(a.as_ref())).score).expect("an adapter"),
    };
    Ok(a.resolve(&ctx_for(a.as_ref())))
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
        ([one], Some((x, y))) => (resolve(one, Some(x)), resolve(one, Some(y))),
        ([], Some((x, y))) => {
            let here = std::env::current_dir().unwrap_or_default();
            (resolve(&here, Some(x)), resolve(&here, Some(y)))
        }
        ([one, two], None) => (resolve(one, tool.as_deref()), resolve(two, tool.as_deref())),
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
        Some("compare") => compare(&args[1..]),
        Some(other) => fail(&format!("unknown or unimplemented command: {other}")),
    }
}
