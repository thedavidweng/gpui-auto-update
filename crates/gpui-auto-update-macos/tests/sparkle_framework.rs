//! Checks against the real Sparkle framework. They need the `sparkle`
//! feature and an extracted Sparkle distribution:
//!
//! ```sh
//! gpui-auto-update sparkle fetch --out <dir>
//! SPARKLE_FRAMEWORK_PATH=<dir> DYLD_FRAMEWORK_PATH=<dir> \
//!     cargo test -p gpui-auto-update-macos --features sparkle -- --include-ignored
//! ```
//!
//! The main-thread behavior is covered by the `sparkle_main_thread` test,
//! which runs without the test harness so that it owns the main thread.

#![cfg(all(target_os = "macos", feature = "sparkle"))]

use gpui_auto_update_core::ErrorKind;
use gpui_auto_update_macos::SparkleBackend;

#[test]
#[ignore = "needs Sparkle.framework: set SPARKLE_FRAMEWORK_PATH and DYLD_FRAMEWORK_PATH"]
fn starting_sparkle_off_the_main_thread_is_a_configuration_error() {
    // The test harness runs every test on a worker thread.
    let error = SparkleBackend::start().unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Configuration);
}
