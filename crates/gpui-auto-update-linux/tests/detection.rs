//! Managed-install detection, exercised against isolated temporary roots.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use gpui_auto_update_core::Capability;
use gpui_auto_update_linux::{DetectionInputs, DetectionReason, detect};

const APP: &str = "demo";
const MARKER: &str = "gpui-auto-update managed-install 1\napp=demo\n";

/// A fake filesystem root with a user home inside it.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let home = root.join("home/alice");
        fs::create_dir_all(&home).unwrap();
        Self {
            _dir: dir,
            root,
            home,
        }
    }

    /// Creates `<prefix>/bin/<APP>` and returns the executable path.
    fn install_layout(&self, prefix: &Path) -> PathBuf {
        let bin = prefix.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let exe = bin.join(APP);
        fs::write(&exe, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    fn write_marker(&self, prefix: &Path, contents: &str) -> PathBuf {
        let dir = prefix.join("share").join(APP);
        fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("gpui-auto-update.managed");
        fs::write(&marker, contents).unwrap();
        marker
    }

    /// A complete managed install at `~/.local/opt/demo`.
    fn managed_install(&self) -> (PathBuf, PathBuf) {
        let prefix = self.home.join(".local/opt/demo");
        let exe = self.install_layout(&prefix);
        self.write_marker(&prefix, MARKER);
        (prefix, exe)
    }

    fn inputs(&self, exe: &Path) -> DetectionInputs {
        DetectionInputs {
            app_name: APP.to_owned(),
            arch: "x86_64".to_owned(),
            euid: current_euid(),
            executable: exe.to_path_buf(),
            home: Some(self.home.clone()),
            root: self.root.clone(),
        }
    }
}

/// The effective uid, read from a file this process just created.
fn current_euid() -> u32 {
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().metadata().unwrap().uid()
}

#[test]
fn marked_user_local_install_is_self_managed() {
    let fx = Fixture::new();
    let (prefix, exe) = fx.managed_install();

    let detection = detect(&fx.inputs(&exe));

    assert_eq!(detection.capability(), &Capability::SelfManaged);
    assert_eq!(detection.reason(), &DetectionReason::ManagedInstall);
    let install = detection.install().expect("managed install details");
    assert_eq!(install.prefix(), prefix.as_path());
    assert_eq!(install.executable(), exe.as_path());
    assert_eq!(install.app_name(), APP);
}

fn assert_unsupported(detection: &gpui_auto_update_linux::Detection, reason: DetectionReason) {
    assert_eq!(detection.capability(), &Capability::Unsupported);
    assert_eq!(detection.reason(), &reason);
    assert!(detection.install().is_none());
}

#[test]
fn unmarked_install_is_unsupported() {
    let fx = Fixture::new();
    let exe = fx.install_layout(&fx.home.join(".local/opt/demo"));

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::MissingMarker);
}

#[test]
fn marker_with_unexpected_contents_is_unsupported() {
    for contents in [
        "",
        "gpui-auto-update managed-install 1\napp=demo",
        "gpui-auto-update managed-install 1\napp=other\n",
        "gpui-auto-update managed-install 2\napp=demo\n",
        "gpui-auto-update managed-install 1\napp=demo\nextra\n",
        "GPUI-AUTO-UPDATE MANAGED-INSTALL 1\napp=demo\n",
    ] {
        let fx = Fixture::new();
        let prefix = fx.home.join(".local/opt/demo");
        let exe = fx.install_layout(&prefix);
        fx.write_marker(&prefix, contents);

        assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::InvalidMarker);
    }
}

#[test]
fn oversized_marker_is_unsupported() {
    let fx = Fixture::new();
    let prefix = fx.home.join(".local/opt/demo");
    let exe = fx.install_layout(&prefix);
    fx.write_marker(&prefix, &format!("{MARKER}{}", "x".repeat(1 << 20)));

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::InvalidMarker);
}

#[test]
fn symlinked_marker_is_unsupported() {
    let fx = Fixture::new();
    let prefix = fx.home.join(".local/opt/demo");
    let exe = fx.install_layout(&prefix);
    let real = fx.home.join("marker-elsewhere");
    fs::write(&real, MARKER).unwrap();
    let dir = prefix.join("share").join(APP);
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(&real, dir.join("gpui-auto-update.managed")).unwrap();

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::InvalidMarker);
}

