//! Runs a real background check against the real Sparkle framework, from a
//! temporary application bundle. Sparkle only starts inside a bundle, so
//! the test copies its own executable into a temporary `.app` (with an
//! `Info.plist` pointing at an appcast served over loopback HTTP by the
//! parent process) and re-launches itself as the bundle's executable.
//!
//! The child asserts the T10 contract end to end: the scheduled-style
//! check finds the update, Sparkle leaves presentation to the application
//! (no Sparkle window), and the update state retains the discovery.
//!
//! Needs the `sparkle` feature and an extracted Sparkle distribution;
//! see `tests/sparkle_framework.rs` for how to run it.

#![cfg(target_os = "macos")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::{env, fs, process, thread};

const CHILD_ENV: &str = "SPARKLE_BUNDLE_TEST_CHILD";
/// Base64 of 32 zero bytes: a structurally valid (throwaway) EdDSA public
/// key. The enclosure is never downloaded, so the signature is never
/// verified.
const PUBLIC_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
/// Base64 of 64 zero bytes, matching `PUBLIC_KEY`'s format.
const SIGNATURE: &str =
    "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==";
const PAYLOAD: &[u8] = b"gpui-auto-update bundle test placeholder archive";

fn main() {
    if env::var_os(CHILD_ENV).is_some() {
        child::run();
    } else {
        parent::run();
    }
}

/// The bundled child: starts Sparkle, runs a background check, and prints
/// markers the parent asserts on.
mod child {
    use super::*;

    use gpui_auto_update_core::{Capability, CheckKind, CheckOutcome, UpdateCoordinator};
    use gpui_auto_update_macos::{PresentationPolicy, SessionState, SparkleBackend, SparkleUpdate};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use sparkle_updater::MainThreadMarker;

    /// Records how Sparkle presented the scheduled discovery.
    struct RecordingPolicy {
        shown: std::sync::mpsc::Sender<String>,
    }

    impl PresentationPolicy for RecordingPolicy {
        fn should_show_scheduled_update(&self, update: &SparkleUpdate, _: bool) -> bool {
            eprintln!("reminder: should_show version={}", update.version);
            false
        }

        fn will_show_update(&self, handled_by_sparkle: bool, _: &SparkleUpdate, s: SessionState) {
            let _ = self.shown.send(format!(
                "handled_by_sparkle={handled_by_sparkle} user_initiated={}",
                s.user_initiated
            ));
        }
    }

    pub fn run() -> ! {
        let mtm = MainThreadMarker::new().expect("the child runs on the main thread");
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        // Sparkle must start on the main thread; the blocking check then
        // runs on a worker while the main thread runs the event loop.
        let (shown_tx, shown_rx) = std::sync::mpsc::channel();
        let backend = match SparkleBackend::start_with_policy(Arc::new(RecordingPolicy {
            shown: shown_tx,
        })) {
            Ok(backend) => backend,
            Err(error) => {
                eprintln!(
                    "sparkle_bundle: start failed: {error} ({})",
                    error.diagnostic().unwrap_or("no diagnostic")
                );
                process::exit(1);
            }
        };
        if backend.capability() != Capability::SelfManaged {
            eprintln!(
                "sparkle_bundle: expected a self-managed bundle, got {:?}",
                backend.capability()
            );
            process::exit(1);
        }
        let worker = thread::spawn(move || {
            let coordinator = UpdateCoordinator::new(backend.clone(), backend.capability());
            backend.attach(&coordinator);

            match coordinator.check(CheckKind::Background) {
                Ok(CheckOutcome::UpdateAvailable(update)) => {
                    eprintln!("sparkle_bundle: available version={}", update.version);
                }
                other => {
                    eprintln!("sparkle_bundle: expected an available update, got {other:?}");
                    process::exit(1);
                }
            }
            // The presentation callbacks arrive after the check resolves.
            match shown_rx.recv_timeout(Duration::from_secs(30)) {
                Ok(presentation) => {
                    eprintln!("reminder: will_show {presentation}");
                    if presentation == "handled_by_sparkle=false user_initiated=false" {
                        eprintln!("sparkle_bundle: ok");
                        process::exit(0);
                    }
                    eprintln!("sparkle_bundle: Sparkle did not leave the presentation to us");
                    process::exit(1);
                }
                Err(_) => {
                    eprintln!("sparkle_bundle: Sparkle never reported the reminder");
                    process::exit(1);
                }
            }
        });
        // If the run loop stalls the worker hangs; the parent kills us.
        drop(worker);
        app.run();
        process::exit(0);
    }
}

