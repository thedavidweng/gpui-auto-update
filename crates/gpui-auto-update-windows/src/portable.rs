//! In-place replacement of a portable application's executable.
//!
//! Windows refuses to delete or overwrite a running executable but allows
//! renaming it, and the running process keeps its mapped image. So the
//! running executable is renamed to a backup next to it and the verified
//! executable is renamed into its place. Both renames stay inside the
//! install directory (the staging root defaults to a directory in it), so
//! each is atomic and never copies data. The backup is removed on a later
//! launch, once nothing runs from it.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gpui_auto_update_core::{ErrorKind, UpdateError};

/// The name the verified executable is staged under before it is moved into
/// place.
pub(crate) const STAGED_FILE_NAME: &str = "update.exe";
/// Appended to the executable's file name to name the backup.
const BACKUP_SUFFIX: &str = ".previous";
/// The staging directory created inside a portable install directory.
pub(crate) const STAGING_DIR_NAME: &str = ".gpui-auto-update";

/// Replaces `executable` with `staged`, keeping the old executable as a
/// backup. On failure the executable is back where it was.
pub(crate) fn replace_executable(staged: &Path, executable: &Path) -> Result<(), UpdateError> {
    let backup = free_backup_path(executable);
    if let Err(error) = fs::rename(executable, &backup) {
        return Err(replacement_error(
            format!("could not move the running executable aside: {error}"),
            error,
        ));
    }
    if let Err(error) = fs::rename(staged, executable) {
        return match fs::rename(&backup, executable) {
            Ok(()) => Err(replacement_error(
                format!("could not move the new executable into place: {error}"),
                error,
            )),
            Err(restore) => Err(UpdateError::new(ErrorKind::Rollback)
                .with_diagnostic(format!(
                    "could not move the new executable into place ({error}) or restore the \
                     previous one from {} ({restore})",
                    backup.display()
                ))
                .with_source(restore)),
        };
    }
    Ok(())
}

/// Removes backups left by earlier updates. A backup still in use (an old
/// instance is running) cannot be removed and is left for a later launch.
pub(crate) fn remove_backups(executable: &Path) {
    let (Some(dir), Some(name)) = (executable.parent(), executable.file_name()) else {
        return;
    };
    let prefix = backup_name(name);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let is_backup = file_name
            .to_str()
            .zip(prefix.to_str())
            .is_some_and(|(file, prefix)| file.starts_with(prefix));
        if is_backup && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            if let Err(error) = fs::remove_file(entry.path()) {
                tracing::debug!(%error, path = %entry.path().display(), "could not remove a previous executable");
            }
        }
    }
}

/// `<exe>.previous`, or a unique variant when an older backup cannot be
/// removed because it is still running.
fn free_backup_path(executable: &Path) -> PathBuf {
    let name = executable.file_name().unwrap_or_default();
    let backup = executable.with_file_name(backup_name(name));
    if !backup.exists() || fs::remove_file(&backup).is_ok() {
        return backup;
    }
    let mut unique = backup_name(name);
    unique.push(format!(
        "-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    executable.with_file_name(unique)
}

fn backup_name(name: &std::ffi::OsStr) -> OsString {
    let mut backup = name.to_os_string();
    backup.push(BACKUP_SUFFIX);
    backup
}

fn replacement_error(diagnostic: String, source: io::Error) -> UpdateError {
    UpdateError::new(ErrorKind::Replacement)
        .with_diagnostic(diagnostic)
        .with_source(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_move_into_place_restores_the_executable() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("demo.exe");
        fs::write(&exe, b"old").unwrap();
        let missing = dir.path().join("missing.exe");

        let error = replace_executable(&missing, &exe).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Replacement);
        assert_eq!(fs::read(&exe).unwrap(), b"old");
        assert!(!dir.path().join("demo.exe.previous").exists());
    }

    #[test]
    fn an_old_backup_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("demo.exe");
        let staged = dir.path().join("update.exe");
        fs::write(&exe, b"v2").unwrap();
        fs::write(dir.path().join("demo.exe.previous"), b"v1").unwrap();
        fs::write(&staged, b"v3").unwrap();

        replace_executable(&staged, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"v3");
        assert_eq!(
            fs::read(dir.path().join("demo.exe.previous")).unwrap(),
            b"v2"
        );

        fs::write(dir.path().join("demo.exe.previous-1-2"), b"v0").unwrap();
        fs::write(dir.path().join("other.exe.previous"), b"keep").unwrap();
        remove_backups(&exe);
        let mut left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["demo.exe", "other.exe.previous"]);
    }
}
