//! What the helper records when an update does not finish normally, so
//! that the next start of the application can report it.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use gpui_auto_update_core::{ErrorKind, UpdateError};

use crate::siblings;

const HEADER: &str = "gpui-auto-update helper-diagnostic 1";
const MAX_LEN: u64 = 16 * 1024;
const MAX_DETAIL: usize = 4096;

/// How an update attempt ended when it did not end with a confirmed new
/// version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HelperOutcome {
    /// The new version failed to start; the previous version was restored
    /// and relaunched.
    RolledBack,
    /// The new version failed and restoring the previous version also
    /// failed; the installation may need manual repair.
    RollbackFailed,
    /// The new version could not be put in place; the previous version was
    /// kept and relaunched.
    NotInstalled,
    /// The new version is installed and still running, but it did not
    /// confirm a successful start in time. The previous version is kept next
    /// to the install for manual recovery.
    Unconfirmed,
}

impl HelperOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::RolledBack => "rolled-back",
            Self::RollbackFailed => "rollback-failed",
            Self::NotInstalled => "not-installed",
            Self::Unconfirmed => "unconfirmed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "rolled-back" => Self::RolledBack,
            "rollback-failed" => Self::RollbackFailed,
            "not-installed" => Self::NotInstalled,
            "unconfirmed" => Self::Unconfirmed,
            _ => return None,
        })
    }
}

/// A failure recorded by the update helper after the application had quit.
///
/// It is stored next to the install (outside the prefix, so it survives a
/// swap or rollback) and returned once by [`take_diagnostic`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelperDiagnostic {
    outcome: HelperOutcome,
    kind: ErrorKind,
    version: Option<String>,
    detail: String,
}

impl HelperDiagnostic {
    pub(crate) fn new(
        outcome: HelperOutcome,
        kind: ErrorKind,
        version: Option<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            outcome,
            kind,
            version,
            detail: detail.into(),
        }
    }

    /// How the update attempt ended.
    pub fn outcome(&self) -> HelperOutcome {
        self.outcome
    }

    /// The failing step, as an error kind: for example
    /// [`ErrorKind::HealthConfirmation`] when the new version did not start,
    /// [`ErrorKind::Replacement`] when it could not be put in place, and
    /// [`ErrorKind::Rollback`] when restoring the previous version failed.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The version that was being installed, when known.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Developer-facing detail, such as an exit status or I/O error. It may
    /// contain paths and is not meant for end users.
    pub fn detail(&self) -> &str {
        &self.detail
    }

    fn encode(&self) -> String {
        let mut text = format!(
            "{HEADER}\noutcome={}\nkind={}\n",
            self.outcome.as_str(),
            kind_name(self.kind)
        );
        if let Some(version) = &self.version {
            text.push_str(&format!("version={}\n", single_line(version)));
        }
        text.push_str(&format!("detail={}\n", single_line(&self.detail)));
        text
    }

    fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let (mut outcome, mut kind, mut version, mut detail) = (None, None, None, None);
        for line in lines {
            let (key, value) = line.split_once('=')?;
            match key {
                "outcome" => outcome = HelperOutcome::parse(value),
                "kind" => kind = parse_kind(value),
                "version" => version = Some(value.to_owned()),
                "detail" => detail = Some(value.to_owned()),
                _ => {}
            }
        }
        Some(Self {
            outcome: outcome?,
            kind: kind?,
            version,
            detail: detail.unwrap_or_default(),
        })
    }
}

impl std::fmt::Display for HelperDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "update helper: {}", self.outcome.as_str())?;
        if let Some(version) = &self.version {
            write!(f, " (version {version})")?;
        }
        write!(f, ": {}", self.detail)
    }
}

impl From<HelperDiagnostic> for UpdateError {
    /// A user-presentable error for the recorded outcome, with the detail
    /// as its diagnostic.
    fn from(diagnostic: HelperDiagnostic) -> Self {
        let version = diagnostic
            .version
            .as_deref()
            .map_or_else(|| "The new version".to_owned(), |v| format!("Version {v}"));
        let message = match diagnostic.outcome {
            HelperOutcome::RolledBack => {
                format!("{version} did not start correctly, so the previous version was restored.")
            }
            HelperOutcome::RollbackFailed => {
                "The update failed and the previous version could not be restored automatically."
                    .to_owned()
            }
            HelperOutcome::NotInstalled => {
                format!("{version} could not be installed, so the current version was kept.")
            }
            HelperOutcome::Unconfirmed => {
                format!("{version} did not confirm that it started correctly.")
            }
        };
        UpdateError::new(diagnostic.kind)
            .with_message(message)
            .with_diagnostic(diagnostic.to_string())
    }
}