/// The parent: builds the bundle, serves the appcast, and drives the
/// child.
mod parent {
    use super::*;

    pub fn run() {
        let framework = framework_dir();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the appcast server");
        let port = listener.local_addr().unwrap().port();
        let directory = env::temp_dir().join(format!("sparkle-bundle-test-{}", process::id()));
        let bundle = directory.join("SparkleBundleTest.app");
        let executable = prepare_bundle(&bundle, port);

        let served = Arc::new(AtomicBool::new(false));
        let server = {
            let served = served.clone();
            thread::spawn(move || serve_appcast(listener, port, served))
        };

        let mut child = Command::new(&executable)
            .env(CHILD_ENV, "1")
            .env("DYLD_FRAMEWORK_PATH", &framework)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("launch the bundled test");

        let deadline = Instant::now() + Duration::from_secs(120);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    assert!(
                        Instant::now() < deadline,
                        "the bundled Sparkle check did not finish in time"
                    );
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => panic!("could not wait for the child: {error}"),
            }
        };
        let mut output = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .ok();
        let mut errors = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut errors)
            .ok();
        served.store(true, Ordering::SeqCst);
        let _ = server.join();
        let _ = fs::remove_dir_all(&directory);

        eprintln!("--- child stdout ---\n{output}\n--- child stderr ---\n{errors}");
        assert!(
            status.success(),
            "the bundled Sparkle check failed (output above)"
        );
        assert!(
            errors.contains("sparkle_bundle: available version=9.9.9"),
            "the scheduled discovery did not surface: {errors}"
        );
        assert!(
            errors.contains("reminder: will_show handled_by_sparkle=false user_initiated=false"),
            "Sparkle did not leave the scheduled presentation to the app: {errors}"
        );
        println!("sparkle_bundle: ok");
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

    /// Copies this test executable into a fresh `.app` bundle whose
    /// Info.plist points Sparkle at the loopback appcast.
    fn prepare_bundle(bundle: &Path, port: u16) -> PathBuf {
        let executable = bundle.join("Contents/MacOS/sparkle-bundle-test");
        fs::create_dir_all(executable.parent().unwrap()).expect("create the bundle layout");
        fs::copy(env::current_exe().unwrap(), &executable).expect("copy the test executable");
        let plist = format!(
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8"?>"#,
                r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#,
                r#"<plist version="1.0"><dict>"#,
                "<key>CFBundleIdentifier</key><string>dev.gpui-auto-update.sparkle-bundle-test-{}</string>",
                "<key>CFBundleName</key><string>sparkle-bundle-test</string>",
                "<key>CFBundleExecutable</key><string>sparkle-bundle-test</string>",
                "<key>CFBundlePackageType</key><string>APPL</string>",
                "<key>CFBundleShortVersionString</key><string>0.1.0</string>",
                "<key>CFBundleVersion</key><string>1</string>",
                "<key>LSMinimumSystemVersion</key><string>12.0</string>",
                "<key>SUFeedURL</key><string>http://127.0.0.1:{}/appcast.xml</string>",
                "<key>SUPublicEDKey</key><string>{}</string>",
                "<key>SUEnableAutomaticChecks</key><true/>",
                "<key>SUScheduledCheckInterval</key><integer>900</integer>",
                "</dict></plist>",
            ),
            process::id(),
            port,
            PUBLIC_KEY,
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

    /// Serves the appcast (and the placeholder archive) until the child is
    /// done.
    fn serve_appcast(listener: TcpListener, port: u16, stop: Arc<AtomicBool>) {
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
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
