//! Bundled smoke tests against the real Sparkle framework. Sparkle only
//! starts inside an application bundle, so the parent copies this test
//! executable into temporary `.app` bundles (each with an `Info.plist`
//! pointing at an appcast the parent serves over loopback HTTP) and
//! re-launches itself as the bundle's executable, once per scenario:
//!
//! - `runtime`: the embedded framework loads, the Sparkle classes and
//!   protocols the backend relies on exist, Sparkle's user driver
//!   implements every required `SPUUserDriver` method of the embedded
//!   headers, and the delegate bridge implements the delegate-protocol
//!   methods the backend's lifecycle depends on.
//! - `background`: a scheduled-style check finds the update and Sparkle
//!   leaves the presentation to the application (no Sparkle window).
//! - `manual`: a manual check with the same presentation policy is
//!   presented by Sparkle's standard UI, without consulting the policy.
//! - `preferences-write` then `preferences-read`: the automatic-check
//!   preference is stored by Sparkle and survives a relaunch.
//!
//! Needs the `sparkle` feature and an extracted Sparkle distribution;
//! see `tests/sparkle_framework.rs` for how to run it.

#![cfg(target_os = "macos")]

#[path = "support/objc_header.rs"]
mod objc_header;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{env, fs, process, thread};

const CHILD_ENV: &str = "SPARKLE_BUNDLE_TEST_CHILD";
const FRAMEWORK_ENV: &str = "SPARKLE_BUNDLE_TEST_FRAMEWORK";
/// Base64 of 32 zero bytes: a structurally valid (throwaway) EdDSA public
/// key. The enclosure is never downloaded, so the signature is never
/// verified.
const PUBLIC_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
/// Base64 of 64 zero bytes, matching `PUBLIC_KEY`'s format.
const SIGNATURE: &str =
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==";
const PAYLOAD: &[u8] = b"gpui-auto-update bundle test placeholder archive";

fn main() {
    match env::var(CHILD_ENV) {
        Ok(scenario) => child::run(&scenario),
        Err(_) => parent::run(),
    }
}

/// The bundled child: starts Sparkle, runs one scenario, and prints
/// markers the parent asserts on. Every line it prints starts with
/// `child:`; a scenario passes by exiting with status 0.
mod child {
    use super::*;

    use std::sync::mpsc::{Receiver, Sender, channel};

    use gpui_auto_update_core::{
        Capability, CheckKind, CheckOutcome, PreferenceStore, UpdateCoordinator, UpdatePreferences,
    };
    use gpui_auto_update_macos::{
        GpuiPresentation, PresentationPolicy, SessionState, SparkleBackend, SparkleUpdate,
    };
    use objc2::runtime::{AnyClass, AnyProtocol, Sel};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use sparkle_updater::MainThreadMarker;

    /// The default presentation decisions, recorded.
    struct RecordingPolicy {
        calls: Sender<String>,
    }

    impl PresentationPolicy for RecordingPolicy {
        fn should_show_scheduled_update(&self, update: &SparkleUpdate, focus: bool) -> bool {
            let show = GpuiPresentation.should_show_scheduled_update(update, focus);
            let _ = self.calls.send(format!("should_show show={show}"));
            show
        }

        fn will_show_update(&self, handled_by_sparkle: bool, u: &SparkleUpdate, s: SessionState) {
            let _ = self.calls.send(format!(
                "will_show handled_by_sparkle={handled_by_sparkle} user_initiated={}",
                s.user_initiated
            ));
            GpuiPresentation.will_show_update(handled_by_sparkle, u, s);
        }
    }

    fn fail(message: impl std::fmt::Display) -> ! {
        eprintln!("child: FAILED: {message}");
        process::exit(1);
    }

