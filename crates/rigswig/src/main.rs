//! rigswig: a harness-agnostic, read-only explorer for coding-agent setups (docs/ROADMAP.md).

use std::process::ExitCode;

// Used by the adapters once they land (roadmap step 3).
#[allow(dead_code)]
mod os_fs;

fn usage() -> String {
    format!(
        "rigswig {}

Usage:
  rigswig schema                            print the JSON Schema of rigswig's output

Planned:
  rigswig status [--repo <path>] [--json]   what a session in a repo loads, and where each piece came from
  rigswig ui [--repo <path>]                the same as a local, read-only page
  rigswig compare <a> <b>                   two repos, or two harnesses on one repo

rigswig never makes a network request, calls a model, or writes a file.
",
        rigswig_core::VERSION
    )
}

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
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
        Some(other) => {
            // The argument is the user's own, but strip control characters before echoing anything to a terminal.
            let shown: String = other.chars().filter(|c| !c.is_control()).collect();
            eprint!("rigswig: unknown or unimplemented command: {shown}\n\n{}", usage());
            ExitCode::from(2)
        }
    }
}
