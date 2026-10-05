//! Downloading a selected release's artifact, verifying it, and staging it.
//!
//! [`ArtifactDownloader`] streams the artifact of a [`FeedItem`] into a
//! private temporary file, enforcing the declared length and a size limit
//! while bytes arrive, then verifies the Ed25519 signature over the bytes on
//! disk. Only a verified file is given its final name; until then it has a
//! random, extensionless name and owner-only permissions, so nothing that is
//! unverified can be executed or extracted by the updater.
//!
//! Every staged artifact lives in a fresh, randomly named directory created
//! inside a staging root chosen by the backend. No part of the staging path
//! comes from the feed: the file name is fixed by the backend
//! ([`ArtifactDownloader::with_file_name`]) and defaults to `artifact`.
//!
//! Calls block the current thread; run them on a background executor.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::coordinator::UpdateCoordinator;
use crate::error::{ErrorKind, UpdateError};
use crate::feed::{FeedItem, FeedLimits};
use crate::fetch::{self, FetchError, HttpClient};
use crate::state::{DownloadProgress, UpdateEvent};
use crate::trust::{TrustedKey, VerifyError};
use crate::version::ReleaseVersion;

const READ_CHUNK: usize = 64 * 1024;
const DEFAULT_FILE_NAME: &str = "artifact";
const MAX_FILE_NAME_LEN: usize = 128;

/// Why an artifact could not be downloaded, verified, or staged.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// The feed declares a length above the downloader's size limit, so the
    /// artifact was not requested.
    #[error("artifact length {length} exceeds the {limit}-byte limit")]
    ArtifactTooLarge {
        /// Length declared by the feed.
        length: u64,
        /// The configured limit.
        limit: u64,
    },
    /// The artifact could not be fetched, it exceeded the size limit while
    /// streaming, or the transfer timed out.
    #[error("could not download the artifact: {0}")]
    Fetch(#[from] FetchError),
    /// The server's `Content-Length` or the bytes received differ from the
    /// length declared by the feed. `actual` is a lower bound when the
    /// transfer was stopped for exceeding the declared length.
    #[error("artifact is {actual} bytes but the feed declares {expected}")]
    LengthMismatch {
        /// Length declared by the feed.
        expected: u64,
        /// Length announced or received.
        actual: u64,
    },
    /// The downloaded bytes do not verify against the trusted key.
    #[error("artifact signature does not verify against the trusted key")]
    BadSignature,
    /// The staging directory or file could not be created or written.
    #[error("could not stage the artifact: {0}")]
    Staging(#[source] io::Error),
    /// The progress callback stopped the download.
    #[error("the download was interrupted: {0}")]
    Interrupted(UpdateError),
}

/// A staged file name that is not a single plain component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "staged file name {0:?} must be 1 to 128 ASCII letters, digits, '.', '_' or '-', not starting with '.'"
)]
pub struct InvalidFileName(pub String);

impl From<DownloadError> for UpdateError {
    fn from(error: DownloadError) -> Self {
        let kind = match &error {
            DownloadError::Interrupted(inner) => return inner.clone(),
            DownloadError::ArtifactTooLarge { .. } | DownloadError::Fetch(_) => ErrorKind::Download,
            DownloadError::LengthMismatch { .. } => ErrorKind::LengthMismatch,
            DownloadError::BadSignature => ErrorKind::Signature,
            DownloadError::Staging(_) => ErrorKind::Staging,
        };
        UpdateError::new(kind)
            .with_diagnostic(error.to_string())
            .with_source(error)
    }
}

/// A verified artifact in its own staging directory.
///
/// Dropping it leaves the files in place so a later step (or a helper
/// process) can install them; call [`Self::discard`] to remove them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedArtifact {
    directory: PathBuf,
    path: PathBuf,
    expected_version: ReleaseVersion,
    length: u64,
}

impl StagedArtifact {
    /// The verified artifact file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The fresh directory created for this artifact. It contains only the
    /// artifact file and is the place to extract or prepare it.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The `sparkle:version` of the feed entry the artifact was downloaded
    /// for.
    ///
    /// Feed metadata is not signed: the signature proves the bytes came from
    /// the release key, not that they are this version. Before installing,
    /// backends must confirm that the version inside the artifact equals this
    /// one, so a relabeled older release cannot be installed as a newer one.
    pub fn expected_version(&self) -> &ReleaseVersion {
        &self.expected_version
    }

    /// Size of the verified artifact in bytes.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Removes the staging directory and everything in it.
    pub fn discard(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.directory)
    }
}

/// Downloads, verifies, and stages artifacts of selected feed items.
#[derive(Debug, Clone)]
pub struct ArtifactDownloader {
    client: HttpClient,
    key: TrustedKey,
    max_artifact_bytes: u64,
    file_name: String,
}

impl ArtifactDownloader {
    /// A downloader that trusts `key`, fetches with `client` (whose policy
    /// sets the timeouts), and allows artifacts up to the default
    /// [`FeedLimits::max_artifact_bytes`].
    pub fn new(client: HttpClient, key: TrustedKey) -> Self {
        Self {
            client,
            key,
            max_artifact_bytes: FeedLimits::default().max_artifact_bytes,
            file_name: DEFAULT_FILE_NAME.to_owned(),
        }
    }

    /// Replaces the artifact size limit.
    pub fn with_max_artifact_bytes(mut self, limit: u64) -> Self {
        self.max_artifact_bytes = limit;
        self
    }