    pub fn run(scenario: &str) -> ! {
        let mtm = MainThreadMarker::new().expect("the child runs on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        // Sparkle must start on the main thread; the scenario then runs on
        // a worker while the main thread runs the event loop.
        let (calls, recorded) = channel();
        let backend = SparkleBackend::start_with_policy(Arc::new(RecordingPolicy { calls }))
            .unwrap_or_else(|error| {
                fail(format!(
                    "start: {error} ({})",
                    error.diagnostic().unwrap_or("no diagnostic")
                ))
            });
        if backend.capability() != Capability::SelfManaged {
            fail(format!(
                "expected a self-managed bundle, got {:?}",
                backend.capability()
            ));
        }
        let scenario = scenario.to_owned();
        let worker = thread::spawn(move || {
            match scenario.as_str() {
                "runtime" => runtime(),
                "background" => background(&backend, &recorded),
                "manual" => manual(&backend, &recorded),
                "preferences-write" => preferences_write(&backend),
                "preferences-read" => preferences_read(&backend),
                other => fail(format!("unknown scenario {other:?}")),
            }
            eprintln!("child: ok");
            process::exit(0);
        });
        // If the run loop stalls the worker hangs; the parent kills us.
        drop(worker);
        app.run();
        process::exit(0);
    }

    fn check(backend: &SparkleBackend, kind: CheckKind) {
        let coordinator = UpdateCoordinator::new(backend.clone(), backend.capability());
        backend.attach(&coordinator);
        match coordinator.check(kind) {
            Ok(CheckOutcome::UpdateAvailable(update)) => {
                eprintln!("child: available version={}", update.version);
            }
            other => fail(format!("expected an available update, got {other:?}")),
        }
    }

    /// The presentation calls Sparkle makes until it decides who presents
    /// the update.
    fn presentation(recorded: &Receiver<String>) -> Vec<String> {
        let mut calls = Vec::new();
        // The presentation callbacks arrive after the check resolves.
        while let Ok(call) = recorded.recv_timeout(Duration::from_secs(30)) {
            eprintln!("child: {call}");
            let decided = call.starts_with("will_show");
            calls.push(call);
            if decided {
                return calls;
            }
        }
        fail(format!("Sparkle never decided who presents: {calls:?}"));
    }

    fn background(backend: &SparkleBackend, recorded: &Receiver<String>) {
        check(backend, CheckKind::Background);
        let calls = presentation(recorded);
        if calls.last().map(String::as_str)
            != Some("will_show handled_by_sparkle=false user_initiated=false")
        {
            fail("Sparkle did not leave the scheduled presentation to the app");
        }
    }

    fn manual(backend: &SparkleBackend, recorded: &Receiver<String>) {
        check(backend, CheckKind::Manual);
        let calls = presentation(recorded);
        if calls.iter().any(|call| call.starts_with("should_show")) {
            fail("a manual check consulted the scheduled-update policy");
        }
        if calls.last().map(String::as_str)
            != Some("will_show handled_by_sparkle=true user_initiated=true")
        {
            fail("Sparkle's standard UI did not present the manual check");
        }
    }

    fn load(backend: &SparkleBackend) -> bool {
        match backend.preferences().load() {
            Ok(Some(preferences)) => preferences.automatic_checks,
            other => fail(format!("could not load the preferences: {other:?}")),
        }
    }

    fn save(backend: &SparkleBackend, automatic: bool) {
        if let Err(error) = backend
            .preferences()
            .save(&UpdatePreferences::new(automatic))
        {
            fail(format!("could not save the preferences: {error}"));
        }
    }

    /// The bundle's Info.plist enables automatic checks; the stored
    /// preference overrides it.
    fn preferences_write(backend: &SparkleBackend) {
        if !load(backend) {
            fail("Info.plist enables automatic checks, but Sparkle reports them off");
        }
        save(backend, false);
        if load(backend) {
            fail("disabling automatic checks did not stick");
        }
        save(backend, true);
        save(backend, false);
        eprintln!("child: automatic_checks={}", load(backend));
    }

    fn preferences_read(backend: &SparkleBackend) {
        let automatic = load(backend);
        eprintln!("child: automatic_checks={automatic}");
        if automatic {
            fail("the disabled preference did not survive the relaunch");
        }
    }

    fn class(name: &str) -> &'static AnyClass {
        let c_name = std::ffi::CString::new(name).unwrap();
        AnyClass::get(&c_name).unwrap_or_else(|| fail(format!("class {name} is not loaded")))
    }