#[test]
fn marker_directory_is_unsupported() {
    let fx = Fixture::new();
    let prefix = fx.home.join(".local/opt/demo");
    let exe = fx.install_layout(&prefix);
    fs::create_dir_all(
        prefix
            .join("share")
            .join(APP)
            .join("gpui-auto-update.managed"),
    )
    .unwrap();

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::InvalidMarker);
}

#[test]
fn executable_outside_a_bin_directory_is_an_unknown_layout() {
    let fx = Fixture::new();
    let build = fx.home.join("src/demo/target/release");
    fs::create_dir_all(&build).unwrap();
    let exe = build.join(APP);
    fs::write(&exe, b"").unwrap();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::UnknownLayout);
}

#[test]
fn executable_with_a_different_name_is_an_unknown_layout() {
    let fx = Fixture::new();
    let (prefix, _) = fx.managed_install();
    let renamed = prefix.join("bin/demo-renamed");
    fs::write(&renamed, b"").unwrap();
    fs::set_permissions(&renamed, fs::Permissions::from_mode(0o755)).unwrap();

    assert_unsupported(
        &detect(&fx.inputs(&renamed)),
        DetectionReason::UnknownLayout,
    );
}

#[test]
fn launching_through_a_symlink_resolves_to_the_managed_install() {
    let fx = Fixture::new();
    let (prefix, exe) = fx.managed_install();
    let local_bin = fx.home.join(".local/bin");
    fs::create_dir_all(&local_bin).unwrap();
    let link = local_bin.join(APP);
    std::os::unix::fs::symlink(&exe, &link).unwrap();

    let detection = detect(&fx.inputs(&link));

    assert_eq!(detection.capability(), &Capability::SelfManaged);
    assert_eq!(detection.install().unwrap().prefix(), prefix.as_path());
}

#[test]
fn missing_executable_is_unsupported() {
    let fx = Fixture::new();
    let exe = fx.home.join(".local/opt/demo/bin/demo");

    assert_unsupported(
        &detect(&fx.inputs(&exe)),
        DetectionReason::ExecutableUnresolvable,
    );
}

fn assert_externally_managed(
    detection: &gpui_auto_update_linux::Detection,
    manager: Option<&str>,
    reason: DetectionReason,
) {
    assert_eq!(
        detection.capability(),
        &Capability::ExternallyManaged {
            manager: manager.map(str::to_owned),
        }
    );
    assert_eq!(detection.reason(), &reason);
    assert!(detection.install().is_none());
}

#[test]
fn root_session_is_unsupported_even_for_a_managed_install() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    let inputs = DetectionInputs {
        euid: 0,
        ..fx.inputs(&exe)
    };

    assert_unsupported(&detect(&inputs), DetectionReason::RootSession);
}

#[test]
fn supported_architectures_are_x86_64_and_aarch64() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    for arch in ["x86_64", "aarch64"] {
        let inputs = DetectionInputs {
            arch: arch.to_owned(),
            ..fx.inputs(&exe)
        };
        assert_eq!(detect(&inputs).capability(), &Capability::SelfManaged);
    }
    for arch in ["x86", "arm", "riscv64", "powerpc64", ""] {
        let inputs = DetectionInputs {
            arch: arch.to_owned(),
            ..fx.inputs(&exe)
        };
        assert_unsupported(
            &detect(&inputs),
            DetectionReason::UnsupportedArchitecture(arch.to_owned()),
        );
    }
}

#[test]
fn invalid_application_names_are_unsupported() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    for name in [
        "",
        ".",
        "..",
        ".hidden",
        "a/b",
        "with space",
        &"x".repeat(65),
    ] {
        let inputs = DetectionInputs {
            app_name: name.to_owned(),
            ..fx.inputs(&exe)
        };
        assert_unsupported(&detect(&inputs), DetectionReason::InvalidAppName);
    }
}

#[test]
fn marked_install_outside_home_is_unsupported() {
    let fx = Fixture::new();
    let prefix = fx.root.join("srv/apps/demo");
    let exe = fx.install_layout(&prefix);
    fx.write_marker(&prefix, MARKER);

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::OutsideHome);
}

#[test]
fn another_users_home_is_outside_home() {
    let fx = Fixture::new();
    let prefix = fx.root.join("home/alice-other/.local/opt/demo");
    let exe = fx.install_layout(&prefix);
    fx.write_marker(&prefix, MARKER);

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::OutsideHome);
}

#[test]
fn home_itself_is_never_a_managed_prefix() {
    let fx = Fixture::new();
    let exe = fx.install_layout(&fx.home);
    fx.write_marker(&fx.home, MARKER);

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::OutsideHome);
}

