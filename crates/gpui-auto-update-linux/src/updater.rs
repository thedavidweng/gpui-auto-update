//! The Linux update flow for one managed install, independent of any UI
//! framework: stage a checked release, hand it to the helper, and report
//! what the helper left behind.

use std::fs;
use std::os::unix::fs::DirBuilderExt as _;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gpui_auto_update_core::check::FeedCheckSource;
use gpui_auto_update_core::download::{ArtifactDownloader, DownloadError};
use gpui_auto_update_core::feed::Arch;
use gpui_auto_update_core::{Capability, ErrorKind, UpdateCoordinator, UpdateError};

use crate::detect::{Detection, ManagedInstall};
use crate::diagnostic::take_diagnostic;
use crate::extract::{ReleaseStager, StagedRelease};
use crate::handoff::{HandoffRequest, HelperCommand};
use crate::health::{StartupConfirmation, confirm_startup_for};
use crate::siblings;

/// Stages and installs updates for the managed install found by detection.
///
/// It shares the [`FeedCheckSource`] that the coordinator checks with, so
/// it knows which signed artifact belongs to the available update. Every
/// method blocks; call them on a background executor.
#[derive(Debug)]
pub struct LinuxUpdater {
    detection: Detection,
    source: Arc<FeedCheckSource>,
    downloader: ArtifactDownloader,
    stager: ReleaseStager,
    helper: Option<HelperCommand>,
    staged: Mutex<Option<StagedRelease>>,
}

impl LinuxUpdater {
    /// An updater for the installation described by `detection`, which
    /// downloads the release selected by `source` with `downloader`.
    ///
    /// The helper is the running executable, with default timeouts, and
    /// releases are staged for the build architecture with default archive
    /// limits.
    pub fn new(
        detection: Detection,
        source: Arc<FeedCheckSource>,
        downloader: ArtifactDownloader,
    ) -> Self {
        Self {
            detection,
            source,
            downloader,
            stager: ReleaseStager::new(Arch::current().unwrap_or(Arch::X86_64)),
            helper: HelperCommand::current().ok(),
            staged: Mutex::new(None),
        }
    }

    /// Replaces the stager, for example to change the archive limits.
    pub fn with_stager(mut self, stager: ReleaseStager) -> Self {
        self.stager = stager;
        self
    }

    /// Replaces the helper command, for example to change its timeouts.
    pub fn with_helper(mut self, helper: HelperCommand) -> Self {
        self.helper = Some(helper);
        self
    }

    /// The detection result this updater was created with.
    pub fn detection(&self) -> &Detection {
        &self.detection
    }

    /// Whether this installation may update itself.
    pub fn capability(&self) -> Capability {
        self.detection.capability().clone()
    }

    /// Downloads, verifies, and extracts the update that the last check
    /// selected into a staging install next to the managed install.
    ///
    /// Download and verification progress is applied to `coordinator`,
    /// which must be showing the available update. On success the caller
    /// moves the state to staged; nothing in the running install changed.
    pub fn stage(&self, coordinator: &UpdateCoordinator) -> Result<(), UpdateError> {
        let install = self.install()?;
        let selected = self.source.selected().ok_or_else(|| {
            UpdateError::new(ErrorKind::InvalidState)
                .with_message("There is no update to install right now.")
        })?;
        let downloads = siblings::fixed(install.prefix(), siblings::DOWNLOADS)
            .and_then(|path| {
                match fs::DirBuilder::new().mode(0o700).create(&path) {
                    Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
                        return Err(error);
                    }
                    _ => {}
                }
                if fs::symlink_metadata(&path)?.is_dir() {
                    Ok(path)
                } else {
                    Err(std::io::Error::other(
                        "the download location is not a directory",
                    ))
                }
            })
            .map_err(staging_error)?;

        let artifact = self
            .downloader
            .download(&selected.item, &downloads, |event| coordinator.apply(event))
            .map_err(|error| match error {
                DownloadError::Interrupted(error) => error,
                error => UpdateError::from(error),
            });
        let staged = artifact.and_then(|artifact| {
            let staged = self.stager.stage(install, &artifact);
            let _ = artifact.discard();
            staged.map_err(UpdateError::from)
        });
        let _ = fs::remove_dir(&downloads);

        let staged = staged?;
        if let Some(previous) = self.lock().replace(staged) {
            let _ = previous.discard();
        }
        Ok(())
    }

    /// Hands the staged release to the helper and waits for its
    /// acknowledgement that both the current and the staged installation
    /// are acceptable.
    ///
    /// On success the helper waits for this process to exit; the caller must
    /// then quit the application through its normal quit path, after which
    /// the helper swaps the install, relaunches, and waits for
    /// [`Self::confirm_startup`] from the new version. On failure nothing was
    /// changed and the staged release is discarded.
    pub fn hand_off(&self) -> Result<(), UpdateError> {
        let install = self.install()?;
        let staged = self.lock().take().ok_or_else(|| {
            UpdateError::new(ErrorKind::InvalidState)
                .with_message("There is no staged update to install.")
        })?;
        let Some(helper) = &self.helper else {
            let _ = staged.discard();
            return Err(UpdateError::new(ErrorKind::HelperLaunch)
                .with_diagnostic("the running executable could not be resolved"));
        };
        match helper.hand_off(&HandoffRequest::for_release(install, &staged)) {
            Ok(pending) => {
                pending.commit();
                Ok(())
            }
            Err(error) => {
                let _ = staged.discard();
                Err(error.into())
            }
        }
    }

    /// Reads and removes what the helper recorded about the last update
    /// attempt, as a user-presentable error. Call it once at startup.
    pub fn take_previous_failure(&self) -> Option<UpdateError> {
        let install = self.detection.install()?;
        match take_diagnostic(install.prefix()) {
            Ok(diagnostic) => diagnostic.map(UpdateError::from),
            Err(error) => Some(
                UpdateError::new(ErrorKind::Internal)
                    .with_message("The previous update reported a problem that could not be read.")
                    .with_diagnostic(format!("unreadable helper diagnostic: {error}")),
            ),
        }
    }

    /// Tells the helper that this version started successfully; see
    /// [`confirm_startup`](crate::confirm_startup). Does nothing when the
    /// process was not launched by the helper.
    pub fn confirm_startup(&self) -> Result<StartupConfirmation, UpdateError> {
        match self.detection.install() {
            Some(install) => Ok(confirm_startup_for(install.prefix())?),
            None => Ok(crate::health::confirm_startup()?),
        }
    }

    fn install(&self) -> Result<&ManagedInstall, UpdateError> {
        self.detection.install().ok_or_else(|| {
            self.detection
                .capability()
                .denial()
                .unwrap_or_else(|| UpdateError::new(ErrorKind::UnsupportedInstallation))
        })
    }

    fn lock(&self) -> MutexGuard<'_, Option<StagedRelease>> {
        self.staged.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn staging_error(error: std::io::Error) -> UpdateError {
    UpdateError::new(ErrorKind::Staging)
        .with_diagnostic(format!("could not prepare the download directory: {error}"))
        .with_source(error)
}