    /// Sets the name the verified artifact is given in its staging
    /// directory, for example `setup.exe` when an installer must keep its
    /// extension. The name must be a single plain file name.
    pub fn with_file_name(mut self, name: &str) -> Result<Self, InvalidFileName> {
        let valid = !name.is_empty()
            && name.len() <= MAX_FILE_NAME_LEN
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !valid {
            return Err(InvalidFileName(name.to_owned()));
        }
        self.file_name = name.to_owned();
        Ok(self)
    }

    /// Downloads `item`'s artifact into a new directory under
    /// `staging_root` (created if missing), verifies it, and returns the
    /// staged file.
    ///
    /// `on_event` receives, in order, [`UpdateEvent::DownloadStarted`]
    /// before any network access, [`UpdateEvent::DownloadProgressed`] after
    /// each chunk received, and [`UpdateEvent::VerificationStarted`] once the
    /// declared number of bytes is on disk. Returning an error from it stops
    /// the download with [`DownloadError::Interrupted`]. Progress totals are
    /// the length declared by the feed, which the download must match.
    ///
    /// On any failure the partial download and its directory are removed.
    pub fn download(
        &self,
        item: &FeedItem,
        staging_root: &Path,
        mut on_event: impl FnMut(UpdateEvent) -> Result<(), UpdateError>,
    ) -> Result<StagedArtifact, DownloadError> {
        let artifact = &item.artifact;
        let expected = artifact.length;
        let mut emit = |event| on_event(event).map_err(DownloadError::Interrupted);

        emit(UpdateEvent::DownloadStarted {
            total: Some(expected),
        })?;
        if expected > self.max_artifact_bytes {
            return Err(DownloadError::ArtifactTooLarge {
                length: expected,
                limit: self.max_artifact_bytes,
            });
        }

        std::fs::create_dir_all(staging_root).map_err(DownloadError::Staging)?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("update-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let directory = builder
            .tempdir_in(staging_root)
            .map_err(DownloadError::Staging)?;
        let mut partial = tempfile::Builder::new()
            .prefix(".partial-")
            .tempfile_in(directory.path())
            .map_err(DownloadError::Staging)?;

        let mut body = self.client.open(&artifact.url, self.max_artifact_bytes)?;
        if let Some(announced) = body.content_length().filter(|&n| n != expected) {
            return Err(DownloadError::LengthMismatch {
                expected,
                actual: announced,
            });
        }

        let mut buf = vec![0u8; READ_CHUNK];
        let mut downloaded: u64 = 0;
        loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(fetch::read_error(e, self.max_artifact_bytes).into()),
            };
            downloaded = downloaded.saturating_add(n as u64);
            if downloaded > expected {
                return Err(DownloadError::LengthMismatch {
                    expected,
                    actual: downloaded,
                });
            }
            partial
                .write_all(&buf[..n])
                .map_err(DownloadError::Staging)?;
            emit(UpdateEvent::DownloadProgressed(DownloadProgress {
                downloaded,
                total: Some(expected),
            }))?;
        }
        if downloaded != expected {
            return Err(DownloadError::LengthMismatch {
                expected,
                actual: downloaded,
            });
        }
        partial.flush().map_err(DownloadError::Staging)?;
        partial
            .as_file()
            .sync_all()
            .map_err(DownloadError::Staging)?;

        emit(UpdateEvent::VerificationStarted)?;
        // Verify the bytes as stored, which are the bytes that will be used.
        let file: &mut File = partial.as_file_mut();
        file.seek(SeekFrom::Start(0))
            .map_err(DownloadError::Staging)?;
        self.key
            .verify_artifact(&artifact.signature, expected, BufReader::new(file))
            .map_err(|error| match error {
                VerifyError::BadSignature => DownloadError::BadSignature,
                VerifyError::LengthMismatch { expected, actual } => {
                    DownloadError::LengthMismatch { expected, actual }
                }
                VerifyError::Io(e) => DownloadError::Staging(e),
            })?;

        let path = directory.path().join(&self.file_name);
        partial
            .persist_noclobber(&path)
            .map_err(|e| DownloadError::Staging(e.error))?;
        let directory = directory.keep();
        Ok(StagedArtifact {
            directory,
            path,
            expected_version: item.version.clone(),
            length: expected,
        })
    }

    /// Like [`Self::download`], but reports every step to `coordinator`.
    ///
    /// The coordinator must be showing the available update; otherwise its
    /// rejection is returned and nothing is downloaded. On success the state
    /// becomes [`crate::UpdateState::Staged`]; on failure it becomes
    /// [`crate::UpdateState::Failed`] with the returned error.
    pub fn download_and_stage(
        &self,
        coordinator: &UpdateCoordinator,
        item: &FeedItem,
        staging_root: &Path,
    ) -> Result<StagedArtifact, UpdateError> {
        match self.download(item, staging_root, |event| coordinator.apply(event)) {
            Ok(staged) => {
                if let Err(error) = coordinator.apply(UpdateEvent::Staged) {
                    // The state moved on without us; do not leave files behind.
                    let _ = staged.discard();
                    return Err(error);
                }
                Ok(staged)
            }
            Err(DownloadError::Interrupted(error)) => Err(error),
            Err(error) => {
                let error = UpdateError::from(error);
                if let Err(rejected) = coordinator.apply(UpdateEvent::Failed(error.clone())) {
                    tracing::debug!(
                        kind_of_error = ?rejected.kind(),
                        "could not record the download failure in the update state"
                    );
                }
                tracing::warn!(
                    kind_of_error = ?error.kind(),
                    diagnostic = error.diagnostic(),
                    "artifact download failed: {error}"
                );
                Err(error)
            }
        }
    }
}
