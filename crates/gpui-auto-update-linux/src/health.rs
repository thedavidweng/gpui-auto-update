//! The startup health signal: how a relaunched application tells the
//! helper that its main window opened.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::siblings;

/// The environment variable through which the helper passes the health
/// file to the version it launches.
pub const HEALTH_FILE_ENV: &str = "GPUI_AUTO_UPDATE_HEALTH_FILE";

/// The result of [`confirm_startup`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StartupConfirmation {
    /// This process was not launched by the update helper, so there was
    /// nothing to confirm.
    NotRequested,
    /// The helper was told that this version started successfully.
    Confirmed,
}

/// Why the startup confirmation could not be delivered.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HealthError {
    /// The health file named by [`HEALTH_FILE_ENV`] is not one the helper
    /// could have reserved for this installation.
    #[error("the health file {0:?} does not belong to this installation")]
    UnexpectedPath(PathBuf),
    /// The running executable is not `<prefix>/bin/<app>`.
    #[error("the running executable is not inside a managed install")]
    NoInstall,
    /// The health file could not be created.
    #[error("could not create the health file: {0}")]
    Io(#[from] io::Error),
}

impl From<HealthError> for UpdateError {
    fn from(error: HealthError) -> Self {
        UpdateError::new(ErrorKind::HealthConfirmation)
            .with_diagnostic(error.to_string())
            .with_source(error)
    }
}

/// Tells the update helper that this freshly installed version started
/// successfully. Call it once the application's main window has opened.
///
/// After an update, the helper keeps the previous version until this
/// signal arrives and restores it if the new version exits first. When the
/// process was not launched by the helper this does nothing and returns
/// [`StartupConfirmation::NotRequested`] without touching the filesystem,
/// so it is safe to call on every start. Calling it again after a
/// confirmation also returns [`StartupConfirmation::Confirmed`].
///
/// The install is located from the running executable, which must be
/// `<prefix>/bin/<app>`. The call does a small amount of blocking file I/O.
pub fn confirm_startup() -> Result<StartupConfirmation, HealthError> {
    let Some(health) = std::env::var_os(HEALTH_FILE_ENV) else {
        return Ok(StartupConfirmation::NotRequested);
    };
    let executable = std::env::current_exe()?.canonicalize()?;
    let prefix = executable
        .parent()
        .filter(|bin| bin.file_name() == Some(OsStr::new("bin")))
        .and_then(Path::parent)
        .ok_or(HealthError::NoInstall)?;
    signal(prefix, Path::new(&health))
}

/// Like [`confirm_startup`] for the managed install at `prefix`, for
/// applications whose executable is not `<prefix>/bin/<app>` itself.
pub fn confirm_startup_for(prefix: &Path) -> Result<StartupConfirmation, HealthError> {
    match std::env::var_os(HEALTH_FILE_ENV) {
        None => Ok(StartupConfirmation::NotRequested),
        Some(health) => signal(&prefix.canonicalize()?, Path::new(&health)),
    }
}

/// Creates `health` after checking that it is a health-file name next to
/// `prefix`. It is created exclusively and never followed, so the variable
/// cannot be used to write elsewhere.
fn signal(prefix: &Path, health: &Path) -> Result<StartupConfirmation, HealthError> {
    let unexpected = || HealthError::UnexpectedPath(health.to_path_buf());
    let (parent, prefix_name) = siblings::split(prefix)?;
    let expected_start = siblings::role_prefix(prefix_name, siblings::HEALTH);
    let name = health.file_name().ok_or_else(unexpected)?;
    let health_parent = health
        .parent()
        .and_then(|dir| dir.canonicalize().ok())
        .ok_or_else(unexpected)?;
    if !health.is_absolute()
        || health_parent != parent
        || !name.as_bytes().starts_with(expected_start.as_bytes())
    {
        return Err(unexpected());
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(parent.join(name))
    {
        Ok(_) => Ok(StartupConfirmation::Confirmed),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            match fs::symlink_metadata(parent.join(name)) {
                Ok(meta) if meta.is_file() => Ok(StartupConfirmation::Confirmed),
                _ => Err(unexpected()),
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// Whether the helper's health file for this launch has been created.
pub(crate) fn is_signaled(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().canonicalize().unwrap().join("demo");
        fs::create_dir(&prefix).unwrap();
        (dir, prefix)
    }

    #[test]
    fn creates_the_reserved_health_file() {
        let (_dir, prefix) = install();
        let health = prefix
            .parent()
            .unwrap()
            .join(".demo.gpui-auto-update-health-1-2-3");
        assert_eq!(
            signal(&prefix, &health).unwrap(),
            StartupConfirmation::Confirmed
        );
        assert!(is_signaled(&health));
        assert_eq!(
            signal(&prefix, &health).unwrap(),
            StartupConfirmation::Confirmed
        );
    }

    #[test]
    fn refuses_paths_the_helper_could_not_have_reserved() {
        let (dir, prefix) = install();
        let parent = prefix.parent().unwrap().to_path_buf();
        for health in [
            parent.join("elsewhere"),
            parent.join(".other.gpui-auto-update-health-1"),
            prefix.join(".demo.gpui-auto-update-health-1"),
            PathBuf::from(".demo.gpui-auto-update-health-1"),
        ] {
            assert!(
                matches!(
                    signal(&prefix, &health),
                    Err(HealthError::UnexpectedPath(_))
                ),
                "{health:?}"
            );
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn never_follows_a_planted_symlink() {
        let (dir, prefix) = install();
        let target = dir.path().join("target");
        let health = prefix
            .parent()
            .unwrap()
            .join(".demo.gpui-auto-update-health-9");
        std::os::unix::fs::symlink(&target, &health).unwrap();
        assert!(signal(&prefix, &health).is_err());
        assert!(!target.exists());
    }
}
