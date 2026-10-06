//! The real engine: Sparkle 2 through the `sparkle-updater` crate.
//!
//! `sparkle-updater` owns the Objective-C interoperability (and its unsafe
//! code); this module only moves calls onto the main thread, where Sparkle
//! must be used, and converts its types.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use dispatch2::{DispatchQueue, MainThreadBound};
use gpui_auto_update_core::{ErrorKind, UpdateError};
use sparkle_updater::events::{ErrorPayload, NoUpdateReason as RawReason, UpdateInfo};
use sparkle_updater::{MainThreadMarker, SparkleUpdater, UpdateEvent, UpdaterConfig};

use crate::backend::SparkleBackend;
use crate::engine::SparkleEngine;
use crate::event::{
    NoUpdateReason, SparkleError, SparkleEvent, SparkleUpdate, UpdateStage, UserChoice,
};
use crate::hub::SparkleEvents;

/// How long a call from another thread waits for the main thread. GPUI's
/// main thread services the main dispatch queue between frames, so this only
/// expires when the main thread is blocked.
const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(10);

impl SparkleBackend {
    /// Starts Sparkle's standard updater controller (`SPUStandardUpdaterController`
    /// and `SPUUpdater`) for the running application bundle.
    ///
    /// Call it once, on the main thread, after the application has finished
    /// launching (for example inside GPUI's `Application::run`), and keep
    /// the backend for the application's lifetime. Sparkle reads its
    /// configuration (`SUFeedURL`, `SUPublicEDKey`, and so on) from the
    /// bundle's `Info.plist`.
    ///
    /// Returns [`SparkleBackend::unsupported`] when the executable is not
    /// inside an application bundle, a [`ErrorKind::Configuration`] error
    /// when called off the main thread, and Sparkle's start-up error (for
    /// example a missing or insecure feed URL) as a
    /// [`ErrorKind::Configuration`] error.
    pub fn start() -> Result<Self, UpdateError> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Err(UpdateError::new(ErrorKind::Configuration)
                .with_diagnostic("Sparkle must be started on the main thread"));
        };
        let events = SparkleEvents::new();
        let sink = events.clone();
        let config = UpdaterConfig {
            event_callback: Some(Rc::new(move |event| {
                if let Some(event) = convert_event(event) {
                    sink.publish(event);
                }
            })),
            ..UpdaterConfig::default()
        };
        match SparkleUpdater::new(mtm, config) {
            Ok(Some(updater)) => Ok(Self::new(
                NativeEngine {
                    updater: Arc::new(MainThreadBound::new(updater, mtm)),
                },
                events,
            )),
            Ok(None) => {
                tracing::info!(
                    "not running from an application bundle; Sparkle updates are unavailable"
                );
                Ok(Self::unsupported())
            }
            Err(error) => Err(UpdateError::new(ErrorKind::Configuration)
                .with_diagnostic(format!("Sparkle did not start: {error}"))
                .with_source(error)),
        }
    }
}

struct NativeEngine {
    updater: Arc<MainThreadBound<SparkleUpdater>>,
}

impl NativeEngine {
    /// Runs `f` with the updater on the main thread: directly when already
    /// there, otherwise through the main dispatch queue with a timeout.
    ///
    /// Waiting with a timeout rather than `dispatch_sync` keeps a blocked
    /// main thread (for example one waiting on a lock the caller holds) from
    /// deadlocking the caller.
    fn on_main<R, F>(&self, f: F) -> Result<R, UpdateError>
    where
        R: Send + 'static,
        F: FnOnce(&SparkleUpdater) -> sparkle_updater::Result<R> + Send + 'static,
    {
        let result = if let Some(mtm) = MainThreadMarker::new() {
            f(self.updater.get(mtm))
        } else {
            let (sender, receiver) = mpsc::sync_channel(1);
            let updater = Arc::clone(&self.updater);
            DispatchQueue::main().exec_async(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    let _ = sender.send(f(updater.get(mtm)));
                }
            });
            receiver.recv_timeout(MAIN_THREAD_TIMEOUT).map_err(|_| {
                UpdateError::new(ErrorKind::TemporarilyUnavailable).with_diagnostic(format!(
                    "the main thread did not run the Sparkle call within {MAIN_THREAD_TIMEOUT:?}"
                ))
            })?
        };
        result.map_err(|error| {
            UpdateError::new(ErrorKind::Internal)
                .with_diagnostic(format!("Sparkle call failed: {error}"))
                .with_source(error)
        })
    }
}

impl SparkleEngine for NativeEngine {
    fn check_for_updates(&self) -> Result<(), UpdateError> {
        self.on_main(|u| u.check_for_updates())
    }

    fn check_for_updates_in_background(&self) -> Result<(), UpdateError> {
        self.on_main(|u| u.check_for_updates_in_background())
    }

    fn session_in_progress(&self) -> Result<bool, UpdateError> {
        self.on_main(|u| u.session_in_progress())
    }

