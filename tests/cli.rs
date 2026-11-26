//! Smoke tests against the real binary, with stdout piped (not a console).

use std::process::{Command, Output};

fn autotyper(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_autotyper"))
        .args(args)
        .env_remove("NO_COLOR")
        .output()
        .expect("run the autotyper binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("UTF-8 output")
}

#[test]
fn author_prints_the_plain_card_when_piped() {
    let out = autotyper(&["--author"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.is_ascii(), "{text:?}");
    assert!(!text.contains('\x1b'));
    assert!(text.contains("Viren Hirpara"));
}

#[test]
fn author_wins_over_other_arguments() {
    let out = autotyper(&["--clipboard", "--nonsense", "--author"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).contains("Viren Hirpara"));
}

#[test]
fn help_lists_the_options_and_hints_at_author() {
    let out = autotyper(&["--help"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stdout(&out);
    assert!(text.contains("--clipboard"));
    assert!(text.contains("--author") && text.contains("who built this"));
    assert!(!text.contains("Viren"));
}

#[test]
fn version_prints_the_crate_version() {
    let out = autotyper(&["--version"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        stdout(&out).trim(),
        format!("autotyper {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_arguments_fail_with_usage_hint() {
    let out = autotyper(&["--definitely-not-an-option"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--help"));
}
