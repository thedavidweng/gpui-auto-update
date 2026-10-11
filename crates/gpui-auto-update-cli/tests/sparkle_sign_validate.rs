//! `gpui-auto-update sparkle sign` and `sparkle validate` on fixture app
//! bundles, signed ad hoc with the system `codesign`.
#![cfg(target_os = "macos")]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::*;

struct Fixture {
    tmp: tempfile::TempDir,
    dist: PathBuf,
    app: PathBuf,
}

fn fixture(spec: &AppSpec<'_>, mode: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let app = fixture_app(tmp.path(), &dist, spec);
    let out = cli(&[
        "sparkle",
        "embed",
        "--app",
        app.to_str().unwrap(),
        "--sparkle",
        dist.to_str().unwrap(),
        "--sandbox",
        mode,
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    Fixture { tmp, dist, app }
}

fn sign(app: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "sparkle",
        "sign",
        "--app",
        app.to_str().unwrap(),
        "--identity",
        "-",
    ];
    args.extend_from_slice(extra);
    cli(&args)
}

fn validate(app: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["sparkle", "validate", "--app", app.to_str().unwrap()];
    args.extend_from_slice(extra);
    cli(&args)
}

fn codesign_details(path: &Path) -> String {
    let out = Command::new("codesign")
        .args(["-dv", "--verbose=4"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn sip_enabled() -> bool {
    let out = Command::new("csrutil").arg("status").output().unwrap();
    !String::from_utf8_lossy(&out.stdout).contains("disabled")
}

fn entitlements_file(dir: &Path, entries: &[(&str, String)]) -> PathBuf {
    let path = dir.join(format!("ent-{}.plist", entries.len()));
    write(&path, &plist_xml(entries));
    path
}

fn sandbox_entitlements(dir: &Path, network_client: bool) -> PathBuf {
    let mut entries = vec![
        ("com.apple.security.app-sandbox", "<true/>".to_owned()),
        (
            "com.apple.security.temporary-exception.mach-lookup.global-name",
            "<array><string>com.example.Fixture-spks</string><string>com.example.Fixture-spki</string></array>"
                .to_owned(),
        ),
    ];
    if network_client {
        entries.push(("com.apple.security.network.client", "<true/>".to_owned()));
    }
    entitlements_file(dir, &entries)
}

fn sandboxed_info() -> Vec<(&'static str, String)> {
    let mut info = valid_info();
    info.push(("SUEnableInstallerLauncherService", "<true/>".to_owned()));
    info.push(("SUEnableDownloaderService", "<true/>".to_owned()));
    info
}

#[test]
fn signs_nested_sparkle_code_inside_out_with_hardened_runtime() {
    let f = fixture(&AppSpec::default(), "sandboxed");
    let out = sign(&f.app, &[]);
    assert!(out.status.success(), "{}", stderr(&out));

    let order: Vec<String> = stdout(&out)
        .lines()
        .filter_map(|l| l.strip_prefix("signed "))
        .map(|p| {
            Path::new(p)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        order,
        [
            "Installer.xpc",
            "Downloader.xpc",
            "Autoupdate",
            "Updater.app",
            "Sparkle.framework",
            "Fixture.app"
        ]
    );

    let fw = f.app.join("Contents/Frameworks/Sparkle.framework");
    for path in [
        fw.join("Versions/B/XPCServices/Installer.xpc"),
        fw.join("Versions/B/XPCServices/Downloader.xpc"),
        fw.join("Versions/B/Autoupdate"),
        fw.join("Versions/B/Updater.app"),
        fw.clone(),
    ] {
        let details = codesign_details(&path);
        assert!(details.contains("runtime"), "{}: {details}", path.display());
    }
    // Library validation would refuse the ad-hoc framework if the ad-hoc app
    // itself ran with the hardened runtime.
    let app_details = codesign_details(&f.app);
    assert!(!app_details.contains("runtime"), "{app_details}");
    let verify = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&f.app)
        .output()
        .unwrap();
    assert!(verify.status.success(), "{verify:?}");
}

#[test]
fn the_signed_app_loads_the_embedded_framework() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    assert!(sign(&f.app, &[]).status.success());
    let status = Command::new(f.app.join("Contents/MacOS/Fixture"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn an_ad_hoc_app_under_library_validation_is_reported_as_unlaunchable() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    assert!(sign(&f.app, &[]).status.success());
    let resign = Command::new("codesign")
        .args(["--force", "--sign", "-", "--options", "runtime"])
        .arg(&f.app)
        .output()
        .unwrap();
    assert!(resign.status.success(), "{resign:?}");
    let launched = Command::new(f.app.join("Contents/MacOS/Fixture"))
        .output()
        .unwrap();
    // Hosts with System Integrity Protection disabled (such as GitHub's macOS
    // runners) do not enforce library validation, so dyld loads the framework
    // there; the validator must still flag the bundle.
    if launched.status.success() {
        assert!(
            !sip_enabled(),
            "dyld should refuse the framework: {launched:?}"
        );
    }

    let out = validate(&f.app, &["--sandbox", "non-sandboxed"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("library validation"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn the_downloader_service_keeps_its_entitlements() {
    let f = fixture(&AppSpec::default(), "sandboxed");
    let downloader = f
        .app
        .join("Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices/Downloader.xpc");
    let ent = entitlements_file(
        f.tmp.path(),
        &[("com.apple.security.network.client", "<true/>".to_owned())],
    );
    let pre = Command::new("codesign")
        .args(["--force", "--sign", "-", "--entitlements"])
        .arg(&ent)
        .arg(&downloader)
        .output()
        .unwrap();
    assert!(pre.status.success(), "{pre:?}");

    assert!(sign(&f.app, &[]).status.success());

    let out = Command::new("codesign")
        .args(["-d", "--entitlements", "-", "--xml"])
        .arg(&downloader)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("com.apple.security.network.client"),
        "{out:?}"
    );
}

#[test]
fn the_identity_can_come_from_the_environment() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    let out = Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .args(["sparkle", "sign", "--app", f.app.to_str().unwrap()])
        .env("GPUI_AUTO_UPDATE_SIGNING_IDENTITY", "-")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn signing_without_an_identity_is_a_usage_error() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    let out = Command::new(env!("CARGO_BIN_EXE_gpui-auto-update"))
        .args(["sparkle", "sign", "--app", f.app.to_str().unwrap()])
        .env_remove("GPUI_AUTO_UPDATE_SIGNING_IDENTITY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--identity"), "{}", stderr(&out));
}

#[test]
fn a_correctly_packaged_ad_hoc_app_validates_with_a_distribution_warning() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    assert!(sign(&f.app, &[]).status.success());
    let out = validate(&f.app, &["--sandbox", "non-sandboxed"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("0 errors"), "{text}");
    assert!(text.contains("ad hoc"), "{text}");
}

#[test]
fn developer_id_can_be_required_for_release_builds() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    assert!(sign(&f.app, &[]).status.success());
    let out = validate(
        &f.app,
        &["--sandbox", "non-sandboxed", "--require-developer-id"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("Developer ID"), "{}", stdout(&out));
}

#[test]
fn unsigned_bundles_fail_signature_validation() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    let out = validate(&f.app, &["--sandbox", "non-sandboxed"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("not signed"), "{}", stdout(&out));
}

#[test]
fn signature_checks_can_be_skipped_before_signing() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    let out = validate(
        &f.app,
        &["--sandbox", "non-sandboxed", "--no-signature-checks"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
}

#[test]
fn code_without_hardened_runtime_is_rejected() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    assert!(sign(&f.app, &[]).status.success());
    let autoupdate = f
        .app
        .join("Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate");
    let resign = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(&autoupdate)
        .output()
        .unwrap();
    assert!(resign.status.success());
    let out = validate(&f.app, &["--sandbox", "non-sandboxed"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("Autoupdate") && text.contains("hardened runtime"),
        "{text}"
    );
}

#[test]
fn a_missing_rpath_to_the_frameworks_directory_is_reported() {
    let spec = AppSpec {
        rpath: None,
        ..AppSpec::default()
    };
    let f = fixture(&spec, "non-sandboxed");
    let out = validate(
        &f.app,
        &["--sandbox", "non-sandboxed", "--no-signature-checks"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("@executable_path/../Frameworks"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn an_rpath_that_finds_sparkle_outside_the_bundle_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let outside = dist.to_str().unwrap().to_owned();
    let rpath = format!("{outside},-rpath,@executable_path/../Frameworks");
    let spec = AppSpec {
        rpath: Some(&rpath),
        ..AppSpec::default()
    };
    let app = fixture_app(tmp.path(), &dist, &spec);
    assert!(
        cli(&[
            "sparkle",
            "embed",
            "--app",
            app.to_str().unwrap(),
            "--sparkle",
            &outside,
            "--sandbox",
            "non-sandboxed",
        ])
        .status
        .success()
    );
    let out = validate(
        &app,
        &["--sandbox", "non-sandboxed", "--no-signature-checks"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("outside the app bundle"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn apps_that_do_not_link_sparkle_get_a_warning() {
    let spec = AppSpec {
        link_sparkle: false,
        ..AppSpec::default()
    };
    let f = fixture(&spec, "non-sandboxed");
    let out = validate(
        &f.app,
        &["--sandbox", "non-sandboxed", "--no-signature-checks"],
    );
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).contains("does not link"), "{}", stdout(&out));
}

#[test]
fn a_missing_framework_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    let app = fixture_app(tmp.path(), &dist, &AppSpec::default());
    let out = validate(
        &app,
        &["--sandbox", "non-sandboxed", "--no-signature-checks"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("Contents/Frameworks/Sparkle.framework"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_missing_license_notice_is_reported() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    fs::remove_file(
        f.app
            .join("Contents/Resources/ThirdPartyNotices/Sparkle/LICENSE"),
    )
    .unwrap();
    let out = validate(
        &f.app,
        &["--sandbox", "non-sandboxed", "--no-signature-checks"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("license"), "{}", stdout(&out));
    let _ = &f.dist;
}

#[test]
fn info_plist_metadata_is_validated() {
    // (key, replacement value or None to remove it, expected message part)
    let cases: &[(&str, Option<&str>, &str)] = &[
        ("CFBundleIdentifier", None, "CFBundleIdentifier"),
        (
            "CFBundleIdentifier",
            Some("<string>no dots</string>"),
            "reverse-DNS",
        ),
        (
            "CFBundleShortVersionString",
            None,
            "CFBundleShortVersionString",
        ),
        (
            "CFBundleShortVersionString",
            Some("<string>1.2-beta</string>"),
            "CFBundleShortVersionString",
        ),
        ("CFBundleVersion", None, "CFBundleVersion"),
        (
            "CFBundleVersion",
            Some("<string>42a</string>"),
            "CFBundleVersion",
        ),
        ("SUFeedURL", None, "SUFeedURL"),
        (
            "SUFeedURL",
            Some("<string>http://updates.example.com/appcast.xml</string>"),
            "https",
        ),
        ("SUPublicEDKey", None, "SUPublicEDKey"),
        (
            "SUPublicEDKey",
            Some("<string>dG9vIHNob3J0</string>"),
            "32 bytes",
        ),
        (
            "SUPublicEDKey",
            Some("<string>not base64!</string>"),
            "base64",
        ),
        ("LSMinimumSystemVersion", None, "LSMinimumSystemVersion"),
        (
            "LSMinimumSystemVersion",
            Some("<string>11.0</string>"),
            "12.0",
        ),
        (
            "SUEnableAutomaticChecks",
            Some("<integer>1</integer>"),
            "boolean",
        ),
        (
            "SUScheduledCheckInterval",
            Some("<string>daily</string>"),
            "SUScheduledCheckInterval",
        ),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let dist = fixture_distribution(tmp.path(), "2.10.0");
    for (i, &(key, value, expected)) in cases.iter().enumerate() {
        let mut info: Vec<(&str, String)> = valid_info()
            .into_iter()
            .filter(|(k, _)| *k != key)
            .collect();
        if let Some(v) = value {
            info.push((key, v.to_owned()));
        }
        let root = tmp.path().join(i.to_string());
        let app = fixture_app(
            &root,
            &dist,
            &AppSpec {
                info,
                ..AppSpec::default()
            },
        );
        assert!(
            cli(&[
                "sparkle",
                "embed",
                "--app",
                app.to_str().unwrap(),
                "--sparkle",
                dist.to_str().unwrap(),
                "--sandbox",
                "non-sandboxed",
            ])
            .status
            .success()
        );
        let out = validate(
            &app,
            &["--sandbox", "non-sandboxed", "--no-signature-checks"],
        );
        let text = stdout(&out);
        assert_eq!(out.status.code(), Some(1), "{key}={value:?}: {text}");
        let line = text
            .lines()
            .find(|l| l.starts_with("error") && l.contains(expected))
            .unwrap_or_else(|| {
                panic!("{key}={value:?}: no error mentioning {expected:?} in\n{text}")
            });
        assert!(line.contains(key), "{line}");
    }
}

#[test]
fn the_build_version_must_increase_over_the_previous_release() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    let args = ["--sandbox", "non-sandboxed", "--no-signature-checks"];
    let same = validate(
        &f.app,
        &[&args[..], &["--previous-build-version", "42"]].concat(),
    );
    assert_eq!(same.status.code(), Some(1));
    assert!(stdout(&same).contains("greater than"), "{}", stdout(&same));
    let older = validate(
        &f.app,
        &[&args[..], &["--previous-build-version", "41.9"]].concat(),
    );
    assert!(older.status.success(), "{}", stdout(&older));
}

#[test]
fn sandboxed_apps_need_the_installer_launcher_keys() {
    let f = fixture(&AppSpec::default(), "sandboxed");
    let out = validate(&f.app, &["--sandbox", "sandboxed", "--no-signature-checks"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("SUEnableInstallerLauncherService"), "{text}");
    assert!(text.contains("SUEnableDownloaderService"), "{text}");
}

#[test]
fn sandboxed_apps_need_the_xpc_services() {
    let spec = AppSpec {
        info: sandboxed_info(),
        ..AppSpec::default()
    };
    let f = fixture(&spec, "non-sandboxed");
    let out = validate(&f.app, &["--sandbox", "sandboxed", "--no-signature-checks"]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("Installer.xpc"), "{text}");
    assert!(text.contains("Downloader.xpc"), "{text}");
}

#[test]
fn a_fully_configured_sandboxed_app_validates() {
    let spec = AppSpec {
        info: sandboxed_info(),
        ..AppSpec::default()
    };
    let f = fixture(&spec, "sandboxed");
    let ent = sandbox_entitlements(f.tmp.path(), false);
    let signed = sign(&f.app, &["--entitlements", ent.to_str().unwrap()]);
    assert!(signed.status.success(), "{}", stderr(&signed));
    // The sandbox mode is read from the signed entitlements.
    let out = validate(&f.app, &[]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).contains("sandboxed"), "{}", stdout(&out));
}

#[test]
fn sandboxed_apps_need_the_mach_lookup_exceptions() {
    let spec = AppSpec {
        info: sandboxed_info(),
        ..AppSpec::default()
    };
    let f = fixture(&spec, "sandboxed");
    let ent = entitlements_file(
        f.tmp.path(),
        &[("com.apple.security.app-sandbox", "<true/>".to_owned())],
    );
    assert!(
        sign(&f.app, &["--entitlements", ent.to_str().unwrap()])
            .status
            .success()
    );
    let out = validate(&f.app, &[]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("com.example.Fixture-spks"), "{text}");
    assert!(text.contains("com.example.Fixture-spki"), "{text}");
}

#[test]
fn a_declared_sandbox_mode_must_match_the_entitlements() {
    let spec = AppSpec {
        info: sandboxed_info(),
        ..AppSpec::default()
    };
    let f = fixture(&spec, "sandboxed");
    let ent = sandbox_entitlements(f.tmp.path(), false);
    assert!(
        sign(&f.app, &["--entitlements", ent.to_str().unwrap()])
            .status
            .success()
    );
    let out = validate(&f.app, &["--sandbox", "non-sandboxed"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("app-sandbox"), "{}", stdout(&out));
}

#[test]
fn an_undeclared_sandbox_mode_of_an_unsigned_app_is_an_error() {
    let f = fixture(&AppSpec::default(), "non-sandboxed");
    let out = validate(&f.app, &["--no-signature-checks"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("--sandbox"), "{}", stdout(&out));
}

/// Downloads the official pinned archive, then embeds, signs, validates, and
/// launches an app that links it. Run with `--ignored`.
#[test]
#[ignore = "downloads the official Sparkle archive"]
fn the_pinned_official_sparkle_packages_signs_validates_and_loads() {
    let tmp = tempfile::tempdir().unwrap();
    let dist = tmp.path().join("dist");
    let fetched = cli(&["sparkle", "fetch", "--out", dist.to_str().unwrap()]);
    assert!(fetched.status.success(), "{}", stderr(&fetched));
    let app = app_linking_real_sparkle(tmp.path(), &dist);
    for mode in ["non-sandboxed", "sandboxed"] {
        let mut info = valid_info();
        if mode == "sandboxed" {
            info.push(("SUEnableInstallerLauncherService", "<true/>".to_owned()));
            info.push(("SUEnableDownloaderService", "<true/>".to_owned()));
        }
        write(&app.join("Contents/Info.plist"), &plist_xml(&info));
        let embedded = cli(&[
            "sparkle",
            "embed",
            "--app",
            app.to_str().unwrap(),
            "--sparkle",
            dist.to_str().unwrap(),
            "--sandbox",
            mode,
        ]);
        assert!(embedded.status.success(), "{}", stderr(&embedded));
        let entitlements = sandbox_entitlements(tmp.path(), false);
        let sign_args: &[&str] = if mode == "sandboxed" {
            &["--entitlements", entitlements.to_str().unwrap()]
        } else {
            &[]
        };
        let signed = sign(&app, sign_args);
        assert!(signed.status.success(), "{}", stderr(&signed));
        let validated = validate(&app, &["--sandbox", mode]);
        assert!(validated.status.success(), "{mode}: {}", stdout(&validated));
        let launched = Command::new(app.join("Contents/MacOS/Fixture"))
            .status()
            .unwrap();
        assert!(launched.success(), "{mode}");
    }
}
