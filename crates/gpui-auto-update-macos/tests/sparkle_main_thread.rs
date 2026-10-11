//! Starts the real Sparkle updater on the main thread (this test runs
//! without the libtest harness so that `main` is the process's main
//! thread). Built only with the `sparkle` feature; see
//! `tests/sparkle_framework.rs` for how to run it.
//!
//! A test executable is not an application bundle, so Sparkle must decline
//! to start and the backend must report an unsupported installation rather
//! than fail.

#[cfg(target_os = "macos")]
fn main() {
    use gpui_auto_update_core::{Capability, CheckKind, ErrorKind, UpdateCoordinator};
    use gpui_auto_update_macos::SparkleBackend;

    let backend = SparkleBackend::start().expect("Sparkle start-up outside a bundle");
    assert_eq!(backend.capability(), Capability::Unsupported);

    let coordinator = UpdateCoordinator::new(backend.clone(), backend.capability());
    let error = coordinator.check(CheckKind::Manual).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::UnsupportedInstallation);

    println!("sparkle_main_thread: ok");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
