//! The install/handoff contract that platform backends implement.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_auto_update_core::{
    AvailableUpdate, Capability, Channel, DownloadProgress, ErrorKind, UpdateCoordinator,
    UpdateError, UpdateEvent, UpdateState,
};

/// Downloads, verifies, stages, and installs updates for one platform.
///
/// The facade calls every method on GPUI's background executor, never on the
/// foreground thread, so implementations may block on network and
/// filesystem I/O. Calls never overlap: the facade runs at most one stage,
/// install, or relaunch at a time.
///
/// Checks are not part of this trait; they are resolved by the
/// [`CheckSource`](gpui_auto_update_core::CheckSource) passed to
/// [`UpdaterConfig::new`](crate::UpdaterConfig::new). A backend that needs to
/// know which artifact belongs to a checked [`AvailableUpdate`] typically
/// shares state with that source.
pub trait UpdateBackend: Send + Sync + 'static {
    /// Whether this installation may update itself. Called once while the
    /// updater starts, unless the application supplies a capability with
    /// [`UpdaterConfig::with_capability`](crate::UpdaterConfig::with_capability).
    fn capability(&self) -> Capability;

    /// Downloads, verifies, and stages `update` so it can be installed
    /// without further network access.
    ///
    /// When this is called the state is already
    /// [`UpdateState::Downloading`] with an unknown total; report finer
    /// progress through `progress`. Returning `Ok` moves the state to
    /// [`UpdateState::Staged`]; returning an error moves it to
    /// [`UpdateState::Failed`].
    fn stage(&self, update: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError>;

    /// Installs the staged `update`, or prepares the helper or installer
    /// that will, and says how the application must end.
    ///
    /// The application's prepare-to-install hooks have completed before
    /// this is called, and the state is [`UpdateState::Installing`].
    fn install(
        &self,
        update: &AvailableUpdate,
        progress: &ProgressSink,
    ) -> Result<Handoff, UpdateError>;

    /// Says how to relaunch after an installation that completes, or has
    /// completed, outside the running application (state
    /// [`UpdateState::WaitingForQuit`] or [`UpdateState::Completed`]).
    ///
    /// Defaults to GPUI's own restart.
    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        Ok(Handoff::Restart { restart_path: None })
    }

    /// The release channel checks currently select from; `None` is the
    /// default channel.
    fn channel(&self) -> Option<Channel> {
        None
    }

    /// Changes the release channel. Defaults to a
    /// [`ErrorKind::Configuration`] error for backends without channels.
    fn set_channel(&self, channel: Option<Channel>) -> Result<(), UpdateError> {
        let _ = channel;
        Err(UpdateError::new(ErrorKind::Configuration)
            .with_message("This application does not offer update channels."))
    }
}

impl<T: UpdateBackend + ?Sized> UpdateBackend for Arc<T> {
    fn capability(&self) -> Capability {
        (**self).capability()
    }
    fn stage(&self, update: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError> {
        (**self).stage(update, progress)
    }
    fn install(
        &self,
        update: &AvailableUpdate,
        progress: &ProgressSink,
    ) -> Result<Handoff, UpdateError> {
        (**self).install(update, progress)
    }
    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        (**self).relaunch()
    }
    fn channel(&self) -> Option<Channel> {
        (**self).channel()
    }
    fn set_channel(&self, channel: Option<Channel>) -> Result<(), UpdateError> {
        (**self).set_channel(channel)
    }
}

impl<T: UpdateBackend + ?Sized> UpdateBackend for Box<T> {
    fn capability(&self) -> Capability {
        (**self).capability()
    }
    fn stage(&self, update: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError> {
        (**self).stage(update, progress)
    }
    fn install(
        &self,
        update: &AvailableUpdate,
        progress: &ProgressSink,
    ) -> Result<Handoff, UpdateError> {
        (**self).install(update, progress)
    }
    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        (**self).relaunch()
    }
    fn channel(&self) -> Option<Channel> {
        (**self).channel()
    }
    fn set_channel(&self, channel: Option<Channel>) -> Result<(), UpdateError> {
        (**self).set_channel(channel)
    }
}

/// How the application ends after an install, as decided by the backend.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Handoff {
    /// The facade restarts the application with GPUI's restart mechanism
    /// (`App::restart`), launching `restart_path` instead of the application
    /// itself when set (for example a helper that finishes the update).
    Restart {
        /// What GPUI launches once the application has exited.
        restart_path: Option<PathBuf>,
    },
    /// A helper or installer has been started and owns relaunching; the
    /// facade quits the application (`App::quit`) without restarting it.
    Quit,
    /// The backend terminates and relaunches the application itself (for
    /// example Sparkle's installer); the facade does nothing further.
    BackendOwned,
}

/// Reports progress of a running stage or install to the updater.
///
/// Every report updates the [`UpdateState`] and is delivered to GPUI
/// observers on the foreground thread. Reports that do not fit the current
/// state are ignored and logged.
#[derive(Clone, Debug)]
pub struct ProgressSink {
    coordinator: UpdateCoordinator,
}

impl ProgressSink {
    /// A sink that applies reports to `coordinator`.
    pub fn new(coordinator: UpdateCoordinator) -> Self {
        Self { coordinator }
    }

    /// The artifact download started; `total` is its size when known.
    pub fn download_started(&self, total: Option<u64>) {
        let event = if matches!(self.coordinator.state(), UpdateState::Downloading { .. }) {
            UpdateEvent::DownloadProgressed(DownloadProgress {
                downloaded: 0,
                total,
            })
        } else {
            UpdateEvent::DownloadStarted { total }
        };
        self.apply(event);
    }

    /// More of the artifact was downloaded.
    pub fn download_progressed(&self, progress: DownloadProgress) {
        self.apply(UpdateEvent::DownloadProgressed(progress));
    }

    /// Verification of the downloaded artifact started.
    pub fn verification_started(&self) {
        self.apply(UpdateEvent::VerificationStarted);
    }

    /// Applies any backend step, for backends that report more than
    /// download progress.
    pub fn report(&self, event: UpdateEvent) {
        self.apply(event);
    }

    fn apply(&self, event: UpdateEvent) {
        if let Err(error) = self.coordinator.apply(event.clone()) {
            tracing::debug!(?event, kind_of_error = ?error.kind(), "ignored backend progress report");
        }
    }
}

/// A backend for installations this library cannot update: it reports
/// [`Capability::Unsupported`] and refuses every operation.
#[derive(Clone, Copy, Debug, Default)]
pub struct UnsupportedBackend;

impl UpdateBackend for UnsupportedBackend {
    fn capability(&self) -> Capability {
        Capability::Unsupported
    }

    fn stage(&self, _: &AvailableUpdate, _: &ProgressSink) -> Result<(), UpdateError> {
        Err(UpdateError::new(ErrorKind::UnsupportedInstallation))
    }

    fn install(&self, _: &AvailableUpdate, _: &ProgressSink) -> Result<Handoff, UpdateError> {
        Err(UpdateError::new(ErrorKind::UnsupportedInstallation))
    }

    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        Err(UpdateError::new(ErrorKind::UnsupportedInstallation))
    }
}
