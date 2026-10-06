//! The Windows backend end to end against loopback feeds: architecture feed
//! selection, verified staging, version confirmation, installer handoff
//! through an injected launcher, and portable in-place replacement.
//!
//! Process creation is the only part replaced by a fake here; real launches
//! are covered by `tests/windows_launch.rs` on Windows.

mod support;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_auto_update_core::feed::Arch;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{
    Capability, CheckKind, CheckOutcome, ErrorKind, UpdateCoordinator, UpdateError, UpdateState,
};
use gpui_auto_update_windows::{
    InnoSetup, InstallTarget, InstallerCommand, InstallerLauncher, InstallerStrategy,
    LaunchedInstaller, PortableExecutable, UpdateStrategy, WindowsBackend, WindowsHandoff,
    WindowsUpdateConfig,
};
use support::{MACHINE_AMD64, MACHINE_ARM64, PeImage, feed, item, loopback_policy, trusted_key};
use url::Url;

// ---------------------------------------------------------------------------
// Fake process creation

#[derive(Clone, Debug, Default)]
struct Launches(Arc<Mutex<Vec<InstallerCommand>>>);

impl Launches {
    fn all(&self) -> Vec<InstallerCommand> {
        self.0.lock().unwrap().clone()
    }
}

/// What the fake launcher does when asked to start an installer.
#[derive(Clone, Debug)]
enum Behavior {
    /// The process starts and keeps running.
    Runs,
    /// The process starts and exits at once with this code.
    Exits(i32),
    /// Process creation fails with this OS error code.
    Fails(i32),
}

#[derive(Debug)]
struct FakeLauncher {
    launches: Launches,
    behavior: Behavior,
}

struct FakeProcess(Option<i32>);

impl LaunchedInstaller for FakeProcess {
    fn try_wait(&mut self) -> io::Result<Option<i32>> {
        Ok(self.0)
    }
}

