//! Sparkle's lifecycle notifications, in a form that does not depend on the
//! Sparkle framework.
//!
//! The real engine converts the notifications of Sparkle's updater delegate
//! into these types; tests and alternative engines construct them directly.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// One lifecycle notification from Sparkle's updater.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum SparkleEvent {
    /// The appcast was downloaded and parsed.
    AppcastLoaded,
    /// A check found a valid update (`updater:didFindValidUpdate:`).
    UpdateFound(SparkleUpdate),
    /// A check found no applicable update
    /// (`updaterDidNotFindUpdate:error:`).
    NoUpdateFound(NoUpdateReason),
    /// Downloading the update is about to start.
    WillDownload {
        /// The update's display version.
        version: String,
    },
    /// The update finished downloading.
    Downloaded {
        /// The update's display version.
        version: String,
    },
    /// The update could not be downloaded.
    DownloadFailed {
        /// The update's display version.
        version: String,
        /// Why the download failed.
        error: SparkleError,
    },
    /// The user canceled the download in Sparkle's window.
    DownloadCanceled,
    /// Extracting and validating the downloaded update is about to start.
    WillExtract {
        /// The update's display version.
        version: String,
    },
    /// The update was extracted and validated and is ready to install.
    Extracted {
        /// The update's display version.
        version: String,
    },
    /// Sparkle is about to install the update.
    WillInstall {
        /// The update's display version.
        version: String,
    },
    /// Sparkle will install the update when the application quits.
    WillInstallOnQuit {
        /// The update's display version.
        version: String,
    },
    /// Sparkle is about to relaunch the application into the new version.
    WillRelaunch,
    /// The user answered Sparkle's update prompt.
    UserChoice {
        /// What the user chose.
        choice: UserChoice,
        /// How far the update had progressed when the user chose.
        stage: UpdateStage,
        /// The update's display version.
        version: String,
    },
    /// Sparkle is about to install and relaunch `update` and agreed to
    /// wait for the application's save hooks
    /// (`updater:shouldPostponeRelaunchForUpdate:untilInvokingBlock:`).
    /// Resume `continuation` once saving completed; dropping it leaves the
    /// relaunch postponed, and Sparkle installs the update when the
    /// application quits.
    RelaunchRequested {
        /// The update being installed.
        update: SparkleUpdate,
        /// Resumes the postponed relaunch.
        continuation: RelaunchContinuation,
    },
    /// The update session stopped because of an error
    /// (`updater:didAbortWithError:`). Sparkle also reports "no update
    /// found" this way; see [`SparkleError::is_no_update`].
    Aborted(SparkleError),
    /// The update cycle that a check started finished, with the error that
    /// ended it, if any.
    CycleFinished {
        /// The error that ended the cycle.
        error: Option<SparkleError>,
    },
    /// Sparkle scheduled its next automatic check.
    CheckScheduled {
        /// How long until the scheduled check runs.
        delay: Duration,
    },
    /// Sparkle will not schedule automatic checks (they are disabled).
    CheckNotScheduled,
    /// Sparkle is about to present an update session
    /// (`standardUserDriverWillHandleShowingUpdate:forUpdate:state:`) —
    /// in its own window (`handled_by_sparkle`) or leaving presentation to
    /// the application, a gentle reminder for a scheduled discovery.
    WillShowUpdate {
        /// Whether Sparkle's own UI presents the update.
        handled_by_sparkle: bool,
        /// The update being presented.
        update: SparkleUpdate,
        /// Where the session stands and whether the user asked for it.
        session: SessionState,
    },
    /// The user interacted with a reminder the application presented
    /// (`standardUserDriverDidReceiveUserAttentionForUpdate:`); clear
    /// attention indicators such as badges.
    UserAttentionReceived {
        /// The update the reminder was about.
        update: SparkleUpdate,
    },
    /// The update session is ending
    /// (`standardUserDriverWillFinishUpdateSession`); remove any reminder
    /// UI.
    SessionWillFinish,
}

/// Where an update session being presented stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionState {
    /// How far the update had progressed.
    pub stage: UpdateStage,
    /// Whether the user asked for the check that found the update.
    pub user_initiated: bool,
}

/// What resumes a postponed relaunch; the real engine's closure hops to
/// the main thread first.
type Resume = Box<dyn FnOnce() + Send>;

/// A Sparkle install-and-relaunch postponed until the application's save
/// hooks completed; see [`SparkleEvent::RelaunchRequested`].
///
/// Resuming is one-shot and may happen from any thread: only the first
/// call has an effect. Clones refer to the same continuation, so a value
/// delivered to several observers can still be resumed only once.
#[derive(Clone)]
pub struct RelaunchContinuation {
    resume: Arc<Mutex<Option<Resume>>>,
}

impl RelaunchContinuation {
    /// A continuation that resumes the postponed relaunch by running
    /// `resume`. The real engine hops to the main thread first; test
    /// engines may pass any closure.
    pub fn from_fn(resume: impl FnOnce() + Send + 'static) -> Self {
        Self {
            resume: Arc::new(Mutex::new(Some(Box::new(resume)))),
        }
    }

    /// Lets Sparkle continue installing and relaunching. Dropping the
    /// continuation without resuming leaves the relaunch postponed.
    pub fn resume(self) {
        let resume = self
            .resume
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(resume) = resume {
            resume();
        }
    }
}

