//! Real Windows process creation and file locking, run only on Windows CI.
//!
//! - An artifact Windows cannot start surfaces as a structured error while
//!   the application is still running.
//! - Post-exit ordering: a child "application" stages and hands off to an
//!   installer stand-in, then exits; the installer does its work only after
//!   the application has gone.
//! - A portable executable is replaced while a process runs from it.
#![cfg(windows)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use gpui_auto_update_core::feed::Arch;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{Capability, CheckKind, ErrorKind, UpdateCoordinator, UpdateError};
use gpui_auto_update_windows::{
    InnoSetup, InstallTarget, InstallerCommand, InstallerStrategy, PortableExecutable,
    UpdateStrategy, WindowsBackend, WindowsHandoff, WindowsUpdateConfig, confirm_embedded_version,
};
use support::{
    MACHINE_AMD64, MACHINE_ARM64, PeImage, feed, item, loopback_policy, serve, trusted_key,
};
use url::Url;

const CHILD_ENV: &str = "GPUI_AUTO_UPDATE_WINDOWS_HANDOFF_CHILD";

fn arch() -> Arch {
    Arch::current().expect("CI runs on x86_64 or aarch64")
}

fn native_machine() -> u16 {
    match arch() {
        Arch::X86_64 => MACHINE_AMD64,
        _ => MACHINE_ARM64,
    }
}

fn system32(program: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&root).join("System32").join(program)
}

/// A backend for an install in `install_dir` whose feed offers `artifact`
/// as release 1.5.0.
fn backend(
    install_dir: &Path,
    staging: Option<&Path>,
    strategy: UpdateStrategy,
    artifact: Vec<u8>,
) -> WindowsBackend {
    let base = serve_feed(artifact);
    let mut config = WindowsUpdateConfig::new(
        ReleaseVersion::parse("1.0.0").unwrap(),
        trusted_key(),
        strategy,
    )
    .with_arch(arch())
    .with_feed(arch(), Url::parse(&base).unwrap())
    .with_fetch_policy(loopback_policy())
    .with_install_target(InstallTarget::new(install_dir.to_path_buf(), "demo.exe").unwrap())
    .with_launch_grace(Duration::from_millis(200));
    if let Some(staging) = staging {
        config = config.with_staging_root(staging.to_path_buf());
    }
    WindowsBackend::new(config).unwrap()
}

fn serve_feed(artifact: Vec<u8>) -> String {
    // The artifact URL must be known before the feed is built, so serve the
    // artifact first and the feed from a second server.
    let artifact_base = serve(vec![("/artifact", artifact.clone())]);
    let xml = feed(&[item(
        "1.5.0",
        arch().as_str(),
        &format!("{artifact_base}/artifact"),
        &artifact,
        &artifact,
    )]);
    format!("{}/feed.xml", serve(vec![("/feed.xml", xml)]))
}

fn check_and_stage(backend: &WindowsBackend) {
    let coordinator = UpdateCoordinator::new(backend.check_source(), Capability::SelfManaged);
    coordinator.check(CheckKind::Manual).unwrap();
    backend.stage(&coordinator).unwrap();
}

#[test]
fn per_user_installs_are_self_managed() {
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(
        dir.path(),
        Some(&dir.path().join("staging")),
        UpdateStrategy::inno_setup(InnoSetup::new()),
        PeImage::installer("1.5.0").build(),
    );
    assert_eq!(backend.capability(), Capability::SelfManaged);
}

#[test]
fn an_installer_windows_cannot_start_is_a_structured_error() {
    // The fixture is a well-formed PE header with no code, which
    // CreateProcess rejects as an invalid image.
    let dir = tempfile::tempdir().unwrap();
    let backend = backend(
        dir.path(),
        Some(&dir.path().join("staging")),
        UpdateStrategy::inno_setup(InnoSetup::new()),
        PeImage::installer("1.5.0").build(),
    );
    check_and_stage(&backend);
    let error = backend.install().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::HelperLaunch, "{error:?}");
    assert!(error.diagnostic().unwrap().contains("/DIR="));
    assert!(backend.staged_path().is_some());
}