    fn protocol(name: &str) -> &'static AnyProtocol {
        let c_name = std::ffi::CString::new(name).unwrap();
        AnyProtocol::get(&c_name)
            .unwrap_or_else(|| fail(format!("protocol {name} is not registered")))
    }

    fn header_protocol(
        framework: &Path,
        header: &str,
        name: &str,
    ) -> objc_header::ProtocolSelectors {
        let path = framework.join("Sparkle.framework/Headers").join(header);
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| fail(format!("{}: {error}", path.display())));
        objc_header::protocol_selectors(&source, name)
            .unwrap_or_else(|| fail(format!("{header} does not declare @protocol {name}")))
    }

    fn responds(class: &AnyClass, selector: &str) -> bool {
        class.responds_to(Sel::register(&std::ffi::CString::new(selector).unwrap()))
    }

    /// The delegate-protocol methods the backend's lifecycle and
    /// presentation depend on, by protocol and header.
    const BRIDGED: &[(&str, &str, &[&str])] = &[
        (
            "SPUUpdaterDelegate.h",
            "SPUUpdaterDelegate",
            &[
                "updater:didFindValidUpdate:",
                "updaterDidNotFindUpdate:error:",
                "updater:willDownloadUpdate:withRequest:",
                "updater:didDownloadUpdate:",
                "updater:willExtractUpdate:",
                "updater:didExtractUpdate:",
                "updater:willInstallUpdate:",
                "updater:didAbortWithError:",
                "updater:didFinishUpdateCycleForUpdateCheck:error:",
                "updater:failedToDownloadUpdate:error:",
                "updater:userDidMakeChoice:forUpdate:state:",
                "updater:willInstallUpdateOnQuit:immediateInstallationBlock:",
                "updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:",
                "updaterWillRelaunchApplication:",
                "allowedChannelsForUpdater:",
            ],
        ),
        (
            "SPUStandardUserDriverDelegate.h",
            "SPUStandardUserDriverDelegate",
            &[
                "supportsGentleScheduledUpdateReminders",
                "standardUserDriverShouldHandleShowingScheduledUpdate:andInImmediateFocus:",
                "standardUserDriverWillHandleShowingUpdate:forUpdate:state:",
                "standardUserDriverDidReceiveUserAttentionForUpdate:",
                "standardUserDriverWillFinishUpdateSession",
            ],
        ),
    ];

    fn runtime() {
        let framework = PathBuf::from(
            env::var_os(FRAMEWORK_ENV).unwrap_or_else(|| fail(format!("{FRAMEWORK_ENV} unset"))),
        );
        for name in [
            "SPUStandardUpdaterController",
            "SPUUpdater",
            "SPUUpdaterSettings",
            "SPUStandardUserDriver",
            "SUAppcastItem",
            "SPUUserUpdateState",
        ] {
            class(name);
        }
        eprintln!("child: classes loaded");

        // Sparkle presents through its standard user driver; it must
        // implement every method the embedded protocol requires.
        let user_driver = header_protocol(&framework, "SPUUserDriver.h", "SPUUserDriver");
        if user_driver.required.is_empty() {
            fail("SPUUserDriver.h declares no required methods");
        }
        let driver = class("SPUStandardUserDriver");
        if !driver.conforms_to(protocol("SPUUserDriver")) {
            fail("SPUStandardUserDriver does not conform to SPUUserDriver");
        }
        for selector in &user_driver.required {
            if !responds(driver, selector) {
                fail(format!("SPUStandardUserDriver lacks required -{selector}"));
            }
        }
        eprintln!(
            "child: user driver implements {} required methods",
            user_driver.required.len()
        );

        // The binding's delegate class is registered once Sparkle starts.
        let delegate = class("RustSparkleUpdaterDelegate");
        for (header, name, selectors) in BRIDGED {
            protocol(name);
            let declared = header_protocol(&framework, header, name);
            for selector in *selectors {
                if !declared.declares(selector) {
                    fail(format!("{name} no longer declares -{selector}"));
                }
                if !responds(delegate, selector) {
                    fail(format!(
                        "the delegate bridge does not implement -{selector}"
                    ));
                }
            }
        }
        eprintln!("child: delegate bridge implements the bridged methods");
    }
}