#[test]
fn unknown_home_is_unsupported() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    let inputs = DetectionInputs {
        home: None,
        ..fx.inputs(&exe)
    };
    assert_unsupported(&detect(&inputs), DetectionReason::HomeUnresolvable);

    let inputs = DetectionInputs {
        home: Some(fx.root.join("home/nobody")),
        ..fx.inputs(&exe)
    };
    assert_unsupported(&detect(&inputs), DetectionReason::HomeUnresolvable);
}

#[test]
fn system_wide_installs_are_externally_managed() {
    for prefix in ["usr", "usr/local", "opt/demo", "app"] {
        let fx = Fixture::new();
        let prefix = fx.root.join(prefix);
        let exe = fx.install_layout(&prefix);
        // Even a marker does not make a system-wide install self-managed.
        fx.write_marker(&prefix, MARKER);

        assert_externally_managed(
            &detect(&fx.inputs(&exe)),
            None,
            DetectionReason::SystemInstall,
        );
    }
}

#[test]
fn package_manager_stores_name_their_manager() {
    let cases = [
        ("nix/store/abc123-demo-1.0", "Nix"),
        ("snap/demo/42", "Snap"),
        ("home/linuxbrew/.linuxbrew/Cellar/demo/1.0", "Homebrew"),
        ("home/alice/.linuxbrew/Cellar/demo/1.0", "Homebrew"),
        (
            "var/lib/flatpak/app/org.example.Demo/current/active/files",
            "Flatpak",
        ),
        (
            "home/alice/.local/share/flatpak/app/org.example.Demo/x86_64/stable/active/files",
            "Flatpak",
        ),
    ];
    for (prefix, manager) in cases {
        let fx = Fixture::new();
        let prefix = fx.root.join(prefix);
        let exe = fx.install_layout(&prefix);
        fx.write_marker(&prefix, MARKER);

        assert_externally_managed(
            &detect(&fx.inputs(&exe)),
            Some(manager),
            DetectionReason::PackageManager,
        );
    }
}

#[test]
fn flatpak_sandbox_is_externally_managed() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    fs::write(fx.root.join(".flatpak-info"), b"[Application]\n").unwrap();

    assert_externally_managed(
        &detect(&fx.inputs(&exe)),
        Some("Flatpak"),
        DetectionReason::PackageManager,
    );
}

#[test]
fn install_owned_by_another_user_is_unsupported() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    let inputs = DetectionInputs {
        euid: current_euid().wrapping_add(1).max(1),
        ..fx.inputs(&exe)
    };

    assert_unsupported(&detect(&inputs), DetectionReason::NotUserOwned);
}

#[test]
fn non_executable_entry_point_is_an_unknown_layout() {
    let fx = Fixture::new();
    let (_, exe) = fx.managed_install();
    fs::set_permissions(&exe, fs::Permissions::from_mode(0o644)).unwrap();

    assert_unsupported(&detect(&fx.inputs(&exe)), DetectionReason::UnknownLayout);
}

/// Restores a directory's permissions so the temp dir can be removed.
struct RestoreMode(PathBuf);

impl Drop for RestoreMode {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn non_writable_parent_is_temporarily_unavailable() {
    if current_euid() == 0 {
        // Root bypasses permission bits, so the parent stays writable.
        return;
    }
    let fx = Fixture::new();
    let (prefix, exe) = fx.managed_install();
    let parent = prefix.parent().unwrap().to_path_buf();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    let _restore = RestoreMode(parent);

    let detection = detect(&fx.inputs(&exe));

    assert_eq!(detection.capability(), &Capability::TemporarilyUnavailable);
    assert_eq!(detection.reason(), &DetectionReason::ParentNotWritable);
    assert!(detection.install().is_none());
}

#[test]
fn detection_leaves_no_files_behind() {
    let fx = Fixture::new();
    let (prefix, exe) = fx.managed_install();
    let parent = prefix.parent().unwrap();

    assert!(detect(&fx.inputs(&exe)).capability().can_self_update());

    let entries: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, [std::ffi::OsString::from(APP)]);
}

#[test]
fn every_denial_still_reports_a_capability_with_an_explanation() {
    let fx = Fixture::new();
    let exe = fx.install_layout(&fx.home.join(".local/opt/demo"));

    let detection = detect(&fx.inputs(&exe));

    assert!(!detection.capability().can_self_update());
    assert!(detection.capability().denial().is_some());
    assert!(!detection.reason().to_string().is_empty());
}
