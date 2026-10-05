//! The public update state model and its transition rules.

use std::fmt;

use crate::capability::Capability;
use crate::error::{ErrorKind, UpdateError};

/// A named release track, such as `stable` or `beta`, that limits which feed
/// entries a check may select.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Channel(String);

impl Channel {
    /// Creates a channel with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The channel name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Markup of inline release notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReleaseNotesFormat {
    /// Plain text.
    PlainText,
    /// HTML, as in Sparkle's `<description>` element.
    Html,
    /// Markdown.
    Markdown,
}

/// Release notes of an available update, or where to find them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReleaseNotes {
    /// Release notes are published at this URL.
    Link(String),
    /// Release notes embedded in the feed.
    Inline {
        /// The release notes text.
        content: String,
        /// The markup of `content`.
        format: ReleaseNotesFormat,
    },
}

/// Metadata of a newer release selected by a check.
///
/// Construct it with [`AvailableUpdate::new`] and the `with_*` methods.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct AvailableUpdate {
    /// The user-presentable version (Sparkle's `shortVersionString`).
    pub version: String,
    /// The machine build version (Sparkle's `version`), when the feed
    /// distinguishes it from [`Self::version`].
    pub build: Option<String>,
    /// The release channel the update belongs to; `None` is the default
    /// channel.
    pub channel: Option<Channel>,
    /// Release notes or their location, when available.
    pub release_notes: Option<ReleaseNotes>,
    /// The publication date exactly as the feed states it (an RFC 2822
    /// `pubDate` for Sparkle-compatible feeds), when available.
    pub published: Option<String>,
    /// Whether the release is marked critical.
    pub critical: bool,
    /// Whether the release is marked as a major upgrade.
    pub major: bool,
}

impl AvailableUpdate {
    /// Creates update metadata for `version` with no optional fields set.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            build: None,
            channel: None,
            release_notes: None,
            published: None,
            critical: false,
            major: false,
        }
    }

    /// Sets the machine build version.
    pub fn with_build(mut self, build: impl Into<String>) -> Self {
        self.build = Some(build.into());
        self
    }

    /// Sets the release channel.
    pub fn with_channel(mut self, channel: Channel) -> Self {
        self.channel = Some(channel);
        self
    }

    /// Sets the release notes.
    pub fn with_release_notes(mut self, release_notes: ReleaseNotes) -> Self {
        self.release_notes = Some(release_notes);
        self
    }

    /// Sets the publication date as stated by the feed.
    pub fn with_published(mut self, published: impl Into<String>) -> Self {
        self.published = Some(published.into());
        self
    }

    /// Marks the release as critical.
    pub fn with_critical(mut self, critical: bool) -> Self {
        self.critical = critical;
        self
    }

    /// Marks the release as a major upgrade.
    pub fn with_major(mut self, major: bool) -> Self {
        self.major = major;
        self
    }
}

/// Bytes received so far for an artifact download.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DownloadProgress {
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total size in bytes, when known.
    pub total: Option<u64>,
}

impl DownloadProgress {
    /// Completed fraction in `0.0..=1.0`, or `None` when the total size is
    /// unknown.
    pub fn fraction(&self) -> Option<f64> {
        let total = self.total?;
        if total == 0 {
            return Some(1.0);
        }
        Some((self.downloaded.min(total) as f64) / (total as f64))
    }
}

/// What the updater is doing for this installation.
///
/// The variant set may grow, so matches must include a wildcard arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateState {
    /// Updating is disabled because the installation is externally managed,
    /// unsupported, or temporarily unable to update.
    Disabled {
        /// Why updating is disabled.
        capability: Capability,
    },
    /// Nothing has happened yet, or a previous outcome was dismissed.
    Idle,
    /// A check is running.
    Checking,
    /// The last check found no newer release.
    UpToDate,
    /// The last check found a newer release.
    Available(AvailableUpdate),
    /// The update's artifact is downloading.
    Downloading {
        /// The update being downloaded.
        update: AvailableUpdate,
        /// Download progress; the total may be unknown.
        progress: DownloadProgress,
    },
    /// The downloaded artifact's length and signature are being verified.
    Verifying(AvailableUpdate),
    /// The verified update is staged and ready to install.
    Staged(AvailableUpdate),
    /// The update is being installed.
    Installing(AvailableUpdate),
    /// Installation continues once the application quits.
    WaitingForQuit(AvailableUpdate),
    /// The application is relaunching into the new version.
    Relaunching(AvailableUpdate),
    /// The new version failed and the previous version was restored.
    RolledBack {
        /// The update that was rolled back.
        update: AvailableUpdate,
        /// Why the update was rolled back.
        error: UpdateError,
    },
    /// The update finished installing.
    Completed(AvailableUpdate),
    /// The last operation failed.
    Failed(UpdateError),
}