/// The parent: builds the bundles, serves the appcast, and drives the
/// children.
mod parent {
    use super::*;

    pub fn run() {
        objc_header::self_test();

        let framework = framework_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the appcast server");
        let port = listener.local_addr().unwrap().port();
        let directory = env::temp_dir().join(format!("sparkle-bundle-test-{}", process::id()));

        let served = Arc::new(AtomicBool::new(false));
        let server = {
            let served = served.clone();
            thread::spawn(move || serve_appcast(listener, port, served))
        };

        let mut failures = Vec::new();
        let mut bundle_ids = Vec::new();
        let scenarios: &[(&str, &str, &[&str])] = &[
            ("runtime", "runtime", &["user driver implements"]),
            (
                "background",
                "background",
                &[
                    "child: available version=9.9.9",
                    "child: should_show show=false",
                    "child: will_show handled_by_sparkle=false user_initiated=false",
                ],
            ),
            (
                "manual",
                "manual",
                &[
                    "child: available version=9.9.9",
                    "child: will_show handled_by_sparkle=true user_initiated=true",
                ],
            ),
            // Both launches share one bundle (and so one defaults domain).
            (
                "preferences",
                "preferences-write",
                &["child: automatic_checks=false"],
            ),
            (
                "preferences",
                "preferences-read",
                &["child: automatic_checks=false"],
            ),
        ];
        for (bundle_name, scenario, markers) in scenarios {
            let bundle = directory.join(format!("{bundle_name}.app"));
            let bundle_id = format!(
                "dev.gpui-auto-update.sparkle-bundle-test.{bundle_name}-{}",
                process::id()
            );
            if !bundle.exists() {
                bundle_ids.push(bundle_id.clone());
            }
            // Sparkle's own scheduler would start a background session at
            // launch and race the manual check.
            let scheduled = *bundle_name != "manual";
            let executable = prepare_bundle(&bundle, &bundle_id, port, scheduled);
            let errors = run_child(&executable, scenario, &framework);
            eprintln!("--- {scenario} ---\n{errors}");
            let missing: Vec<&&str> = markers
                .iter()
                .filter(|marker| !errors.contains(**marker))
                .collect();
            if !errors.contains("child: ok") || !missing.is_empty() {
                failures.push(format!("{scenario} (missing {missing:?})"));
            }
        }

        served.store(true, Ordering::SeqCst);
        let _ = server.join();
        let _ = fs::remove_dir_all(&directory);
        for bundle_id in bundle_ids {
            let _ = Command::new("defaults")
                .args(["delete", &bundle_id])
                .stderr(Stdio::null())
                .status();
        }

        assert!(
            failures.is_empty(),
            "bundled Sparkle scenarios failed (output above): {failures:?}"
        );
        println!("sparkle_bundle: ok");
    }