    fn automatically_checks_for_updates(&self) -> Result<bool, UpdateError> {
        self.on_main(|u| u.automatically_checks_for_updates())
    }

    fn set_automatically_checks_for_updates(&self, enabled: bool) -> Result<(), UpdateError> {
        self.on_main(move |u| u.set_automatically_checks_for_updates(enabled))
    }

    fn last_update_check(&self) -> Result<Option<SystemTime>, UpdateError> {
        let millis = self.on_main(|u| u.last_update_check_date())?;
        Ok(millis.and_then(|millis| {
            let since_epoch = Duration::try_from_secs_f64(millis / 1000.0).ok()?;
            SystemTime::UNIX_EPOCH.checked_add(since_epoch)
        }))
    }

    fn allowed_channels(&self) -> Result<Option<Vec<String>>, UpdateError> {
        self.on_main(|u| u.allowed_channels())
    }

    fn set_allowed_channels(&self, channels: Option<Vec<String>>) -> Result<(), UpdateError> {
        self.on_main(move |u| u.set_allowed_channels(channels))
    }
}

fn convert_event(event: UpdateEvent) -> Option<SparkleEvent> {
    Some(match event {
        UpdateEvent::DidFinishLoadingAppcast => SparkleEvent::AppcastLoaded,
        UpdateEvent::DidFindValidUpdate(info) => SparkleEvent::UpdateFound(convert_update(info)),
        UpdateEvent::DidNotFindUpdate(info) => SparkleEvent::NoUpdateFound(match info.reason {
            RawReason::OnLatestVersion => NoUpdateReason::OnLatestVersion,
            RawReason::OnNewerThanLatestVersion => NoUpdateReason::OnNewerThanLatestVersion,
            RawReason::SystemIsTooOld => NoUpdateReason::SystemTooOld,
            RawReason::SystemIsTooNew => NoUpdateReason::SystemTooNew,
            RawReason::HardwareDoesNotSupportArm64 => NoUpdateReason::HardwareUnsupported,
            RawReason::Unknown => NoUpdateReason::Unknown,
        }),
        UpdateEvent::WillDownloadUpdate(v) => SparkleEvent::WillDownload { version: v.version },
        UpdateEvent::DidDownloadUpdate(v) => SparkleEvent::Downloaded { version: v.version },
        UpdateEvent::WillInstallUpdate(v) => SparkleEvent::WillInstall { version: v.version },
        UpdateEvent::DidAbortWithError(error) => SparkleEvent::Aborted(convert_error(error)),
        UpdateEvent::DidFinishUpdateCycle(info) => SparkleEvent::CycleFinished {
            error: info.error.map(convert_error),
        },
        UpdateEvent::FailedToDownloadUpdate(info) => SparkleEvent::DownloadFailed {
            version: info.version,
            error: convert_error(info.error),
        },
        UpdateEvent::UserDidCancelDownload => SparkleEvent::DownloadCanceled,
        UpdateEvent::WillExtractUpdate(v) => SparkleEvent::WillExtract { version: v.version },
        UpdateEvent::DidExtractUpdate(v) => SparkleEvent::Extracted { version: v.version },
        UpdateEvent::WillRelaunchApplication => SparkleEvent::WillRelaunch,
        UpdateEvent::UserDidMakeChoice(info) => SparkleEvent::UserChoice {
            choice: match info.choice.as_str() {
                "skip" => UserChoice::Skip,
                "install" => UserChoice::Install,
                _ => UserChoice::Dismiss,
            },
            stage: match info.stage.as_str() {
                "notDownloaded" => UpdateStage::NotDownloaded,
                "downloaded" => UpdateStage::Downloaded,
                "installing" => UpdateStage::Installing,
                _ => UpdateStage::Unknown,
            },
            version: info.version,
        },
        UpdateEvent::WillScheduleUpdateCheck(info) => SparkleEvent::CheckScheduled {
            delay: Duration::try_from_secs_f64(info.delay).unwrap_or_default(),
        },
        UpdateEvent::WillNotScheduleUpdateCheck => SparkleEvent::CheckNotScheduled,
        UpdateEvent::WillInstallUpdateOnQuit(v) => {
            SparkleEvent::WillInstallOnQuit { version: v.version }
        }
        _ => return None,
    })
}

fn convert_update(info: UpdateInfo) -> SparkleUpdate {
    let mut update = SparkleUpdate::new(info.version);
    update.title = info.title;
    update.channel = info.channel;
    update.release_notes = info.release_notes;
    update.release_notes_format = info.item_description_format;
    update.release_notes_url = info.release_notes_url;
    update.full_release_notes_url = info.full_release_notes_url;
    update.published = info.date_string;
    update.critical = info.is_critical;
    update.major = info.is_major_upgrade;
    update.information_only = info.is_information_only;
    update
}

fn convert_error(error: ErrorPayload) -> SparkleError {
    let mut converted = SparkleError::new(error.domain, error.code, error.message);
    converted.failure_reason = error.failure_reason;
    converted
}
