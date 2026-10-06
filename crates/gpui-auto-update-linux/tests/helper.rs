//! The update helper, driven through real child processes.
//!
//! This test binary plays every role, the way a real application does: its
//! `main` first hands control to `run_helper_if_requested`, exactly as an
//! application's `main` must. When it is started under the fake application
//! name (`demo`, through a hard link placed in a temporary managed install)
//! it acts as that application; otherwise it runs the test cases.
//!
//! The fake application reads `share/demo/fake.conf` from its own prefix to
//! learn its version and how it behaves on start, and appends what it does
//! to the log file named by `FAKE_LOG`, which every process inherits. Tests
//! assert on that log and on the installation left on disk.
#![cfg(unix)]

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use std::os::unix::fs::MetadataExt as _;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer as _, SigningKey};
use gpui_auto_update_core::check::{FeedCheckSource, UpdateChecker};
use gpui_auto_update_core::download::ArtifactDownloader;
use gpui_auto_update_core::feed::{Arch, Os, UpdateTarget};
use gpui_auto_update_core::fetch::{FetchPolicy, HttpClient};
use gpui_auto_update_core::trust::TrustedKey;
use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{
    Capability, CheckKind, CheckOutcome, ErrorKind, UpdateCoordinator, UpdateError, UpdateEvent,
};
use gpui_auto_update_linux::{
    DetectionInputs, HandoffRequest, HelperCommand, HelperOutcome, LinuxUpdater,
    StartupConfirmation, confirm_startup, detect, marker_contents, take_diagnostic,
};

const APP: &str = "demo";
const LOG_ENV: &str = "FAKE_LOG";
const WAIT: Duration = Duration::from_secs(30);

fn main() -> ExitCode {
    gpui_auto_update_linux::run_helper_if_requested();

    let exe = std::env::current_exe().expect("current executable");
    if exe.file_name().is_some_and(|name| name == APP) {
        return fake_app::main(&exe);
    }
    run_tests()
}

// ---------------------------------------------------------------------------
// Test runner (the helper protocol owns stdout, so libtest cannot be used)

