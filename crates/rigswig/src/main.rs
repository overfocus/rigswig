//! rigswig: a harness-agnostic, read-only explorer for coding-agent setups (docs/ROADMAP.md).

mod os_fs;
mod render;

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

Planned:
  rigswig ui [--repo <path>]                the same as a local, read-only page
  rigswig compare <a> <b>                   two repos, or two harnesses on one repo

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

fn status(args: &[String], list_only: bool) -> ExitCode {
    let mut repo: Option<PathBuf> = None;
    let mut json = false;
    let mut tool: Option<String> = None;
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

    if list_only {
        for (a, d) in &scored {
            let ev = if d.evidence.is_empty() { "no sign of it here".into() } else { d.evidence.join(", ") };
            println!("{:<12} {:>3}  {}", a.id(), d.score, printable(&ev));
        }
        return ExitCode::SUCCESS;
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
        Some("status") => status(&args[1..], false),
        Some("tools") => status(&args[1..], true),
        Some(other) => fail(&format!("unknown or unimplemented command: {other}")),
    }
}
