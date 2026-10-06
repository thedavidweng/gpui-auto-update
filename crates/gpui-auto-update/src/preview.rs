//! Deterministic preview states for developing and testing update UI.

use gpui_auto_update_core::{
    AvailableUpdate, Capability, Channel, DownloadProgress, ErrorKind, ReleaseNotes,
    ReleaseNotesFormat, UpdateError, UpdateState,
};

/// The version every preview update carries, so a preview can never be
/// mistaken for a real release.
pub const PREVIEW_VERSION: &str = "0.0.0-preview";

/// The channel every preview update belongs to.
pub const PREVIEW_CHANNEL: &str = "preview";

/// A fixed, clearly marked update state for UI development.
///
/// While an updater shows a preview it never checks, downloads, installs,
/// quits, or restarts: the operations that would do so are rejected with
/// [`ErrorKind::InvalidState`], and manual checks report the outcome the
/// preview implies. Every preview update has version [`PREVIEW_VERSION`] and
/// channel [`PREVIEW_CHANNEL`], and every preview message starts with
/// `"Preview:"`, so preview UI is distinguishable from a real updater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PreviewState {
    /// A newer release is available.
    UpdateAvailable,
    /// The update is downloading (42 of 100 MiB).
    Downloading,
    /// The update is downloaded, verified, and ready to install.
    ReadyToInstall,
    /// The last operation failed.
    Error,
    /// The installation is updated by a package manager.
    ExternallyManaged,
    /// The update is installed and takes effect after a restart.
    RestartRequired,
}

impl PreviewState {
    /// Every preview state, in lifecycle order.
    pub const ALL: [PreviewState; 6] = [
        Self::UpdateAvailable,
        Self::Downloading,
        Self::ReadyToInstall,
        Self::Error,
        Self::ExternallyManaged,
        Self::RestartRequired,
    ];

    /// The marked update that preview states refer to.
    pub fn update() -> AvailableUpdate {
        AvailableUpdate::new(PREVIEW_VERSION)
            .with_channel(Channel::new(PREVIEW_CHANNEL))
            .with_release_notes(ReleaseNotes::Inline {
                content: "Preview: this update is not real. Nothing is downloaded or installed."
                    .to_owned(),
                format: ReleaseNotesFormat::PlainText,
            })
    }

    /// The update state this preview shows.
    pub fn state(self) -> UpdateState {
        const MIB: u64 = 1024 * 1024;
        match self {
            Self::UpdateAvailable => UpdateState::Available(Self::update()),
            Self::Downloading => UpdateState::Downloading {
                update: Self::update(),
                progress: DownloadProgress {
                    downloaded: 42 * MIB,
                    total: Some(100 * MIB),
                },
            },
            Self::ReadyToInstall => UpdateState::Staged(Self::update()),
            Self::Error => UpdateState::Failed(Self::error()),
            Self::ExternallyManaged => UpdateState::Disabled {
                capability: Self::externally_managed(),
            },
            Self::RestartRequired => UpdateState::WaitingForQuit(Self::update()),
        }
    }

    pub(crate) fn error() -> UpdateError {
        UpdateError::new(ErrorKind::FeedRetrieval)
            .with_message("Preview: the update server could not be reached.")
    }

    pub(crate) fn externally_managed() -> Capability {
        Capability::ExternallyManaged {
            manager: Some("Preview package manager".to_owned()),
        }
    }

    pub(crate) fn rejection() -> UpdateError {
        UpdateError::new(ErrorKind::InvalidState)
            .with_message("Preview: update actions are disabled while previewing.")
    }
}