/// A step reported by a backend after a check found an update.
///
/// Checks themselves are driven by [`crate::UpdateCoordinator::check`], so
/// there are no check events here.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpdateEvent {
    /// Downloading the available update started.
    DownloadStarted {
        /// Total size in bytes, when known.
        total: Option<u64>,
    },
    /// More of the artifact was downloaded.
    DownloadProgressed(DownloadProgress),
    /// Verification of the downloaded artifact started.
    VerificationStarted,
    /// The verified update was staged and is ready to install.
    Staged,
    /// Installing the staged update started.
    InstallStarted,
    /// Installation continues after the application quits.
    WaitingForQuit,
    /// The application is relaunching.
    Relaunching,
    /// The update finished installing.
    Completed,
    /// The update failed after installing and the previous version was
    /// restored.
    RolledBack(UpdateError),
    /// The running operation failed.
    Failed(UpdateError),
    /// The user dismissed the current outcome; the state returns to idle.
    Dismissed,
}

impl UpdateState {
    /// Whether an operation is running that excludes other operations.
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking
                | Self::Downloading { .. }
                | Self::Verifying(_)
                | Self::Installing(_)
                | Self::WaitingForQuit(_)
                | Self::Relaunching(_)
        )
    }

    /// The update this state refers to, if any.
    pub fn update(&self) -> Option<&AvailableUpdate> {
        match self {
            Self::Available(update)
            | Self::Downloading { update, .. }
            | Self::Verifying(update)
            | Self::Staged(update)
            | Self::Installing(update)
            | Self::WaitingForQuit(update)
            | Self::Relaunching(update)
            | Self::RolledBack { update, .. }
            | Self::Completed(update) => Some(update),
            _ => None,
        }
    }

    /// Returns the state that `event` leads to, or an error when the event
    /// is incompatible with this state.
    ///
    /// Rejections have kind [`ErrorKind::OperationInProgress`] when this
    /// state is busy, and [`ErrorKind::InvalidState`] otherwise.
    pub fn apply(&self, event: UpdateEvent) -> Result<UpdateState, UpdateError> {
        use UpdateEvent as E;
        let next = match (self, event) {
            (Self::Available(update), E::DownloadStarted { total }) => Self::Downloading {
                update: update.clone(),
                progress: DownloadProgress {
                    downloaded: 0,
                    total,
                },
            },
            (Self::Downloading { update, .. }, E::DownloadProgressed(progress)) => {
                Self::Downloading {
                    update: update.clone(),
                    progress,
                }
            }
            (Self::Downloading { update, .. }, E::VerificationStarted) => {
                Self::Verifying(update.clone())
            }
            (Self::Downloading { update, .. } | Self::Verifying(update), E::Staged) => {
                Self::Staged(update.clone())
            }
            (Self::Staged(update), E::InstallStarted) => Self::Installing(update.clone()),
            (Self::Staged(update) | Self::Installing(update), E::WaitingForQuit) => {
                Self::WaitingForQuit(update.clone())
            }
            (Self::Installing(update) | Self::WaitingForQuit(update), E::Relaunching) => {
                Self::Relaunching(update.clone())
            }
            (
                Self::Installing(update) | Self::WaitingForQuit(update) | Self::Relaunching(update),
                E::Completed,
            ) => Self::Completed(update.clone()),
            (
                Self::Installing(update) | Self::WaitingForQuit(update) | Self::Relaunching(update),
                E::RolledBack(error),
            ) => Self::RolledBack {
                update: update.clone(),
                error,
            },
            (
                Self::Downloading { .. }
                | Self::Verifying(_)
                | Self::Staged(_)
                | Self::Installing(_)
                | Self::WaitingForQuit(_)
                | Self::Relaunching(_),
                E::Failed(error),
            ) => Self::Failed(error),
            (
                Self::UpToDate
                | Self::Available(_)
                | Self::Failed(_)
                | Self::RolledBack { .. }
                | Self::Completed(_),
                E::Dismissed,
            ) => Self::Idle,
            _ => return Err(self.rejection()),
        };
        Ok(next)
    }

    /// Whether a check may start from this state.
    pub(crate) fn check_rejection(&self) -> Option<UpdateError> {
        match self {
            Self::Idle
            | Self::UpToDate
            | Self::Available(_)
            | Self::Failed(_)
            | Self::RolledBack { .. }
            | Self::Completed(_) => None,
            Self::Disabled { capability } => Some(
                capability
                    .denial()
                    .unwrap_or_else(|| UpdateError::new(ErrorKind::InvalidState)),
            ),
            _ => Some(self.rejection()),
        }
    }

    fn rejection(&self) -> UpdateError {
        if self.is_busy() {
            UpdateError::new(ErrorKind::OperationInProgress)
        } else {
            UpdateError::new(ErrorKind::InvalidState)
        }
    }
}
