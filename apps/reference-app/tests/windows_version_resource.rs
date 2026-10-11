//! The Windows executable carries the release version the backend checks
//! before a portable update is installed.
#![cfg(windows)]

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use gpui_auto_update::core::feed::Arch;
use gpui_auto_update::core::version::ReleaseVersion;
use gpui_auto_update::windows::{DEFAULT_VERSION_KEY, confirm_embedded_version, pe};

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_reference-app"))
}

#[test]
fn the_executable_declares_the_build_version() {
    let version = option_env!("REFERENCE_APP_VERSION")
        .filter(|version| !version.trim().is_empty())
        .unwrap_or("1.0.0");
    confirm_embedded_version(
        exe(),
        DEFAULT_VERSION_KEY,
        &ReleaseVersion::parse(version.trim()).unwrap(),
    )
    .unwrap();
}

#[test]
fn the_executable_is_built_for_this_architecture() {
    let info = pe::read_version_info(BufReader::new(File::open(exe()).unwrap())).unwrap();
    assert_eq!(info.machine().arch(), Arch::current());
}
