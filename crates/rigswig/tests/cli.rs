use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rigswig")).args(args).output().expect("the binary runs")
}

#[test]
fn version_prints_the_crate_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{}\n", rigswig_core::VERSION));
}

#[test]
fn no_command_prints_usage() {
    let out = run(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("rigswig status"));
}

#[test]
fn unknown_command_exits_2_without_control_characters() {
    let out = run(&["no\u{1b}]0;pwned\u{7}pe"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unknown or unimplemented command: no]0;pwnedpe"));
    assert!(!err.contains('\u{1b}'));
}
