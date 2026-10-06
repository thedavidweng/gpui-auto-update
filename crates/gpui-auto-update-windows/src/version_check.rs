//! Confirming that a verified artifact is the release its feed entry names.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use gpui_auto_update_core::version::ReleaseVersion;
use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::pe::{self, VersionInfo};

/// The version-resource string the built-in strategies compare with the
/// feed's `sparkle:version`.
pub const DEFAULT_VERSION_KEY: &str = "ProductVersion";

/// Checks that the PE image at `path` declares exactly `expected` in its
/// `StringFileInfo` entry `key`.
///
/// The Ed25519 signature proves the bytes come from the release key, but the
/// feed entry that names their version is not signed. Reading the version
/// from inside the signed bytes stops a relabeled older release from being
/// installed as a newer one. Every string table that has `key` must hold the
/// exact `expected` text; a missing key, a missing version resource, or any
/// other value is an [`ErrorKind::ArchiveValidation`] error.
pub fn confirm_embedded_version(
    path: &Path,
    key: &str,
    expected: &ReleaseVersion,
) -> Result<(), UpdateError> {
    let info = read(path)?;
    confirm_info(&info, key, expected)
}

pub(crate) fn read(path: &Path) -> Result<VersionInfo, UpdateError> {
    let file = File::open(path).map_err(|error| {
        UpdateError::new(ErrorKind::Staging)
            .with_diagnostic(format!("could not open the staged artifact: {error}"))
            .with_source(error)
    })?;
    pe::read_version_info(BufReader::new(file)).map_err(|error| {
        UpdateError::new(ErrorKind::ArchiveValidation)
            .with_message("The downloaded update is not a valid Windows program.")
            .with_diagnostic(format!(
                "could not read the artifact's version resource: {error}"
            ))
            .with_source(error)
    })
}

pub(crate) fn confirm_info(
    info: &VersionInfo,
    key: &str,
    expected: &ReleaseVersion,
) -> Result<(), UpdateError> {
    let found: Vec<&str> = info.strings(key).collect();
    let mismatch = |detail: String| {
        UpdateError::new(ErrorKind::ArchiveValidation)
            .with_message("The downloaded update does not match the release it was published as.")
            .with_diagnostic(detail)
    };
    if found.is_empty() {
        return Err(mismatch(format!(
            "the artifact's version resource has no {key:?} string; release {expected} cannot be confirmed"
        )));
    }
    if let Some(other) = found.iter().find(|value| **value != expected.as_str()) {
        return Err(mismatch(format!(
            "the artifact declares {key} {other:?} but the feed entry is version {expected}"
        )));
    }
    Ok(())
}
