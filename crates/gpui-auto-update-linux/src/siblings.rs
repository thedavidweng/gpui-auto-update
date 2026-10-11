//! Names of the hidden directories and files kept next to a managed
//! install, in its parent directory.
//!
//! Everything the updater keeps outside the prefix is a sibling named
//! `.<prefix-name>.gpui-auto-update-<role>…`, on the same filesystem as the
//! prefix so that swaps are renames, and outside the prefix so that it
//! survives the prefix being replaced.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub(crate) const STAGED: &str = "staged";
pub(crate) const BACKUP: &str = "backup";
pub(crate) const FAILED: &str = "failed";
pub(crate) const HEALTH: &str = "health";
pub(crate) const DOWNLOADS: &str = "downloads";
pub(crate) const DIAGNOSTIC: &str = "diagnostic";

/// The parent directory and file name of `prefix`.
pub(crate) fn split(prefix: &Path) -> io::Result<(&Path, &std::ffi::OsStr)> {
    match (prefix.parent(), prefix.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => Ok((parent, name)),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the installation prefix has no parent directory",
        )),
    }
}

/// `.<prefix-name>.gpui-auto-update-<role>`, the common start of every
/// sibling with that role.
pub(crate) fn role_prefix(prefix_name: &std::ffi::OsStr, role: &str) -> OsString {
    let mut name = OsString::from(".");
    name.push(prefix_name);
    name.push(format!(".gpui-auto-update-{role}"));
    name
}

/// The single sibling with `role`, such as the diagnostic file.
pub(crate) fn fixed(prefix: &Path, role: &str) -> io::Result<PathBuf> {
    let (parent, name) = split(prefix)?;
    Ok(parent.join(role_prefix(name, role)))
}

/// A sibling path with `role` that does not exist yet:
/// `.<prefix-name>.gpui-auto-update-<role>-<pid>-<nanos>-<n>`.
pub(crate) fn unique(prefix: &Path, role: &str) -> io::Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let (parent, name) = split(prefix)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    for _ in 0..64 {
        let mut candidate = role_prefix(name, role);
        candidate.push(format!(
            "-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let path = parent.join(candidate);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not find an unused name next to the installation",
    ))
}

/// Creates a new, empty, private sibling directory with `role` and returns
/// its path. Renaming a directory onto it replaces it atomically, so the
/// name stays reserved until then.
pub(crate) fn reserve_dir(prefix: &Path, role: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt as _;
    for _ in 0..8 {
        let path = unique(prefix, role)?;
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a directory next to the installation",
    ))
}

/// Flushes the directory entry changes made in `directory`.
pub(crate) fn sync_dir(directory: &Path) -> io::Result<()> {
    fs::File::open(directory)?.sync_all()
}