impl std::fmt::Debug for RelaunchContinuation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelaunchContinuation")
            .finish_non_exhaustive()
    }
}

impl PartialEq for RelaunchContinuation {
    /// Two continuations are equal when they resume the same postponed
    /// relaunch.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.resume, &other.resume)
    }
}

impl Eq for RelaunchContinuation {}

/// An update Sparkle found in the appcast (`SUAppcastItem`).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SparkleUpdate {
    /// The display version (`sparkle:shortVersionString`, falling back to
    /// `sparkle:version`).
    pub version: String,
    /// The item's title.
    pub title: Option<String>,
    /// The item's channel; `None` is the default channel.
    pub channel: Option<String>,
    /// Inline release notes (the item's `<description>`).
    pub release_notes: Option<String>,
    /// The markup of [`Self::release_notes`] as the appcast declares it
    /// (`sparkle:format`, for example `html`, `plain-text`, or `markdown`).
    pub release_notes_format: Option<String>,
    /// Where release notes for this version are published.
    pub release_notes_url: Option<String>,
    /// Where cumulative release notes are published.
    pub full_release_notes_url: Option<String>,
    /// The publication date exactly as the appcast states it.
    pub published: Option<String>,
    /// Whether the update is marked critical.
    pub critical: bool,
    /// Whether the update is a major upgrade.
    pub major: bool,
    /// Whether the item only informs about a release and cannot be
    /// installed by Sparkle.
    pub information_only: bool,
}

impl SparkleUpdate {
    /// An update for `version` with no optional metadata.
    pub fn new(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            title: None,
            channel: None,
            release_notes: None,
            release_notes_format: None,
            release_notes_url: None,
            full_release_notes_url: None,
            published: None,
            critical: false,
            major: false,
            information_only: false,
        }
    }

    /// Sets the channel.
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// Sets inline release notes and their declared format.
    pub fn with_release_notes(mut self, notes: impl Into<String>, format: Option<&str>) -> Self {
        self.release_notes = Some(notes.into());
        self.release_notes_format = format.map(str::to_owned);
        self
    }

    /// Sets the release notes link.
    pub fn with_release_notes_url(mut self, url: impl Into<String>) -> Self {
        self.release_notes_url = Some(url.into());
        self
    }

    /// Sets the publication date as stated by the appcast.
    pub fn with_published(mut self, published: impl Into<String>) -> Self {
        self.published = Some(published.into());
        self
    }

    /// Marks the update critical.
    pub fn with_critical(mut self, critical: bool) -> Self {
        self.critical = critical;
        self
    }

    /// Marks the update as a major upgrade.
    pub fn with_major(mut self, major: bool) -> Self {
        self.major = major;
        self
    }
}

/// Why Sparkle found no applicable update (`SPUNoUpdateFoundReason`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NoUpdateReason {
    /// Sparkle did not say, or reported a reason this crate does not know.
    #[default]
    Unknown,
    /// The running version is the newest one in the appcast.
    OnLatestVersion,
    /// The running version is newer than anything in the appcast.
    OnNewerThanLatestVersion,
    /// A newer version requires a newer macOS.
    SystemTooOld,
    /// A newer version does not support this macOS.
    SystemTooNew,
    /// A newer version requires an Apple silicon Mac.
    HardwareUnsupported,
}

/// The user's answer to Sparkle's update prompt (`SPUUserUpdateChoice`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UserChoice {
    /// Skip this version.
    Skip,
    /// Install the update.
    Install,
    /// Not now; Sparkle may remind the user later.
    Dismiss,
}

/// How far an update had progressed when the user made a choice
/// (`SPUUserUpdateStage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UpdateStage {
    /// The update has not been downloaded.
    NotDownloaded,
    /// The update is downloaded and ready to install.
    Downloaded,
    /// The update is being installed; dismissing it still installs it when
    /// the application quits.
    Installing,
    /// A stage introduced by a newer Sparkle.
    Unknown,
}

/// An `NSError` reported by Sparkle.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SparkleError {
    /// The error domain, for example [`SparkleError::SPARKLE_DOMAIN`].
    pub domain: String,
    /// The error code within its domain.
    pub code: i64,
    /// Sparkle's localized description.
    pub message: String,
    /// The localized failure reason, when present.
    pub failure_reason: Option<String>,
}

impl SparkleError {
    /// Sparkle's own error domain (`SUSparkleErrorDomain`).
    pub const SPARKLE_DOMAIN: &'static str = "SUSparkleErrorDomain";

    /// `SUNoUpdateError`: the check completed and found nothing to install.
    pub const NO_UPDATE: i64 = 1001;

    /// An error in `domain` with `code` and a localized `message`.
    pub fn new(domain: impl Into<String>, code: i64, message: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            code,
            message: message.into(),
            failure_reason: None,
        }
    }

    /// An error in Sparkle's own domain.
    pub fn sparkle(code: i64, message: impl Into<String>) -> Self {
        Self::new(Self::SPARKLE_DOMAIN, code, message)
    }

    /// Sets the localized failure reason.
    pub fn with_failure_reason(mut self, reason: impl Into<String>) -> Self {
        self.failure_reason = Some(reason.into());
        self
    }

    /// Whether this is Sparkle's "no update found" outcome rather than a
    /// failure.
    pub fn is_no_update(&self) -> bool {
        self.domain == Self::SPARKLE_DOMAIN && self.code == Self::NO_UPDATE
    }
}