    /// Runs one scenario and returns what it printed to stderr.
    fn run_child(executable: &Path, scenario: &str, framework: &Path) -> String {
        let mut child = Command::new(executable)
            .env(CHILD_ENV, scenario)
            .env(FRAMEWORK_ENV, framework)
            .env("DYLD_FRAMEWORK_PATH", framework)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch the bundled test");
        let mut stderr = child.stderr.take().unwrap();
        let reader = thread::spawn(move || {
            let mut errors = String::new();
            let _ = stderr.read_to_string(&mut errors);
            errors
        });
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                Err(error) => panic!("could not wait for the child: {error}"),
            }
        }
        reader.join().unwrap_or_default()
    }

    /// The directory containing Sparkle.framework, from the same variables
    /// the build and the loader use.
    fn framework_dir() -> PathBuf {
        for variable in ["SPARKLE_FRAMEWORK_PATH", "DYLD_FRAMEWORK_PATH"] {
            if let Some(directory) = env::var_os(variable).map(PathBuf::from) {
                if directory.join("Sparkle.framework").is_dir() {
                    return directory;
                }
            }
        }
        panic!(
            "Sparkle.framework not found: set SPARKLE_FRAMEWORK_PATH and DYLD_FRAMEWORK_PATH \
             (gpui-auto-update sparkle fetch --out <dir>)"
        );
    }

    /// Copies this test executable into a `.app` bundle whose Info.plist
    /// points Sparkle at the loopback appcast.
    fn prepare_bundle(bundle: &Path, bundle_id: &str, port: u16, scheduled: bool) -> PathBuf {
        let executable = bundle.join("Contents/MacOS/sparkle-bundle-test");
        fs::create_dir_all(executable.parent().unwrap()).expect("create the bundle layout");
        // Overwriting an executable that already ran in place invalidates
        // the kernel's cached code signature and the next launch is killed.
        let _ = fs::remove_file(&executable);
        fs::copy(env::current_exe().unwrap(), &executable).expect("copy the test executable");
        let plist = format!(
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8"?>"#,
                r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#,
                r#"<plist version="1.0"><dict>"#,
                "<key>CFBundleIdentifier</key><string>{}</string>",
                "<key>CFBundleName</key><string>sparkle-bundle-test</string>",
                "<key>CFBundleExecutable</key><string>sparkle-bundle-test</string>",
                "<key>CFBundlePackageType</key><string>APPL</string>",
                "<key>CFBundleShortVersionString</key><string>0.1.0</string>",
                "<key>CFBundleVersion</key><string>1</string>",
                "<key>LSMinimumSystemVersion</key><string>12.0</string>",
                "<key>SUFeedURL</key><string>http://127.0.0.1:{}/appcast.xml</string>",
                "<key>SUPublicEDKey</key><string>{}</string>",
                "<key>SUEnableAutomaticChecks</key><{}/>",
                "<key>SUScheduledCheckInterval</key><integer>900</integer>",
                "</dict></plist>",
            ),
            bundle_id, port, PUBLIC_KEY, scheduled,
        );
        fs::write(bundle.join("Contents/Info.plist"), plist).expect("write Info.plist");
        executable
    }

    fn appcast(port: u16) -> String {
        format!(
            concat!(
                r#"<?xml version="1.0" encoding="utf-8"?>"#,
                r#"<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">"#,
                "<channel><title>sparkle-bundle-test</title><item>",
                "<title>Version 9.9.9</title>",
                "<sparkle:shortVersionString>9.9.9</sparkle:shortVersionString>",
                "<sparkle:version>9999</sparkle:version>",
                "<pubDate>Mon, 05 Oct 2026 10:00:00 +0000</pubDate>",
                "<sparkle:minimumSystemVersion>12.0</sparkle:minimumSystemVersion>",
                r#"<enclosure url="http://127.0.0.1:{}/update.zip" sparkle:edSignature="{}" length="{}" type="application/octet-stream"/>"#,
                "</item></channel></rss>",
            ),
            port,
            SIGNATURE,
            PAYLOAD.len(),
        )
    }

    /// Serves the appcast (and the placeholder archive) until the children
    /// are done.
    fn serve_appcast(listener: TcpListener, port: u16, stop: Arc<AtomicBool>) {
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let mut request = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    // Read the whole request head; a single read can return
                    // a partial line.
                    loop {
                        match stream.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(read) => {
                                request.extend_from_slice(&chunk[..read]);
                                if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let request = String::from_utf8_lossy(&request);
                    let (content_type, body) = if request.starts_with("GET /appcast.xml") {
                        ("application/xml", appcast(port).into_bytes())
                    } else if request.starts_with("GET /update.zip") {
                        ("application/octet-stream", PAYLOAD.to_vec())
                    } else {
                        eprintln!(
                            "sparkle_bundle: unexpected request: {:?}",
                            request.lines().next().unwrap_or("")
                        );
                        ("text/plain", Vec::new())
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.write_all(&body);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    }
}