type Case = (&'static str, fn());

const CASES: &[Case] = &[
    (
        "helper_refuses_an_invalid_staged_release_before_the_host_quits",
        helper_refuses_an_invalid_staged_release_before_the_host_quits,
    ),
    (
        "helper_swaps_only_after_the_host_quits_and_keeps_a_confirmed_update",
        helper_swaps_only_after_the_host_quits_and_keeps_a_confirmed_update,
    ),
    (
        "failed_startup_restores_and_relaunches_the_previous_version",
        failed_startup_restores_and_relaunches_the_previous_version,
    ),
    (
        "unconfirmed_startup_keeps_the_running_version_and_reports_it",
        unconfirmed_startup_keeps_the_running_version_and_reports_it,
    ),
    (
        "replacement_failure_after_quit_is_reported_by_the_next_start",
        replacement_failure_after_quit_is_reported_by_the_next_start,
    ),
    (
        "linux_updater_stages_a_signed_release_and_hands_it_to_the_helper",
        linux_updater_stages_a_signed_release_and_hands_it_to_the_helper,
    ),
    (
        "startup_confirmation_without_a_helper_is_not_requested",
        startup_confirmation_without_a_helper_is_not_requested,
    ),
];

fn run_tests() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--list") {
        for (name, _) in CASES {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    let filters: Vec<&String> = args.iter().filter(|arg| !arg.starts_with('-')).collect();
    let selected: Vec<&Case> = CASES
        .iter()
        .filter(|(name, _)| filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
        .collect();
    println!("running {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, case) in selected {
        let result = std::panic::catch_unwind(case);
        println!(
            "test {name} ... {}",
            if result.is_ok() { "ok" } else { "FAILED" }
        );
        if result.is_err() {
            failed.push(*name);
        }
    }
    if failed.is_empty() {
        println!("test result: ok");
        ExitCode::SUCCESS
    } else {
        println!("test result: FAILED: {failed:?}");
        ExitCode::FAILURE
    }
}

// ---------------------------------------------------------------------------
// Fixtures

/// A temporary home with a managed install of the fake application and a
/// staging area next to it.
struct Fixture {
    _dir: tempfile::TempDir,
    parent: PathBuf,
    prefix: PathBuf,
    log: PathBuf,
}

impl Fixture {
    /// Installs version `1.0.0`, which behaves as `mode` when started.
    fn new(mode: &str) -> Self {
        // The fake application is a hard link to this binary, so the
        // temporary directory must be on the same filesystem as the build.
        let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let root = dir.path().canonicalize().unwrap();
        let parent = root.join("home/.local/opt");
        fs::create_dir_all(&parent).unwrap();
        let prefix = parent.join(APP);
        write_release(&prefix, "1.0.0", mode);
        Self {
            log: root.join("events.log"),
            _dir: dir,
            parent,
            prefix,
        }
    }

    /// Stages `version` next to the install the way the stager does: a
    /// private sibling directory holding the release root.
    fn stage(&self, version: &str, mode: &str) -> PathBuf {
        let directory = self
            .parent
            .join(format!(".{APP}.gpui-auto-update-staged-{version}-t3st"));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let staged = directory.join(format!("{APP}-{version}-linux-{}", std::env::consts::ARCH));
        write_release(&staged, version, mode);
        staged
    }

    /// Starts the installed application as the host of an update to
    /// `staged` and waits for it to exit.
    fn run_host(&self, staged: &Path, health_timeout_ms: u64, extra: &[&str]) {
        let status = Command::new(self.prefix.join("bin").join(APP))
            .arg("--fake-host")
            .arg(staged)
            .arg(health_timeout_ms.to_string())
            .args(extra)
            .env(LOG_ENV, &self.log)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "the host failed: {status}\n{}",
            self.events().join("\n")
        );
    }

    fn events(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Waits until the log contains `event`.
    fn wait_for(&self, event: &str) {
        let deadline = Instant::now() + WAIT;
        while !self.events().iter().any(|line| line == event) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {event:?}; events:\n{}",
                self.events().join("\n")
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until `condition` holds.
    fn wait_until(&self, what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting until {what}; events:\n{}",
                self.events().join("\n")
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Index of `event` in the log.
    fn position(&self, event: &str) -> usize {
        self.events()
            .iter()
            .position(|line| line == event)
            .unwrap_or_else(|| panic!("{event:?} not logged:\n{}", self.events().join("\n")))
    }

    fn installed_version(&self) -> String {
        fake_app::config(&self.prefix).0
    }

    /// Hidden siblings of the install other than the diagnostic file.
    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != APP && !name.ends_with("gpui-auto-update-diagnostic"))
            .collect();
        names.sort();
        names
    }

    fn has_diagnostic(&self) -> bool {
        self.parent
            .join(format!(".{APP}.gpui-auto-update-diagnostic"))
            .exists()
    }
}

/// Writes a managed-install layout whose executable is this test binary.
fn write_release(prefix: &Path, version: &str, mode: &str) {
    let bin = prefix.join("bin");
    let share = prefix.join("share").join(APP);
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&share).unwrap();
    fs::hard_link(std::env::current_exe().unwrap(), bin.join(APP)).unwrap();
    fs::write(share.join("gpui-auto-update.managed"), marker_contents(APP)).unwrap();
    fs::write(
        share.join("fake.conf"),
        format!("version={version}\nmode={mode}\n"),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Cases

fn helper_refuses_an_invalid_staged_release_before_the_host_quits() {
    let fixture = Fixture::new("confirm");
    let staged = fixture.stage("2.0.0", "confirm");
    fs::remove_file(
        staged
            .join("share")
            .join(APP)
            .join("gpui-auto-update.managed"),
    )
    .unwrap();

    // This process is the host; the helper is this same binary.
    let error = HelperCommand::new(std::env::current_exe().unwrap())
        .hand_off(&HandoffRequest::new(APP, &fixture.prefix, &staged).with_version("2.0.0"))
        .expect_err("the helper must refuse a staged release without a marker");

    let error = UpdateError::from(error);
    assert_eq!(error.kind(), ErrorKind::HelperLaunch);
    let diagnostic = error.diagnostic().unwrap_or_default().to_owned();
    assert!(diagnostic.contains("staged"), "{diagnostic}");
    assert_eq!(fixture.installed_version(), "1.0.0");
    assert!(
        staged.exists(),
        "a refused handoff leaves the staged release alone"
    );
    assert!(!fixture.has_diagnostic());
}

fn helper_swaps_only_after_the_host_quits_and_keeps_a_confirmed_update() {
    let fixture = Fixture::new("confirm");
    let staged = fixture.stage("2.0.0", "confirm");

    fixture.run_host(&staged, 20_000, &[]);
    fixture.wait_for("start 2.0.0");
    fixture.wait_for("confirm 2.0.0 confirmed");
    fixture.wait_until("the helper cleaned up", || fixture.leftovers().is_empty());

    // The host was acknowledged before it began quitting, and the install
    // was still the old version after it had saved and right before it
    // exited.
    let acknowledged = fixture.position("host acknowledged");
    let saved = fixture.position("host saved with 1.0.0 installed");
    let exited = fixture.position("host exiting");
    let started = fixture.position("start 2.0.0");
    assert!(acknowledged < saved && saved < exited && exited < started);

    assert_eq!(fixture.installed_version(), "2.0.0");
    assert!(!fixture.has_diagnostic());
    assert!(
        !fixture.events().iter().any(|line| line == "start 1.0.0"),
        "a confirmed update does not relaunch the old version"
    );
}

fn failed_startup_restores_and_relaunches_the_previous_version() {
    let fixture = Fixture::new("confirm");
    let staged = fixture.stage("2.0.0", "crash");

    fixture.run_host(&staged, 20_000, &[]);
    fixture.wait_for("start 2.0.0");
    fixture.wait_for("start 1.0.0");
    // The restored application surfaces what happened on its next start.
    fixture.wait_for("diagnostic rolled-back 2.0.0 HealthConfirmation");
    fixture.wait_until("the helper cleaned up", || fixture.leftovers().is_empty());

    assert!(fixture.position("crash 2.0.0") < fixture.position("start 1.0.0"));
    assert_eq!(fixture.installed_version(), "1.0.0");
    assert!(
        !fixture.has_diagnostic(),
        "taking the diagnostic removes it, so it is reported once"
    );
}

fn unconfirmed_startup_keeps_the_running_version_and_reports_it() {
    let fixture = Fixture::new("confirm");
    let staged = fixture.stage("2.0.0", "hang");

    fixture.run_host(&staged, 300, &[]);
    fixture.wait_for("start 2.0.0");
    fixture.wait_until("the helper reported", || fixture.has_diagnostic());

    assert_eq!(fixture.installed_version(), "2.0.0");
    let backups: Vec<String> = fixture
        .leftovers()
        .into_iter()
        .filter(|name| name.contains("gpui-auto-update-backup-"))
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "the previous version is kept for recovery"
    );

    let diagnostic = take_diagnostic(&fixture.prefix)
        .unwrap()
        .expect("a diagnostic");
    assert_eq!(diagnostic.outcome(), HelperOutcome::Unconfirmed);
    assert_eq!(diagnostic.version(), Some("2.0.0"));
    let error = UpdateError::from(diagnostic);
    assert_eq!(error.kind(), ErrorKind::HealthConfirmation);
    assert!(take_diagnostic(&fixture.prefix).unwrap().is_none());
}

fn replacement_failure_after_quit_is_reported_by_the_next_start() {
    let fixture = Fixture::new("confirm");
    let staged = fixture.stage("2.0.0", "confirm");

    // The host removes the staged release after the helper acknowledged it,
    // so the swap fails once the host has quit.
    fixture.run_host(&staged, 20_000, &["--remove-staged"]);
    fixture.wait_for("start 1.0.0");
    fixture.wait_for("diagnostic not-installed 2.0.0 Replacement");

    assert_eq!(fixture.installed_version(), "1.0.0");
    assert!(!fixture.events().iter().any(|line| line == "start 2.0.0"));
}

fn linux_updater_stages_a_signed_release_and_hands_it_to_the_helper() {
    let fixture = Fixture::new("confirm");
    let key = SigningKey::from_bytes(&[7; 32]);
    let feed_url = serve_release(&key, "2.0.0");

    let status = Command::new(fixture.prefix.join("bin").join(APP))
        .args(["--fake-updater", &feed_url])
        .arg(B64.encode(key.verifying_key().to_bytes()))
        .arg(fixture.parent.parent().unwrap().parent().unwrap())
        .env(LOG_ENV, &fixture.log)
        .status()
        .unwrap();
    assert!(status.success(), "{}", fixture.events().join("\n"));

    // The released executable is a shell script that confirms its start by
    // creating the health file it is given, as the protocol specifies.
    fixture.wait_for("start 2.0.0");
    fixture.wait_until("the helper cleaned up", || fixture.leftovers().is_empty());
    assert!(fixture.position("host handed off") < fixture.position("start 2.0.0"));
    assert_eq!(fixture.installed_version(), "2.0.0");
    assert!(!fixture.has_diagnostic());
}

/// Serves a signed feed at `/feed` offering `version` of a release whose
/// executable is a shell script, and returns the feed URL.
fn serve_release(key: &SigningKey, version: &str) -> String {
    let arch = Arch::current()
        .expect("a supported test architecture")
        .as_str();
    let root = format!("{APP}-{version}-linux-{arch}");
    let script =
        "#!/bin/sh\necho \"start 2.0.0\" >> \"$FAKE_LOG\"\n: > \"$GPUI_AUTO_UPDATE_HEALTH_FILE\"\n";
    let mut tar = tar::Builder::new(Vec::new());
    let mut add = |path: String, data: &[u8], mode: u32, kind: tar::EntryType| {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_size(data.len() as u64);
        header.set_cksum();
        tar.append_data(&mut header, path, data).unwrap();
    };
    add(root.clone(), b"", 0o755, tar::EntryType::Directory);
    add(
        format!("{root}/bin/{APP}"),
        script.as_bytes(),
        0o755,
        tar::EntryType::Regular,
    );
    add(
        format!("{root}/share/{APP}/gpui-auto-update.managed"),
        marker_contents(APP).as_bytes(),
        0o644,
        tar::EntryType::Regular,
    );
    add(
        format!("{root}/share/{APP}/fake.conf"),
        format!("version={version}\nmode=script\n").as_bytes(),
        0o644,
        tar::EntryType::Regular,
    );
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&tar.into_inner().unwrap()).unwrap();
    let archive = gz.finish().unwrap();

    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let feed = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0"
     xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"
     xmlns:gpui-auto-update="https://github.com/thedavidweng/gpui-auto-update/xml-namespaces/feed">
  <channel>
    <item>
      <sparkle:version>{version}</sparkle:version>
      <enclosure url="{base}/artifact" length="{length}" type="application/gzip"
                 sparkle:os="linux" gpui-auto-update:arch="{arch}"
                 sparkle:edSignature="{signature}"/>
    </item>
  </channel>
</rss>
"#,
        length = archive.len(),
        signature = B64.encode(key.sign(&archive).to_bytes()),
    );
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let body = if request.url() == "/feed" {
                feed.clone().into_bytes()
            } else {
                archive.clone()
            };
            let response = tiny_http::Response::from_data(body).with_chunked_threshold(usize::MAX);
            let _ = request.respond(response);
        }
    });
    format!("{base}/feed")
}