impl InstallerLauncher for FakeLauncher {
    fn launch(&self, command: &InstallerCommand) -> io::Result<Box<dyn LaunchedInstaller>> {
        self.launches.0.lock().unwrap().push(command.clone());
        match self.behavior {
            Behavior::Runs => Ok(Box::new(FakeProcess(None))),
            Behavior::Exits(code) => Ok(Box::new(FakeProcess(Some(code)))),
            Behavior::Fails(code) => Err(io::Error::from_raw_os_error(code)),
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture

struct Fixture {
    _root: tempfile::TempDir,
    install_dir: PathBuf,
    staging: PathBuf,
    launches: Launches,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let install_dir = root.path().join("Programs").join("Demo");
        fs::create_dir_all(&install_dir).unwrap();
        fs::write(install_dir.join("demo.exe"), b"old executable").unwrap();
        let staging = root.path().join("Temp").join("demo-updates");
        Self {
            install_dir,
            staging,
            _root: root,
            launches: Launches::default(),
        }
    }

    fn target(&self) -> InstallTarget {
        InstallTarget::new(self.install_dir.clone(), "demo.exe").unwrap()
    }

    fn config(&self, strategy: UpdateStrategy, feeds: &[(Arch, &str)]) -> WindowsUpdateConfig {
        self.config_with(strategy, feeds, Behavior::Runs)
    }

    fn config_with(
        &self,
        strategy: UpdateStrategy,
        feeds: &[(Arch, &str)],
        behavior: Behavior,
    ) -> WindowsUpdateConfig {
        // Portable installs keep the default staging root, inside the
        // install dir, so the final rename never crosses volumes.
        let installer = matches!(strategy, UpdateStrategy::Installer(_));
        let mut config = WindowsUpdateConfig::new(v("1.0.0"), trusted_key(), strategy)
            .with_arch(Arch::X86_64)
            .with_fetch_policy(loopback_policy())
            .with_install_target(self.target())
            .with_launch_grace(Duration::from_millis(30))
            .with_launcher(FakeLauncher {
                launches: self.launches.clone(),
                behavior,
            });
        if installer {
            config = config.with_staging_root(self.staging.clone());
        }
        for (arch, url) in feeds {
            config = config.with_feed(*arch, Url::parse(url).unwrap());
        }
        config
    }

    fn exe(&self) -> PathBuf {
        self.install_dir.join("demo.exe")
    }

    /// Entries left in the staging root.
    fn staged_entries(&self) -> Vec<PathBuf> {
        match fs::read_dir(&self.staging) {
            Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

fn v(text: &str) -> ReleaseVersion {
    ReleaseVersion::parse(text).unwrap()
}

/// Serves a feed at `/feed.xml` with one x86_64 entry for `version` whose
/// artifact (`served`, signed over `signed`) is at `/<artifact_name>`.
fn serve_release(
    version: &str,
    artifact_name: &'static str,
    served: Vec<u8>,
    signed: &[u8],
) -> String {
    let base = serve_lazy();
    let url = format!("{}/{artifact_name}", base.url);
    let xml = feed(&[item(version, "x86_64", &url, &served, signed)]);
    base.set(vec![
        ("/feed.xml", xml),
        (leak(format!("/{artifact_name}")), served),
    ]);
    format!("{}/feed.xml", base.url)
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

type Routes = Arc<Mutex<Vec<(&'static str, Vec<u8>)>>>;

/// A loopback server whose routes are set after its address is known.
struct LazyServer {
    url: String,
    routes: Routes,
}

impl LazyServer {
    fn set(&self, routes: Vec<(&'static str, Vec<u8>)>) {
        *self.routes.lock().unwrap() = routes;
    }
}

fn serve_lazy() -> LazyServer {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = server.server_addr().to_ip().unwrap();
    let routes: Routes = Arc::default();
    let shared = routes.clone();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let body = shared
                .lock()
                .unwrap()
                .iter()
                .find(|(path, _)| *path == request.url())
                .map(|(_, body)| body.clone());
            let _ = match body {
                Some(body) => request.respond(
                    tiny_http::Response::from_data(body).with_chunked_threshold(usize::MAX),
                ),
                None => request.respond(tiny_http::Response::empty(404)),
            };
        }
    });
    LazyServer {
        url: format!("http://{addr}"),
        routes,
    }
}

/// Checks and stages with `backend`; returns the coordinator.
fn check_and_stage(backend: &WindowsBackend) -> (UpdateCoordinator, Result<(), UpdateError>) {
    let coordinator = UpdateCoordinator::new(backend.check_source(), Capability::SelfManaged);
    match coordinator.check(CheckKind::Manual).unwrap() {
        CheckOutcome::UpdateAvailable(_) => {}
        CheckOutcome::UpToDate => panic!("the fixture release must be newer"),
    }
    let result = backend.stage(&coordinator);
    (coordinator, result)
}

fn inno() -> UpdateStrategy {
    UpdateStrategy::inno_setup(InnoSetup::new())
}

// ---------------------------------------------------------------------------
// Configuration and architecture

#[test]
fn the_backend_can_be_shared_with_background_work() {
    fn shareable<T: Send + Sync + Clone + 'static>() {}
    shareable::<WindowsBackend>();
    shareable::<WindowsUpdateConfig>();
}

#[test]
fn the_feed_is_chosen_by_the_configured_architecture() {
    let fixture = Fixture::new();
    let installer_x64 = PeImage::installer("1.5.0").build();
    let installer_arm = PeImage::installer("1.6.0").build();
    let base = serve_lazy();
    let x64_url = format!("{}/x64/setup.exe", base.url);
    let arm_url = format!("{}/arm/setup.exe", base.url);
    base.set(vec![
        (
            "/x64.xml",
            feed(&[item(
                "1.5.0",
                "x86_64",
                &x64_url,
                &installer_x64,
                &installer_x64,
            )]),
        ),
        (
            "/arm.xml",
            feed(&[item(
                "1.6.0",
                "aarch64",
                &arm_url,
                &installer_arm,
                &installer_arm,
            )]),
        ),
        ("/x64/setup.exe", installer_x64),
        ("/arm/setup.exe", installer_arm),
    ]);
    let feeds = [
        (Arch::X86_64, leak(format!("{}/x64.xml", base.url))),
        (Arch::Aarch64, leak(format!("{}/arm.xml", base.url))),
    ];

    for (arch, expected) in [(Arch::X86_64, "1.5.0"), (Arch::Aarch64, "1.6.0")] {
        let backend = WindowsBackend::new(fixture.config(inno(), &feeds).with_arch(arch)).unwrap();
        assert_eq!(backend.arch(), arch);
        assert_eq!(backend.feed_url().as_str(), feeds[arch as usize].1);
        let coordinator = UpdateCoordinator::new(backend.check_source(), Capability::SelfManaged);
        match coordinator.check(CheckKind::Manual).unwrap() {
            CheckOutcome::UpdateAvailable(update) => assert_eq!(update.version, expected),
            CheckOutcome::UpToDate => panic!("expected an update for {arch}"),
        }
    }
}

#[test]
fn a_shared_feed_only_offers_entries_for_the_configured_architecture() {
    let fixture = Fixture::new();
    let x64 = PeImage::installer("1.5.0").build();
    let arm = PeImage::installer("1.9.0").build();
    let base = serve_lazy();
    let xml = feed(&[
        item("1.9.0", "aarch64", &format!("{}/arm", base.url), &arm, &arm),
        item("1.5.0", "x86_64", &format!("{}/x64", base.url), &x64, &x64),
    ]);
    base.set(vec![("/feed.xml", xml), ("/x64", x64), ("/arm", arm)]);
    let url = leak(format!("{}/feed.xml", base.url));
    let backend = WindowsBackend::new(fixture.config(inno(), &[(Arch::X86_64, url)])).unwrap();
    let (coordinator, result) = check_and_stage(&backend);
    result.unwrap();
    assert!(matches!(coordinator.state(), UpdateState::Staged(u) if u.version == "1.5.0"));
}

#[test]
fn a_missing_feed_for_the_architecture_is_a_configuration_error() {
    let fixture = Fixture::new();
    let error = WindowsBackend::new(
        fixture
            .config(
                inno(),
                &[(Arch::X86_64, "https://updates.example.com/x64.xml")],
            )
            .with_arch(Arch::Aarch64),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
    assert!(error.diagnostic().unwrap().contains("aarch64"));

    let error = WindowsBackend::new(fixture.config(inno(), &[])).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Configuration);
}

// ---------------------------------------------------------------------------
// Capability

#[test]
fn a_writable_install_is_self_managed() {
    let fixture = Fixture::new();
    let backend = WindowsBackend::new(fixture.config(
        inno(),
        &[(Arch::X86_64, "https://updates.example.com/x64.xml")],
    ))
    .unwrap();
    assert_eq!(backend.capability(), Capability::SelfManaged);
    // The writability probe leaves nothing behind.
    let mut entries: Vec<_> = fs::read_dir(&fixture.install_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    entries.sort();
    assert_eq!(entries, ["demo.exe"]);
}

#[test]
fn a_missing_install_dir_is_unsupported() {
    let fixture = Fixture::new();
    let backend = WindowsBackend::new(
        fixture
            .config(
                inno(),
                &[(Arch::X86_64, "https://updates.example.com/x64.xml")],
            )
            .with_install_target(
                InstallTarget::new(fixture.install_dir.join("missing"), "demo.exe").unwrap(),
            ),
    )
    .unwrap();
    assert_eq!(backend.capability(), Capability::Unsupported);
}

#[cfg(unix)]
#[test]
fn a_read_only_install_dir_is_unsupported_because_updates_never_elevate() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fs::set_permissions(&fixture.install_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let backend = WindowsBackend::new(fixture.config(
        inno(),
        &[(Arch::X86_64, "https://updates.example.com/x64.xml")],
    ))
    .unwrap();
    let capability = backend.capability();
    fs::set_permissions(&fixture.install_dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(capability, Capability::Unsupported);
}

#[test]
fn a_launcher_that_cannot_run_installers_disables_updates() {
    #[derive(Debug)]
    struct NoProcesses;
    impl InstallerLauncher for NoProcesses {
        fn is_supported(&self) -> bool {
            false
        }
        fn launch(&self, _: &InstallerCommand) -> io::Result<Box<dyn LaunchedInstaller>> {
            unreachable!()
        }
    }
    let fixture = Fixture::new();
    let backend = WindowsBackend::new(
        fixture
            .config(
                inno(),
                &[(Arch::X86_64, "https://updates.example.com/x64.xml")],
            )
            .with_launcher(NoProcesses),
    )
    .unwrap();
    assert_eq!(backend.capability(), Capability::Unsupported);
}

// ---------------------------------------------------------------------------
// Installer handoff

#[test]
fn a_verified_installer_is_staged_then_launched_with_per_user_handoff_args() {
    let fixture = Fixture::new();
    let installer = PeImage::installer("1.5.0").build();
    let feed_url = serve_release(
        "1.5.0",
        "Demo-Setup-1.5.0.exe",
        installer.clone(),
        &installer,
    );
    let backend =
        WindowsBackend::new(fixture.config(inno(), &[(Arch::X86_64, leak(feed_url))])).unwrap();

    let (coordinator, result) = check_and_stage(&backend);
    result.unwrap();
    assert!(matches!(coordinator.state(), UpdateState::Staged(u) if u.version == "1.5.0"));
    assert!(
        fixture.launches.all().is_empty(),
        "staging must not run the installer"
    );
    let staged = backend.staged_path().unwrap();
    assert!(staged.starts_with(&fixture.staging));
    assert_eq!(staged.file_name().unwrap(), "setup.exe");
    assert_eq!(fs::read(&staged).unwrap(), installer);

    assert_eq!(backend.install().unwrap(), WindowsHandoff::QuitForInstaller);
    let launches = fixture.launches.all();
    assert_eq!(launches.len(), 1);
    let command = &launches[0];
    assert_eq!(command.program(), staged);
    let install_dir = fixture.install_dir.to_str().unwrap();
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
            &format!("/DIR=\"{install_dir}\""),
        ]
    );
    // The running installation is left for the installer to replace.
    assert_eq!(fs::read(fixture.exe()).unwrap(), b"old executable");
    assert_eq!(
        backend.relaunch_handoff().unwrap(),
        WindowsHandoff::QuitForInstaller
    );
}

#[test]
fn a_tampered_installer_is_rejected_and_never_launched() {
    let fixture = Fixture::new();
    let genuine = PeImage::installer("1.5.0").build();
    let tampered = PeImage::installer("1.5.0").with_trailer(b"evil").build();
    let mut same_length = genuine.clone();
    *same_length.last_mut().unwrap() ^= 0xff;
    for served in [same_length, tampered] {
        let feed_url = serve_release("1.5.0", "setup.exe", served, &genuine);
        let backend =
            WindowsBackend::new(fixture.config(inno(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
        let (coordinator, result) = check_and_stage(&backend);
        let error = result.unwrap_err();
        assert!(
            matches!(
                error.kind(),
                ErrorKind::Signature | ErrorKind::LengthMismatch
            ),
            "{error:?}"
        );
        assert!(matches!(coordinator.state(), UpdateState::Failed(_)));
        assert_eq!(backend.staged_path(), None);
        assert_eq!(
            backend.install().unwrap_err().kind(),
            ErrorKind::InvalidState
        );
        assert!(fixture.launches.all().is_empty());
        assert!(
            fixture
                .staged_entries()
                .iter()
                .all(|p| p.is_dir() && fs::read_dir(p).unwrap().next().is_none())
        );
    }
}

#[test]
fn an_installer_modified_after_staging_is_not_launched() {
    let fixture = Fixture::new();
    let installer = PeImage::installer("1.5.0").build();
    let feed_url = serve_release("1.5.0", "setup.exe", installer.clone(), &installer);
    let backend =
        WindowsBackend::new(fixture.config(inno(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
    let (_, result) = check_and_stage(&backend);
    result.unwrap();
    let staged = backend.staged_path().unwrap();
    let mut bytes = fs::read(&staged).unwrap();
    bytes[0x300] ^= 1;
    fs::write(&staged, bytes).unwrap();

    assert_eq!(backend.install().unwrap_err().kind(), ErrorKind::Signature);
    assert!(fixture.launches.all().is_empty());
}

#[test]
fn a_signed_installer_for_another_version_is_rejected() {
    let fixture = Fixture::new();
    // Correctly signed by the release key, but it is the 1.4.0 installer
    // relabeled as 1.5.0 in the (unsigned) feed entry.
    let old = PeImage::installer("1.4.0").build();
    let feed_url = serve_release("1.5.0", "setup.exe", old.clone(), &old);
    let backend =
        WindowsBackend::new(fixture.config(inno(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
    let (coordinator, result) = check_and_stage(&backend);
    assert_eq!(result.unwrap_err().kind(), ErrorKind::ArchiveValidation);
    assert!(
        matches!(coordinator.state(), UpdateState::Failed(e) if e.kind() == ErrorKind::ArchiveValidation)
    );
    assert_eq!(backend.staged_path(), None);
    assert!(fixture.launches.all().is_empty());
}

#[test]
fn failure_to_start_the_installer_is_a_structured_error() {
    let fixture = Fixture::new();
    let installer = PeImage::installer("1.5.0").build();
    let feed_url = leak(serve_release(
        "1.5.0",
        "setup.exe",
        installer.clone(),
        &installer,
    ));

    // ERROR_FILE_NOT_FOUND, ERROR_ELEVATION_REQUIRED, and an installer that
    // exits with an error during the launch grace period.
    for (behavior, message_part) in [
        (Behavior::Fails(2), None),
        (Behavior::Fails(740), Some("administrator")),
        (Behavior::Exits(1), None),
    ] {
        let backend = WindowsBackend::new(fixture.config_with(
            inno(),
            &[(Arch::X86_64, feed_url)],
            behavior.clone(),
        ))
        .unwrap();
        let (_, result) = check_and_stage(&backend);
        result.unwrap();
        let error = backend.install().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::HelperLaunch, "{behavior:?}");
        assert!(error.diagnostic().is_some(), "{behavior:?}");
        if let Some(part) = message_part {
            assert!(error.message().contains(part), "{}", error.message());
        }
        // The update stays staged so the user can retry.
        assert!(backend.staged_path().is_some());
    }
}

#[test]
fn an_installer_that_finishes_within_the_grace_period_succeeds() {
    let fixture = Fixture::new();
    let installer = PeImage::installer("1.5.0").build();
    let feed_url = serve_release("1.5.0", "setup.exe", installer.clone(), &installer);
    let backend = WindowsBackend::new(fixture.config_with(
        inno(),
        &[(Arch::X86_64, leak(feed_url))],
        Behavior::Exits(0),
    ))
    .unwrap();
    let (_, result) = check_and_stage(&backend);
    result.unwrap();
    assert_eq!(backend.install().unwrap(), WindowsHandoff::QuitForInstaller);
}

#[test]
fn the_strategy_is_declared_not_inferred_from_the_artifact_name() {
    // An artifact named like an MSI or an archive is still handed to the
    // declared Inno Setup strategy, under the strategy's own file name.
    let fixture = Fixture::new();
    let installer = PeImage::installer("1.5.0").build();
    for name in ["demo-1.5.0.msi", "demo-portable-1.5.0.zip", "update"] {
        let feed_url = serve_release(
            "1.5.0",
            leak(name.to_owned()),
            installer.clone(),
            &installer,
        );
        let backend =
            WindowsBackend::new(fixture.config(inno(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
        let (_, result) = check_and_stage(&backend);
        result.unwrap();
        assert_eq!(
            backend.staged_path().unwrap().file_name().unwrap(),
            "setup.exe"
        );
        assert_eq!(backend.install().unwrap(), WindowsHandoff::QuitForInstaller);
    }
    assert_eq!(fixture.launches.all().len(), 3);

    // And an artifact named setup.exe is not run when the declared strategy
    // is portable.
    let fixture = Fixture::new();
    let exe = PeImage::executable(MACHINE_AMD64, "1.5.0").build();
    let feed_url = serve_release("1.5.0", "Demo-Setup.exe", exe.clone(), &exe);
    let backend = WindowsBackend::new(fixture.config(
        UpdateStrategy::portable(PortableExecutable::new()),
        &[(Arch::X86_64, leak(feed_url))],
    ))
    .unwrap();
    let (_, result) = check_and_stage(&backend);
    result.unwrap();
    assert!(matches!(
        backend.install().unwrap(),
        WindowsHandoff::Restart { .. }
    ));
    assert!(fixture.launches.all().is_empty());
}

/// An MSI-like strategy implemented outside the crate, as the extension
/// point documents.
#[derive(Debug)]
struct FakeMsi;

impl InstallerStrategy for FakeMsi {
    fn name(&self) -> &str {
        "MSI"
    }
    fn staged_file_name(&self) -> &str {
        "update.msi"
    }
    fn confirm_version(
        &self,
        artifact: &Path,
        expected: &ReleaseVersion,
    ) -> Result<(), UpdateError> {
        gpui_auto_update_windows::confirm_embedded_version(artifact, "ProductVersion", expected)
    }
    fn command(
        &self,
        artifact: &Path,
        target: &InstallTarget,
    ) -> Result<InstallerCommand, UpdateError> {
        Ok(InstallerCommand::new(r"C:\Windows\System32\msiexec.exe")
            .raw_arg("/i")
            .raw_arg(format!("\"{}\"", artifact.display()))
            .raw_arg("/passive")
            .raw_arg(format!("INSTALLDIR=\"{}\"", target.install_dir().display())))
    }
}

#[test]
fn custom_installer_strategies_plug_into_the_same_verified_flow() {
    let fixture = Fixture::new();
    let package = PeImage::installer("1.5.0").build();
    let feed_url = serve_release("1.5.0", "demo.msi", package.clone(), &package);
    let backend = WindowsBackend::new(fixture.config(
        UpdateStrategy::installer(FakeMsi),
        &[(Arch::X86_64, leak(feed_url))],
    ))
    .unwrap();
    let (_, result) = check_and_stage(&backend);
    result.unwrap();
    let staged = backend.staged_path().unwrap();
    assert_eq!(staged.file_name().unwrap(), "update.msi");
    assert_eq!(backend.install().unwrap(), WindowsHandoff::QuitForInstaller);
    let launches = fixture.launches.all();
    assert_eq!(
        launches[0].program(),
        Path::new(r"C:\Windows\System32\msiexec.exe")
    );
    assert_eq!(launches[0].args()[1], format!("\"{}\"", staged.display()));
}

// ---------------------------------------------------------------------------
// Portable in-place updates

fn portable() -> UpdateStrategy {
    UpdateStrategy::portable(PortableExecutable::new())
}

#[test]
fn a_portable_executable_is_replaced_in_place_and_relaunched() {
    let fixture = Fixture::new();
    let exe = PeImage::executable(MACHINE_AMD64, "1.5.0").build();
    let feed_url = serve_release("1.5.0", "demo.exe", exe.clone(), &exe);
    let backend =
        WindowsBackend::new(fixture.config(portable(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
    let (coordinator, result) = check_and_stage(&backend);
    result.unwrap();
    assert!(matches!(coordinator.state(), UpdateState::Staged(_)));
    let staged = backend.staged_path().unwrap();
    assert!(staged.starts_with(&fixture.install_dir));
    // Staging leaves the running executable alone.
    assert_eq!(fs::read(fixture.exe()).unwrap(), b"old executable");

    let handoff = backend.install().unwrap();
    assert_eq!(
        handoff,
        WindowsHandoff::Restart {
            executable: fixture.exe()
        }
    );
    assert_eq!(fs::read(fixture.exe()).unwrap(), exe);
    assert_eq!(
        fs::read(fixture.install_dir.join("demo.exe.previous")).unwrap(),
        b"old executable"
    );
    assert!(!staged.exists());
    assert!(fixture.launches.all().is_empty());
    assert_eq!(backend.relaunch_handoff().unwrap(), handoff);

    // The next launch removes the previous executable.
    let next = WindowsBackend::new(fixture.config(
        portable(),
        &[(Arch::X86_64, "https://updates.example.com/x64.xml")],
    ))
    .unwrap();
    assert_eq!(next.capability(), Capability::SelfManaged);
    assert!(!fixture.install_dir.join("demo.exe.previous").exists());
}

#[test]
fn a_portable_executable_for_another_architecture_is_rejected() {
    let fixture = Fixture::new();
    let arm = PeImage::executable(MACHINE_ARM64, "1.5.0").build();
    let feed_url = serve_release("1.5.0", "demo.exe", arm.clone(), &arm);
    let backend =
        WindowsBackend::new(fixture.config(portable(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
    let (_, result) = check_and_stage(&backend);
    let error = result.unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ArchiveValidation);
    assert!(error.diagnostic().unwrap().contains("aarch64"));
    assert_eq!(fs::read(fixture.exe()).unwrap(), b"old executable");
}

#[test]
fn a_staged_executable_that_was_replaced_is_not_installed() {
    let fixture = Fixture::new();
    let exe = PeImage::executable(MACHINE_AMD64, "1.5.0").build();
    let feed_url = serve_release("1.5.0", "demo.exe", exe.clone(), &exe);
    let backend =
        WindowsBackend::new(fixture.config(portable(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
    let (_, result) = check_and_stage(&backend);
    result.unwrap();
    let staged = backend.staged_path().unwrap();
    fs::remove_file(&staged).unwrap();
    fs::create_dir(&staged).unwrap();

    let error = backend.install().unwrap_err();
    assert!(
        matches!(error.kind(), ErrorKind::Signature | ErrorKind::Staging),
        "{error:?}"
    );
    assert_eq!(fs::read(fixture.exe()).unwrap(), b"old executable");
    assert!(!fixture.install_dir.join("demo.exe.previous").exists());
}

#[test]
fn a_portable_install_whose_executable_is_missing_is_not_modified() {
    let fixture = Fixture::new();
    let exe = PeImage::executable(MACHINE_AMD64, "1.5.0").build();
    let feed_url = serve_release("1.5.0", "demo.exe", exe.clone(), &exe);
    let backend =
        WindowsBackend::new(fixture.config(portable(), &[(Arch::X86_64, leak(feed_url))])).unwrap();
    let (_, result) = check_and_stage(&backend);
    result.unwrap();
    fs::remove_file(fixture.exe()).unwrap();
    let error = backend.install().unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Replacement);
    assert!(!fixture.exe().exists());
    assert!(!fixture.install_dir.join("demo.exe.previous").exists());
}
