//! Inno Setup handoff arguments: silent per-user switches, a configurable
//! switch set, and the running install directory pinned with `/DIR=`.

mod support;

use std::path::{Path, PathBuf};

use gpui_auto_update_core::ErrorKind;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_windows::{InnoSetup, InstallTarget, InstallerStrategy};
use support::PeImage;

fn target(dir: &str) -> InstallTarget {
    InstallTarget::new(PathBuf::from(dir), "demo.exe").unwrap()
}

fn args(inno: &InnoSetup, dir: &str) -> Vec<String> {
    inno.command(Path::new("setup.exe"), &target(dir))
        .unwrap()
        .args()
        .to_vec()
}

#[test]
fn default_handoff_is_very_silent_per_user_and_pins_the_install_dir() {
    let inno = InnoSetup::new();
    let command = inno
        .command(
            Path::new(r"C:\Users\Ann\AppData\Local\Temp\update-1\setup.exe"),
            &target(r"C:\Users\Ann\AppData\Local\Programs\Demo"),
        )
        .unwrap();
    assert_eq!(
        command.program(),
        Path::new(r"C:\Users\Ann\AppData\Local\Temp\update-1\setup.exe")
    );
    assert_eq!(
        command.args(),
        [
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/SP-",
            "/CURRENTUSER",
            "/CLOSEAPPLICATIONS",
            "/NORESTARTAPPLICATIONS",
            r#"/DIR="C:\Users\Ann\AppData\Local\Programs\Demo""#,
        ]
    );
    assert_eq!(
        command.command_line(),
        concat!(
            r#""C:\Users\Ann\AppData\Local\Temp\update-1\setup.exe" "#,
            "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /CURRENTUSER ",
            "/CLOSEAPPLICATIONS /NORESTARTAPPLICATIONS ",
            r#"/DIR="C:\Users\Ann\AppData\Local\Programs\Demo""#,
        )
    );
}

#[test]
fn passive_handoff_shows_progress_without_prompts() {
    assert_eq!(
        args(&InnoSetup::passive(), r"D:\Apps\Demo"),
        [
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/SP-",
            "/CURRENTUSER",
            "/CLOSEAPPLICATIONS",
            "/NORESTARTAPPLICATIONS",
            r#"/DIR="D:\Apps\Demo""#,
        ]
    );
}

#[test]
fn the_switch_set_is_configurable_but_dir_is_always_last() {
    let inno = InnoSetup::new()
        .with_switches(["/VERYSILENT", "/NORESTART", "/MERGETASKS=!desktopicon"])
        .unwrap();
    assert_eq!(
        args(&inno, r"D:\Apps\Demo"),
        [
            "/VERYSILENT",
            "/NORESTART",
            "/MERGETASKS=!desktopicon",
            r#"/DIR="D:\Apps\Demo""#,
        ]
    );
    let inno = InnoSetup::new().with_extra_switch("/NOCANCEL").unwrap();
    let args = args(&inno, r"D:\Apps\Demo");
    assert_eq!(args[args.len() - 2], "/NOCANCEL");
    assert_eq!(
        inno.switches().last().map(String::as_str),
        Some("/NOCANCEL")
    );
}

#[test]
fn switches_that_would_move_elevate_or_split_the_install_are_rejected() {
    for switch in [
        "/DIR=C:\\Other",
        "/dir=\"C:\\Other\"",
        "/ALLUSERS",
        "/allusers",
        "/LOG",
        "/LOG=C:\\x.log",
        "VERYSILENT",
        "",
        "/",
        "/A B",
        "/A\"B",
        "/A\tB",
        "/A\u{7}",
    ] {
        assert!(
            InnoSetup::new().with_extra_switch(switch).is_err(),
            "accepted {switch:?}"
        );
        assert!(
            InnoSetup::new().with_switches([switch]).is_err(),
            "accepted {switch:?} in a switch set"
        );
    }
}

#[test]
fn a_log_file_is_passed_quoted_before_the_install_dir() {
    let inno = InnoSetup::new().with_log_file(r"C:\Users\Ann\AppData\Local\Demo\update.log");
    let args = args(&inno, r"D:\Apps\Demo");
    assert_eq!(
        &args[args.len() - 2..],
        [
            r#"/LOG="C:\Users\Ann\AppData\Local\Demo\update.log""#,
            r#"/DIR="D:\Apps\Demo""#,
        ]
    );
}

#[test]
fn install_dir_paths_are_normalized_for_the_installer() {
    let cases = [
        (r"\\?\C:\Users\Ann\Demo", r#"/DIR="C:\Users\Ann\Demo""#),
        (
            r"\\?\UNC\server\share\Demo",
            r#"/DIR="\\server\share\Demo""#,
        ),
        (r"C:\Users\Ann\Demo\", r#"/DIR="C:\Users\Ann\Demo""#),
        (r"C:\", r#"/DIR="C:\""#),
        (r"\\?\C:\", r#"/DIR="C:\""#),
        (
            r"C:\Program Files (x86)\Démo app",
            r#"/DIR="C:\Program Files (x86)\Démo app""#,
        ),
    ];
    for (dir, expected) in cases {
        let args = args(&InnoSetup::new(), dir);
        assert_eq!(args.last().unwrap(), expected, "for {dir:?}");
    }
}

#[test]
fn install_dirs_that_cannot_be_quoted_are_rejected() {
    for dir in [r#"C:\Users\Ann\De"mo"#, "C:\\Users\\Ann\\De\nmo", ""] {
        let error = InnoSetup::new()
            .command(Path::new("setup.exe"), &target(dir))
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Configuration, "for {dir:?}");
    }
}

#[test]
fn installers_are_staged_with_an_exe_name_and_confirm_product_version() {
    let inno = InnoSetup::new();
    assert_eq!(inno.staged_file_name(), "setup.exe");
    assert_eq!(inno.name(), "Inno Setup");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("setup.exe");
    std::fs::write(&path, PeImage::installer("1.5.0").build()).unwrap();
    let v150 = ReleaseVersion::parse("1.5.0").unwrap();
    let v160 = ReleaseVersion::parse("1.6.0").unwrap();
    assert_eq!(inno.confirm_version(&path, &v150), Ok(()));
    assert_eq!(
        inno.confirm_version(&path, &v160).unwrap_err().kind(),
        ErrorKind::ArchiveValidation
    );

    // FileVersion is "1.5.0.0" in the fixture, so a different key changes
    // the outcome.
    let by_file_version = InnoSetup::new().with_version_key("FileVersion");
    assert_eq!(
        by_file_version
            .confirm_version(&path, &v150)
            .unwrap_err()
            .kind(),
        ErrorKind::ArchiveValidation
    );
}

#[test]
fn install_targets_come_from_the_running_executable_path() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("demo.exe");
    let target = InstallTarget::for_executable(&exe).unwrap();
    assert_eq!(target.install_dir(), dir.path());
    assert_eq!(target.executable(), exe);

    assert_eq!(
        InstallTarget::for_executable(Path::new("demo.exe"))
            .unwrap_err()
            .kind(),
        ErrorKind::UnsupportedInstallation
    );
    for name in ["", "..", "a/b.exe", r"a\b.exe"] {
        assert!(
            InstallTarget::new(dir.path().to_path_buf(), name).is_err(),
            "accepted executable name {name:?}"
        );
    }
}