/// Reads and removes the diagnostic the update helper left for the managed
/// install at `prefix`, if any.
///
/// Call it once when the application starts (the facade does this) and
/// show the result: it is how a rollback or a failed installation is
/// reported to the user. A file that is not a small regular file with the
/// expected contents is removed and reported as an error.
pub fn take_diagnostic(prefix: &Path) -> io::Result<Option<HelperDiagnostic>> {
    let path = siblings::fixed(prefix, siblings::DIAGNOSTIC)?;
    let link_meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let contents = read_small_file(&path, &link_meta);
    fs::remove_file(&path)?;
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid helper diagnostic");
    let text = String::from_utf8(contents?).map_err(|_| invalid())?;
    HelperDiagnostic::decode(&text)
        .map(Some)
        .ok_or_else(invalid)
}

/// Atomically replaces the diagnostic for the install at `prefix`.
pub(crate) fn record(prefix: &Path, diagnostic: &HelperDiagnostic) -> io::Result<PathBuf> {
    let path = siblings::fixed(prefix, siblings::DIAGNOSTIC)?;
    let (parent, _) = siblings::split(prefix)?;
    let mut file = tempfile::Builder::new()
        .prefix(".gpui-auto-update-diagnostic-")
        .tempfile_in(parent)?;
    file.write_all(diagnostic.encode().as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(&path).map_err(|error| error.error)?;
    let _ = siblings::sync_dir(parent);
    Ok(path)
}

fn read_small_file(path: &Path, link_meta: &fs::Metadata) -> io::Result<Vec<u8>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid helper diagnostic");
    if !link_meta.is_file() || link_meta.len() > MAX_LEN {
        return Err(invalid());
    }
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    if meta.dev() != link_meta.dev() || meta.ino() != link_meta.ino() {
        return Err(invalid());
    }
    let mut contents = Vec::new();
    file.take(MAX_LEN + 1).read_to_end(&mut contents)?;
    if contents.len() as u64 > MAX_LEN {
        return Err(invalid());
    }
    Ok(contents)
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_DETAIL)
        .collect()
}

fn kind_name(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::HelperLaunch => "helper-launch",
        ErrorKind::QuitCoordination => "quit-coordination",
        ErrorKind::Replacement => "replacement",
        ErrorKind::Relaunch => "relaunch",
        ErrorKind::HealthConfirmation => "health-confirmation",
        ErrorKind::Rollback => "rollback",
        _ => "internal",
    }
}

fn parse_kind(text: &str) -> Option<ErrorKind> {
    Some(match text {
        "helper-launch" => ErrorKind::HelperLaunch,
        "quit-coordination" => ErrorKind::QuitCoordination,
        "replacement" => ErrorKind::Replacement,
        "relaunch" => ErrorKind::Relaunch,
        "health-confirmation" => ErrorKind::HealthConfirmation,
        "rollback" => ErrorKind::Rollback,
        "internal" => ErrorKind::Internal,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_round_trip_through_the_file_format() {
        let diagnostic = HelperDiagnostic::new(
            HelperOutcome::RollbackFailed,
            ErrorKind::Rollback,
            Some("2.0.0".into()),
            "rename failed:\nno space",
        );
        let decoded = HelperDiagnostic::decode(&diagnostic.encode()).unwrap();
        assert_eq!(decoded.outcome(), HelperOutcome::RollbackFailed);
        assert_eq!(decoded.kind(), ErrorKind::Rollback);
        assert_eq!(decoded.version(), Some("2.0.0"));
        assert_eq!(decoded.detail(), "rename failed: no space");
    }

    #[test]
    fn unknown_contents_are_rejected() {
        assert!(HelperDiagnostic::decode("something else\n").is_none());
        assert!(
            HelperDiagnostic::decode(&format!("{HEADER}\noutcome=odd\nkind=rollback\n")).is_none()
        );
    }

    #[test]
    fn taking_an_invalid_file_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("demo");
        let path = dir.path().join(".demo.gpui-auto-update-diagnostic");
        fs::write(&path, "garbage").unwrap();
        assert_eq!(
            take_diagnostic(&prefix).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(!path.exists());
        assert!(take_diagnostic(&prefix).unwrap().is_none());
    }

    #[test]
    fn a_recorded_diagnostic_is_taken_once() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("demo");
        let diagnostic = HelperDiagnostic::new(
            HelperOutcome::RolledBack,
            ErrorKind::HealthConfirmation,
            None,
            "exited",
        );
        record(&prefix, &diagnostic).unwrap();
        assert_eq!(take_diagnostic(&prefix).unwrap(), Some(diagnostic));
        assert!(take_diagnostic(&prefix).unwrap().is_none());
    }
}
