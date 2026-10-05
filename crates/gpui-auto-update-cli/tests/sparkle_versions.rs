//! `gpui-auto-update sparkle versions`: the pinned Sparkle distributions.

use std::process::Command;

#[test]
fn lists_the_pinned_sparkle_releases_with_their_checksums() {
    let out = Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .args(["sparkle", "versions"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    let default = stdout
        .lines()
        .find(|l| l.contains("(default)"))
        .expect("one release is marked as the default");
    assert!(default.starts_with("2.10.0 "), "{default}");
    assert!(
        default.contains("c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c"),
        "{default}"
    );
    assert!(stdout.lines().any(|l| l.starts_with("2.9.6 ")), "{stdout}");
}
