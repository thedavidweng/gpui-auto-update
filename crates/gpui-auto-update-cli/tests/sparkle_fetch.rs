//! `gpui-auto-update sparkle fetch`: acquiring a declared Sparkle
//! distribution and verifying its checksum before anything is extracted.
#![cfg(unix)]

mod support;

use std::fs;

use support::*;

#[test]
fn fetches_a_declared_distribution_and_extracts_it_with_symlinks_intact() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let archive = tar_xz(&dist);
    let url = serve(archive.clone(), "Sparkle-2.10.0.tar.xz");
    let out_dir = tmp.path().join("out");

    let out = cli(&[
        "sparkle",
        "fetch",
        "--url",
        &url,
        "--sha256",
        &sha256_hex(&archive),
        "--out",
        out_dir.to_str().unwrap(),
    ]);

    assert!(out.status.success(), "{}", stderr(&out));
    let current = out_dir.join("Sparkle.framework/Versions/Current");
    assert_eq!(fs::read_link(&current).unwrap().to_str(), Some("B"));
    assert!(out_dir.join("Sparkle.framework/Sparkle").exists());
    assert_eq!(
        fs::read_to_string(out_dir.join("LICENSE")).unwrap(),
        SPARKLE_LICENSE
    );
    assert!(stdout(&out).contains("2.10.0"), "{}", stdout(&out));
}

#[test]
fn a_mirror_of_a_pinned_version_must_match_the_pinned_checksum() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let url = serve(tar_xz(&dist), "Sparkle-2.10.0.tar.xz");
    let out_dir = tmp.path().join("out");

    let out = cli(&[
        "sparkle",
        "fetch",
        "--version",
        "2.10.0",
        "--url",
        &url,
        "--out",
        out_dir.to_str().unwrap(),
    ]);

    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("checksum mismatch"), "{err}");
    assert!(
        err.contains("c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c"),
        "{err}"
    );
    assert!(!out_dir.exists(), "nothing may be written on mismatch");
}

#[test]
fn a_declared_checksum_that_does_not_match_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let file = tmp.path().join("Sparkle.tar.xz");
    fs::write(&file, tar_xz(&dist)).unwrap();
    let out_dir = tmp.path().join("out");

    let out = cli(&[
        "sparkle",
        "fetch",
        "--archive",
        file.to_str().unwrap(),
        "--sha256",
        &"0".repeat(64),
        "--out",
        out_dir.to_str().unwrap(),
    ]);

    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("checksum mismatch"),
        "{}",
        stderr(&out)
    );
    assert!(!out_dir.exists());
}

#[test]
fn a_local_archive_can_be_used_offline() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let archive = tar_xz(&dist);
    let file = tmp.path().join("Sparkle.tar.xz");
    fs::write(&file, &archive).unwrap();
    let out_dir = tmp.path().join("out");

    let out = cli(&[
        "sparkle",
        "fetch",
        "--archive",
        file.to_str().unwrap(),
        "--sha256",
        &sha256_hex(&archive).to_uppercase(),
        "--out",
        out_dir.to_str().unwrap(),
    ]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        out_dir
            .join("Sparkle.framework/Versions/B/Sparkle")
            .exists()
    );
}

#[test]
fn plain_http_is_refused_for_remote_hosts() {
    let tmp = tempfile::tempdir().unwrap();
    let out = cli(&[
        "sparkle",
        "fetch",
        "--url",
        "http://downloads.example.com/Sparkle-2.10.0.tar.xz",
        "--sha256",
        &"0".repeat(64),
        "--out",
        tmp.path().join("out").to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("https"), "{}", stderr(&out));
}

#[test]
fn an_unpinned_version_needs_an_explicit_checksum() {
    let tmp = tempfile::tempdir().unwrap();
    let out = cli(&[
        "sparkle",
        "fetch",
        "--version",
        "1.0.0",
        "--out",
        tmp.path().join("out").to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not pinned"), "{}", stderr(&out));
}

#[test]
fn the_output_directory_must_be_new_or_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let out_dir = tmp.path().join("out");
    write(&out_dir.join("keep.txt"), "user data");
    let out = cli(&["sparkle", "fetch", "--out", out_dir.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not empty"), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(out_dir.join("keep.txt")).unwrap(),
        "user data"
    );
}

#[test]
fn archives_without_a_framework_and_license_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    fs::remove_file(dist.join("LICENSE")).unwrap();
    let archive = tar_xz(&dist);
    let file = tmp.path().join("Sparkle.tar.xz");
    fs::write(&file, &archive).unwrap();
    let out_dir = tmp.path().join("out");

    let out = cli(&[
        "sparkle",
        "fetch",
        "--archive",
        file.to_str().unwrap(),
        "--sha256",
        &sha256_hex(&archive),
        "--out",
        out_dir.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("LICENSE"), "{}", stderr(&out));
    assert!(!out_dir.exists());
}

fn fetch_hostile(archive: Vec<u8>) -> (std::process::Output, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("hostile.tar.xz");
    fs::write(&file, &archive).unwrap();
    let out = cli(&[
        "sparkle",
        "fetch",
        "--archive",
        file.to_str().unwrap(),
        "--sha256",
        &sha256_hex(&archive),
        "--out",
        tmp.path().join("nested/out").to_str().unwrap(),
    ]);
    (out, tmp)
}

#[test]
fn entries_that_escape_the_output_directory_are_rejected() {
    let (out, tmp) = fetch_hostile(tar_xz_with_raw_entry(
        b"../escaped.txt",
        tar::EntryType::Regular,
        None,
    ));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("unsafe path"), "{}", stderr(&out));
    assert!(!tmp.path().join("nested/escaped.txt").exists());
}

#[test]
fn symlinks_that_point_outside_the_archive_are_rejected() {
    let (out, _tmp) = fetch_hostile(tar_xz_with_raw_entry(
        b"Sparkle.framework/evil",
        tar::EntryType::Symlink,
        Some("../../../etc"),
    ));
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("symlink"), "{}", stderr(&out));
}

/// Downloads the real pinned archive from GitHub. Run with `--ignored`.
#[test]
#[ignore = "downloads the official Sparkle archive"]
fn the_default_pin_matches_the_official_release() {
    let tmp = tempfile::tempdir().unwrap();
    let out_dir = tmp.path().join("out");
    let out = cli(&["sparkle", "fetch", "--out", out_dir.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        out_dir
            .join("Sparkle.framework/Versions/B/Sparkle")
            .exists()
    );
}