fn startup_confirmation_without_a_helper_is_not_requested() {
    // This process was not launched by a helper.
    assert_eq!(
        confirm_startup().unwrap(),
        StartupConfirmation::NotRequested
    );
}

// ---------------------------------------------------------------------------
// The fake application

mod fake_app {
    use super::*;

    /// `(version, mode)` from `<prefix>/share/demo/fake.conf`.
    pub fn config(prefix: &Path) -> (String, String) {
        let text = fs::read_to_string(prefix.join("share").join(APP).join("fake.conf")).unwrap();
        let value = |key: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(&format!("{key}=")))
                .unwrap()
                .to_owned()
        };
        (value("version"), value("mode"))
    }

    fn log(event: &str) {
        let path = std::env::var_os(LOG_ENV).expect("FAKE_LOG is set");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(format!("{event}\n").as_bytes()).unwrap();
    }

    pub fn main(exe: &Path) -> ExitCode {
        let prefix = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("--fake-host") {
            return host(&prefix, &args[1..]);
        }
        if args.first().map(String::as_str) == Some("--fake-updater") {
            return updater_host(exe, &args[1..]);
        }

        let (version, mode) = config(&prefix);
        log(&format!("start {version}"));
        if let Some(diagnostic) = take_diagnostic(&prefix).unwrap() {
            let outcome = match diagnostic.outcome() {
                HelperOutcome::RolledBack => "rolled-back",
                HelperOutcome::RollbackFailed => "rollback-failed",
                HelperOutcome::NotInstalled => "not-installed",
                HelperOutcome::Unconfirmed => "unconfirmed",
                _ => "other",
            };
            let shown = diagnostic.version().unwrap_or("?").to_owned();
            let kind = UpdateError::from(diagnostic).kind();
            log(&format!("diagnostic {outcome} {shown} {kind:?}"));
        }
        match mode.as_str() {
            "confirm" => {
                // The main window would have opened here.
                let confirmation = match confirm_startup().unwrap() {
                    StartupConfirmation::Confirmed => "confirmed",
                    StartupConfirmation::NotRequested => "not-requested",
                    _ => "other",
                };
                log(&format!("confirm {version} {confirmation}"));
                ExitCode::SUCCESS
            }
            "crash" => {
                log(&format!("crash {version}"));
                ExitCode::from(3)
            }
            "hang" => {
                std::thread::sleep(Duration::from_secs(3));
                ExitCode::SUCCESS
            }
            other => panic!("unknown mode {other}"),
        }
    }

    /// Checks, stages, and hands off through [`LinuxUpdater`], then quits.
    fn updater_host(exe: &Path, args: &[String]) -> ExitCode {
        let (feed_url, key, home) = (&args[0], &args[1], PathBuf::from(&args[2]));
        let detection = detect(&DetectionInputs {
            app_name: APP.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            euid: fs::metadata(exe).unwrap().uid(),
            executable: exe.to_path_buf(),
            home: Some(home),
            root: PathBuf::from("/"),
        });
        assert_eq!(
            detection.capability(),
            &Capability::SelfManaged,
            "{detection:?}"
        );

        let client = HttpClient::new(FetchPolicy {
            allow_insecure_http: true,
            timeout: Duration::from_secs(10),
            ..FetchPolicy::default()
        });
        let target = UpdateTarget::new(Os::Linux, Arch::current().unwrap());
        let checker = UpdateChecker::new(feed_url.parse().unwrap(), target, client.clone());
        let source = Arc::new(FeedCheckSource::new(
            checker,
            ReleaseVersion::parse("1.0.0").unwrap(),
        ));
        let downloader = ArtifactDownloader::new(client, TrustedKey::from_base64(key).unwrap());
        let updater = LinuxUpdater::new(detection, source.clone(), downloader);
        let coordinator = UpdateCoordinator::new(source, updater.capability());

        let outcome = coordinator.check(CheckKind::Manual).unwrap();
        assert!(
            matches!(outcome, CheckOutcome::UpdateAvailable(_)),
            "{outcome:?}"
        );
        updater.stage(&coordinator).unwrap();
        coordinator.apply(UpdateEvent::Staged).unwrap();
        updater.hand_off().unwrap();
        log("host handed off");
        ExitCode::SUCCESS
    }

    /// Hands off to the helper the way the Linux backend does, then saves
    /// and quits.
    fn host(prefix: &Path, args: &[String]) -> ExitCode {
        let staged = PathBuf::from(&args[0]);
        let health_timeout = Duration::from_millis(args[1].parse().unwrap());
        let remove_staged = args.iter().any(|arg| arg == "--remove-staged");

        let command = HelperCommand::current()
            .unwrap()
            .with_health_timeout(health_timeout);
        let request = HandoffRequest::new(APP, prefix, &staged).with_version("2.0.0");
        let pending = match command.hand_off(&request) {
            Ok(pending) => pending,
            Err(error) => {
                log(&format!("host refused: {error}"));
                return ExitCode::FAILURE;
            }
        };
        log("host acknowledged");
        if remove_staged {
            fs::remove_dir_all(staged.parent().unwrap()).unwrap();
        }
        // Saving takes a while; the install must not change meanwhile.
        std::thread::sleep(Duration::from_millis(300));
        log(&format!("host saved with {} installed", config(prefix).0));
        pending.commit();
        log("host exiting");
        ExitCode::SUCCESS
    }
}
