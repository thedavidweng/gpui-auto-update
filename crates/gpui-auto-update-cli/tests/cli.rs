//! Behavior of the `gpui-auto-update` binary as seen from a shell.

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .args(args)
        .output()
        .expect("failed to run gpui-auto-update")
}

#[test]
fn version_flag_prints_the_package_version() {
    let out = cli(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "gpui-auto-update 0.0.0"
    );
}

#[test]
fn help_is_shown_when_no_arguments_are_given() {
    let out = cli(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: gpui-auto-update"));
}

#[test]
fn unknown_arguments_fail_with_usage_on_stderr() {
    let out = cli(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unrecognized subcommand 'frobnicate'"),
        "{stderr}"
    );
    assert!(stderr.contains("Usage: gpui-auto-update"));
    assert!(out.stdout.is_empty());
}
