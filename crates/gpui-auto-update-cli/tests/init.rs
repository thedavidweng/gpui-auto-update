//! `gpui-auto-update init`: inspects a consuming project and explains the
//! updater configuration it needs, without inventing any of it.

use std::fs;
use std::process::{Command, Output};

fn project(cargo_toml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), cargo_toml).unwrap();
    dir
}

const PLAIN: &str = "[package]\nname = \"example\"\nversion = \"1.2.3\"\nedition = \"2024\"\n";

fn run(command: &str, dir: &tempfile::TempDir, extra: &[&str]) -> Output {
    let manifest = dir.path().join("Cargo.toml");
    Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .args([command, "--manifest-path"])
        .arg(&manifest)
        .args(extra)
        .output()
        .expect("failed to run gpui-auto-update")
}

fn output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn init_explains_each_requirement_with_placeholders_only() {
    let dir = project(PLAIN);
    let out = run("init", &dir, &[]);
    assert!(out.status.success(), "{}", output(&out));
    let text = output(&out);
    for needle in [
        "example 1.2.3",
        "[package.metadata.gpui-auto-update]",
        "app-id",
        "public-key",
        "gpui-auto-update keys generate",
        "feed-url",
        "strategy",
        "app-name",
        "gpui-auto-update doctor",
        "does not depend on gpui-auto-update",
    ] {
        assert!(text.contains(needle), "expected {needle:?} in:\n{text}");
    }
    // Identity, key, installer, and host values are the developer's
    // decisions: every value in the snippet is a placeholder.
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(" = ") {
            let key = key.trim_start_matches('#').trim();
            if [
                "app-id",
                "public-key",
                "feed-url",
                "strategy",
                "app-name",
                "x86_64",
            ]
            .contains(&key)
            {
                assert!(value.starts_with("\"<"), "invented value in {line:?}");
            }
        }
    }
    // Nothing was written.
    assert_eq!(
        fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(),
        PLAIN
    );
}

#[test]
fn init_write_appends_a_skeleton_that_doctor_reports_as_unfinished() {
    let dir = project(PLAIN);
    let out = run("init", &dir, &["--write"]);
    assert!(out.status.success(), "{}", output(&out));
    let written = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(written.starts_with(PLAIN), "{written}");
    assert!(
        written.contains("[package.metadata.gpui-auto-update]"),
        "{written}"
    );

    let doctor = run("doctor", &dir, &["--offline"]);
    assert_eq!(doctor.status.code(), Some(1), "{}", output(&doctor));
    assert!(
        output(&doctor).contains("placeholder"),
        "{}",
        output(&doctor)
    );

    let again = run("init", &dir, &["--write"]);
    assert_eq!(again.status.code(), Some(1), "{}", output(&again));
    assert!(output(&again).contains("already"), "{}", output(&again));
    assert_eq!(
        fs::read_to_string(dir.path().join("Cargo.toml")).unwrap(),
        written
    );
}

#[test]
fn init_reports_existing_configuration_and_what_is_missing() {
    let dir = project(&format!(
        "{PLAIN}\n[dependencies]\ngpui-auto-update = \"0.1\"\n\n\
         [package.metadata.gpui-auto-update]\napp-id = \"dev.gpui-auto-update.example\"\n\n\
         [package.metadata.gpui-auto-update.linux]\napp-name = \"example\"\n"
    ));
    let out = run("init", &dir, &[]);
    assert!(out.status.success(), "{}", output(&out));
    let text = output(&out);
    assert!(text.contains("dev.gpui-auto-update.example"), "{text}");
    assert!(text.contains("public-key is not set"), "{text}");
    assert!(!text.contains("does not depend on"), "{text}");
}

#[test]
fn init_rejects_a_version_the_updater_cannot_order() {
    let dir = project("[package]\nname = \"example\"\nversion = \"1.2\"\n");
    let out = run("init", &dir, &[]);
    assert!(output(&out).contains("1.2"), "{}", output(&out));
    assert!(output(&out).contains("SemVer"), "{}", output(&out));
}
