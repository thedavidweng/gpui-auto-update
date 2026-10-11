//! `gpui-auto-update sparkle embed`: placing Sparkle.framework and its
//! license notice inside an application bundle, with the XPC services the
//! declared sandbox mode needs.
#![cfg(unix)]

mod support;

use std::fs;
use std::path::Path;

use support::*;

fn embed(app: &Path, dist: &Path, mode: &str) -> std::process::Output {
    cli(&[
        "sparkle",
        "embed",
        "--app",
        app.to_str().unwrap(),
        "--sparkle",
        dist.to_str().unwrap(),
        "--sandbox",
        mode,
    ])
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let app = fixture_app(tmp.path(), &dist, &AppSpec::default());
    (tmp, dist, app)
}

const FW: &str = "Contents/Frameworks/Sparkle.framework";

#[test]
fn non_sandboxed_apps_get_the_framework_without_xpc_services() {
    let (_tmp, dist, app) = setup();
    let out = embed(&app, &dist, "non-sandboxed");
    assert!(out.status.success(), "{}", stderr(&out));

    let fw = app.join(FW);
    assert_eq!(
        fs::read_link(fw.join("Versions/Current")).unwrap().to_str(),
        Some("B")
    );
    assert_eq!(
        fs::read_link(fw.join("Sparkle")).unwrap().to_str(),
        Some("Versions/Current/Sparkle")
    );
    assert!(fw.join("Versions/B/Autoupdate").is_file());
    assert!(
        fw.join("Versions/B/Updater.app/Contents/MacOS/Updater")
            .is_file()
    );
    assert!(!fw.join("Versions/B/XPCServices").exists());
    assert!(
        fs::symlink_metadata(fw.join("XPCServices")).is_err(),
        "no dangling XPCServices symlink may remain"
    );
    assert!(stdout(&out).contains("sparkle sign"), "{}", stdout(&out));
}

#[test]
fn the_sparkle_license_notice_is_preserved_in_the_bundle() {
    let (_tmp, dist, app) = setup();
    let out = embed(&app, &dist, "non-sandboxed");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        fs::read_to_string(app.join("Contents/Resources/ThirdPartyNotices/Sparkle/LICENSE"))
            .unwrap(),
        SPARKLE_LICENSE
    );
}

#[test]
fn sandboxed_apps_keep_the_installer_and_downloader_services() {
    let (_tmp, dist, app) = setup();
    let out = embed(&app, &dist, "sandboxed");
    assert!(out.status.success(), "{}", stderr(&out));
    let xpc = app.join(FW).join("Versions/B/XPCServices");
    assert!(xpc.join("Installer.xpc/Contents/Info.plist").is_file());
    assert!(xpc.join("Downloader.xpc/Contents/Info.plist").is_file());
    assert!(app.join(FW).join("XPCServices").is_dir());
    let stdout = stdout(&out);
    assert!(
        stdout.contains("SUEnableInstallerLauncherService"),
        "{stdout}"
    );
    assert!(stdout.contains("SUEnableDownloaderService"), "{stdout}");
}

#[test]
fn sandboxed_apps_with_network_access_drop_only_the_downloader() {
    let (_tmp, dist, app) = setup();
    let out = embed(&app, &dist, "sandboxed-network-client");
    assert!(out.status.success(), "{}", stderr(&out));
    let xpc = app.join(FW).join("Versions/B/XPCServices");
    assert!(xpc.join("Installer.xpc").is_dir());
    assert!(!xpc.join("Downloader.xpc").exists());
}

#[test]
fn the_sandbox_mode_must_be_stated_explicitly() {
    let (_tmp, dist, app) = setup();
    let out = cli(&[
        "sparkle",
        "embed",
        "--app",
        app.to_str().unwrap(),
        "--sparkle",
        dist.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--sandbox"), "{}", stderr(&out));
}

#[test]
fn embedding_again_replaces_the_previous_framework() {
    let (_tmp, dist, app) = setup();
    assert!(embed(&app, &dist, "sandboxed").status.success());
    let out = embed(&app, &dist, "non-sandboxed");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!app.join(FW).join("Versions/B/XPCServices").exists());
    let leftovers: Vec<_> = fs::read_dir(app.join("Contents/Frameworks"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["Sparkle.framework"]);
}

#[test]
fn a_distribution_without_its_license_is_refused() {
    let (_tmp, dist, app) = setup();
    fs::remove_file(dist.join("LICENSE")).unwrap();
    let out = embed(&app, &dist, "non-sandboxed");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("LICENSE"), "{}", stderr(&out));
    assert!(!app.join(FW).exists());
}

#[test]
fn the_target_must_be_an_app_bundle() {
    let (tmp, dist, _app) = setup();
    let out = embed(&tmp.path().join("Missing.app"), &dist, "non-sandboxed");
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("Info.plist"), "{}", stderr(&out));
}
