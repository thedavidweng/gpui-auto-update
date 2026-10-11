//! Persistence of the automatic-update preference.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{ErrorKind, UpdateError};

/// The persisted automatic-update settings of one installation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct UpdatePreferences {
    /// Whether background checks run.
    pub automatic_checks: bool,
    /// When the most recent check finished, if one ever has.
    pub last_check: Option<SystemTime>,
}

impl UpdatePreferences {
    /// Creates preferences with no recorded check.
    pub fn new(automatic_checks: bool) -> Self {
        Self {
            automatic_checks,
            last_check: None,
        }
    }

    /// Replaces the time of the most recent check.
    pub fn with_last_check(mut self, last_check: Option<SystemTime>) -> Self {
        self.last_check = last_check;
        self
    }
}

/// Who persists the automatic-update preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreferenceOwner {
    /// This library persists the preference and schedules automatic checks.
    Library,
    /// The platform backend persists the preference and runs its own
    /// periodic schedule (Sparkle on macOS). The store then only mirrors the
    /// backend's settings, which may change outside this library.
    Backend,
}

/// Loads and saves [`UpdatePreferences`].
///
/// Implemented by [`FilePreferenceStore`] on Windows and Linux, by
/// [`MemoryPreferenceStore`] for tests and previews, and by backends that own
/// the preference themselves.
pub trait PreferenceStore: Send + Sync + 'static {
    /// The stored preferences, or `None` when nothing has been stored yet.
    ///
    /// Unreadable or corrupt data is an [`ErrorKind::Preferences`] error;
    /// callers fall back to defaults and the next save replaces it.
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError>;

    /// Replaces the stored preferences.
    fn save(&self, preferences: &UpdatePreferences) -> Result<(), UpdateError>;

    /// Who persists the preference. Defaults to [`PreferenceOwner::Library`].
    fn owner(&self) -> PreferenceOwner {
        PreferenceOwner::Library
    }
}

impl<T: PreferenceStore + ?Sized> PreferenceStore for Box<T> {
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError> {
        (**self).load()
    }
    fn save(&self, preferences: &UpdatePreferences) -> Result<(), UpdateError> {
        (**self).save(preferences)
    }
    fn owner(&self) -> PreferenceOwner {
        (**self).owner()
    }
}

impl<T: PreferenceStore + ?Sized> PreferenceStore for Arc<T> {
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError> {
        (**self).load()
    }
    fn save(&self, preferences: &UpdatePreferences) -> Result<(), UpdateError> {
        (**self).save(preferences)
    }
    fn owner(&self) -> PreferenceOwner {
        (**self).owner()
    }
}

/// Stores preferences as a small JSON file.
///
/// Saves write a temporary file in the same directory, flush it to disk, and
/// rename it over the target, so a crash or power loss leaves either the old
/// or the new preferences, never a partial file. Missing parent directories
/// are created.
#[derive(Clone, Debug)]
pub struct FilePreferenceStore {
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct PreferenceFile {
    automatic_checks: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_check_unix_ms: Option<u64>,
}

impl FilePreferenceStore {
    /// Creates a store backed by the file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file this store reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn error(&self, action: &str, cause: impl std::fmt::Display) -> UpdateError {
        UpdateError::new(ErrorKind::Preferences).with_diagnostic(format!(
            "could not {action} update preferences at {}: {cause}",
            self.path.display()
        ))
    }

    fn write_atomically(&self, contents: &[u8]) -> io::Result<()> {
        let parent = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::Builder::new()
            .prefix(".gpui-auto-update-preferences.")
            .suffix(".tmp")
            .tempfile_in(parent)?;
        temp.write_all(contents)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path).map_err(|error| error.error)?;
        sync_directory(parent);
        Ok(())
    }
}

/// Makes the rename durable where the platform supports syncing a directory.
fn sync_directory(dir: &Path) {
    #[cfg(unix)]
    if let Err(error) = fs::File::open(dir).and_then(|dir| dir.sync_all()) {
        tracing::debug!(%error, "could not sync preference directory");
    }
    #[cfg(not(unix))]
    let _ = dir;
}

impl PreferenceStore for FilePreferenceStore {
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(self.error("read", &error).with_source(error)),
        };
        let file: PreferenceFile = serde_json::from_slice(&bytes)
            .map_err(|error| self.error("parse", &error).with_source(error))?;
        let last_check = file
            .last_check_unix_ms
            .and_then(|ms| SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(ms)));
        Ok(Some(
            UpdatePreferences::new(file.automatic_checks).with_last_check(last_check),
        ))
    }

    fn save(&self, preferences: &UpdatePreferences) -> Result<(), UpdateError> {
        let file = PreferenceFile {
            automatic_checks: preferences.automatic_checks,
            last_check_unix_ms: preferences.last_check.and_then(|time| {
                let ms = time
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .ok()?
                    .as_millis();
                u64::try_from(ms).ok()
            }),
        };
        let contents = serde_json::to_vec_pretty(&file)
            .map_err(|error| self.error("serialize", &error).with_source(error))?;
        self.write_atomically(&contents)
            .map_err(|error| self.error("write", &error).with_source(error))
    }
}

/// Keeps preferences in memory, shared between clones.
///
/// Useful for tests, previews, and applications that persist settings
/// themselves.
#[derive(Clone, Debug)]
pub struct MemoryPreferenceStore {
    preferences: Arc<Mutex<Option<UpdatePreferences>>>,
    owner: PreferenceOwner,
}

impl MemoryPreferenceStore {
    /// Creates an empty store owned by the library.
    pub fn new() -> Self {
        Self {
            preferences: Arc::new(Mutex::new(None)),
            owner: PreferenceOwner::Library,
        }
    }

    /// Declares who owns the preferences held by this store.
    pub fn with_owner(mut self, owner: PreferenceOwner) -> Self {
        self.owner = owner;
        self
    }
}

impl Default for MemoryPreferenceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PreferenceStore for MemoryPreferenceStore {
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError> {
        Ok(self
            .preferences
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }

    fn save(&self, preferences: &UpdatePreferences) -> Result<(), UpdateError> {
        *self
            .preferences
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(preferences.clone());
        Ok(())
    }

    fn owner(&self) -> PreferenceOwner {
        self.owner
    }
}