/// Stands in for an installer: `cmd.exe` waits a moment, then writes
/// `marker`, the way a real installer replaces files and relaunches once
/// the application has exited.
#[derive(Debug)]
struct DelayedMarker {
    marker: PathBuf,
}

impl InstallerStrategy for DelayedMarker {
    fn name(&self) -> &str {
        "delayed marker"
    }
    fn staged_file_name(&self) -> &str {
        "setup.exe"
    }
    fn confirm_version(
        &self,
        artifact: &Path,
        expected: &ReleaseVersion,
    ) -> Result<(), UpdateError> {
        confirm_embedded_version(artifact, "ProductVersion", expected)
    }
    fn command(
        &self,
        _artifact: &Path,
        _target: &InstallTarget,
    ) -> Result<InstallerCommand, UpdateError> {
        Ok(InstallerCommand::new(system32("cmd.exe"))
            .raw_arg("/d")
            .raw_arg("/s")
            .raw_arg("/c")
            .raw_arg(format!(
                "\"ping -n 4 127.0.0.1 >nul & type nul > \"{}\"\"",
                self.marker.display()
            )))
    }
}

/// The "application" half of the ordering fixture; does nothing unless the
/// parent test started this process for it.
#[test]
fn handoff_child() {
    let Some(dir) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let dir = PathBuf::from(dir);
    let backend = backend(
        &dir,
        Some(&dir.join("staging")),
        UpdateStrategy::installer(DelayedMarker {
            marker: dir.join("installed"),
        }),
        PeImage::installer("1.5.0").build(),
    );
    check_and_stage(&backend);
    assert_eq!(backend.install().unwrap(), WindowsHandoff::QuitForInstaller);
    // The application now quits; the installer must not have finished.
    assert!(!dir.join("installed").exists());
    fs::write(dir.join("handed-off"), b"").unwrap();
}

#[test]
fn the_installer_finishes_after_the_application_exits() {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "handoff_child", "--test-threads=1"])
        .env(CHILD_ENV, dir.path())
        .status()
        .unwrap();
    assert!(status.success(), "the application half failed: {status}");
    assert!(dir.path().join("handed-off").exists());
    let exited = Instant::now();
    assert!(
        !dir.path().join("installed").exists(),
        "the installer finished before the application exited"
    );

    let deadline = exited + Duration::from_secs(30);
    while !dir.path().join("installed").exists() {
        assert!(
            Instant::now() < deadline,
            "the installer did not outlive the application"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_running_portable_executable_is_replaced_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("demo.exe");
    fs::copy(system32("PING.EXE"), &exe).unwrap();
    let mut running = Command::new(&exe)
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    // A running image cannot be overwritten, which is why the swap renames.
    assert!(fs::write(&exe, b"overwrite").is_err());

    let new_exe = PeImage::executable(native_machine(), "1.5.0").build();
    let backend = backend(
        dir.path(),
        None,
        UpdateStrategy::portable(PortableExecutable::new()),
        new_exe.clone(),
    );
    assert_eq!(backend.capability(), Capability::SelfManaged);
    check_and_stage(&backend);
    let handoff = backend.install();
    let previous_still_running = running.try_wait().unwrap().is_none();
    let _ = running.kill();
    let _ = running.wait();

    assert_eq!(
        handoff.unwrap(),
        WindowsHandoff::Restart {
            executable: exe.clone()
        }
    );
    assert!(previous_still_running);
    assert_eq!(fs::read(&exe).unwrap(), new_exe);
    assert!(dir.path().join("demo.exe.previous").exists());

    // Once the old process has exited, the next launch cleans up.
    assert_eq!(backend.capability(), Capability::SelfManaged);
    assert!(!dir.path().join("demo.exe.previous").exists());
}
