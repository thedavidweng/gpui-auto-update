//! The Sparkle backend: checks, preferences, channels, and lifecycle
//! tracking on top of a [`SparkleEngine`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use gpui_auto_update_core::{
    Capability, Channel, CheckKind, CheckOutcome, CheckRequest, CheckSource, ErrorKind,
    PreferenceOwner, PreferenceStore, UpdateCoordinator, UpdateError, UpdatePreferences,
};

use crate::engine::SparkleEngine;
use crate::event::{SparkleEvent, UserChoice};
use crate::hub::{EventStream, SparkleEvents};
use crate::mapping::{self, Phase};

/// How long a check waits for Sparkle to resolve it by default.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(120);

/// The macOS backend: Sparkle 2 performs every update step, and this type
/// translates its lifecycle into the core's
/// [`UpdateState`](gpui_auto_update_core::UpdateState).
///
/// - As a [`CheckSource`], a manual check is presented with Sparkle's
///   standard UI and a background check uses Sparkle's background check;
///   both return what Sparkle found.
/// - [`Self::preferences`] is a [`PreferenceStore`] owned by Sparkle: the
///   automatic-check preference is read from and written to Sparkle, which
///   persists it and runs its own check schedule.
/// - [`Self::attach`] mirrors Sparkle-driven downloads, installs, and
///   relaunches into a coordinator's state.
///
/// Sparkle installs and relaunches the application itself, so the facade
/// hands off with `Handoff::BackendOwned`.
///
/// Cloning is cheap and every clone refers to the same Sparkle updater.
#[derive(Clone)]
pub struct SparkleBackend {
    engine: Option<Arc<dyn SparkleEngine>>,
    events: SparkleEvents,
    check_timeout: Duration,
    attached: Arc<AtomicBool>,
}

impl SparkleBackend {
    /// A backend driving `engine`, which reports Sparkle's notifications
    /// through `events`.
    pub fn new(engine: impl SparkleEngine, events: SparkleEvents) -> Self {
        Self {
            engine: Some(Arc::new(engine)),
            events,
            check_timeout: DEFAULT_CHECK_TIMEOUT,
            attached: Arc::default(),
        }
    }

    /// A backend for an installation Sparkle cannot update, for example an
    /// executable that is not inside an application bundle. Its capability
    /// is [`Capability::Unsupported`] and every operation fails.
    pub fn unsupported() -> Self {
        Self {
            engine: None,
            events: SparkleEvents::new(),
            check_timeout: DEFAULT_CHECK_TIMEOUT,
            attached: Arc::default(),
        }
    }

    /// Sets how long a check waits for Sparkle's answer before failing with
    /// [`ErrorKind::TemporarilyUnavailable`].
    pub fn with_check_timeout(mut self, timeout: Duration) -> Self {
        self.check_timeout = timeout;
        self
    }

    /// The channel Sparkle's notifications arrive on.
    pub fn events(&self) -> &SparkleEvents {
        &self.events
    }

    /// Whether this installation may update itself: self-managed whenever a
    /// Sparkle updater is running. Sparkle decides by itself whether it can
    /// write to the bundle and asks for authorization when needed.
    pub fn capability(&self) -> Capability {
        if self.engine.is_some() {
            Capability::SelfManaged
        } else {
            Capability::Unsupported
        }
    }

    /// The automatic-update preference store, owned by Sparkle.
    pub fn preferences(&self) -> SparklePreferences {
        SparklePreferences {
            engine: self.engine.clone(),
        }
    }

    /// Mirrors Sparkle's lifecycle (downloads, installs, relaunches, and
    /// choices the user makes in Sparkle's windows) into `coordinator`'s
    /// state from now on. Only the first call has an effect.
    ///
    /// Events are applied on a dedicated thread so that Sparkle's main
    /// thread never waits on the coordinator or its observers.
    pub fn attach(&self, coordinator: &UpdateCoordinator) {
        if self.engine.is_none() || self.attached.swap(true, Ordering::SeqCst) {
            return;
        }
        let stream = self.events.subscribe();
        let coordinator = coordinator.clone();
        let spawned = thread::Builder::new()
            .name("gpui-auto-update-sparkle".into())
            .spawn(move || {
                while let Some(event) = stream.recv() {
                    if let Some(phase) = mapping::lifecycle_phase(&event) {
                        apply_phase(&coordinator, &phase);
                    }
                }
            });
        if let Err(error) = spawned {
            self.attached.store(false, Ordering::SeqCst);
            tracing::error!("could not start the Sparkle state tracker: {error}");
        }
    }

    /// Presents the available update with Sparkle's standard UI and waits
    /// while the user installs it there, mirroring progress into
    /// `coordinator`.
    ///
    /// Returns once Sparkle has downloaded and validated the update (or
    /// went further and started installing it). Fails with Sparkle's error,
    /// or with [`ErrorKind::InvalidState`] when the user skipped or
    /// postponed the update. Blocks until then, so call it off the main
    /// thread.
    pub fn stage(&self, coordinator: &UpdateCoordinator) -> Result<(), UpdateError> {
        let engine = self.engine()?;
        let stream = self.events.subscribe();
        engine.check_for_updates()?;
        let mut install_chosen = false;
        while let Some(event) = stream.recv() {
            if let Some(phase) = mapping::lifecycle_phase(&event) {
                apply_phase(coordinator, &phase);
                match phase {
                    Phase::Staged
                    | Phase::Installing
                    | Phase::WaitingForQuit
                    | Phase::Relaunching => return Ok(()),
                    Phase::Failed(error) => return Err(error),
                    Phase::Dismissed => return Err(postponed()),
                    Phase::Downloading | Phase::Verifying => {}
                }
            }
            match event {
                SparkleEvent::UserChoice {
                    choice: UserChoice::Install,
                    ..
                } => install_chosen = true,
                SparkleEvent::NoUpdateFound(_) => return Err(postponed()),
                SparkleEvent::CycleFinished { error } if !install_chosen => {
                    return Err(error.map_or_else(postponed, |e| mapping::update_error(&e)));
                }
                _ => {}
            }
        }
        Err(gone())
    }

    /// Brings Sparkle's prompt for the staged update forward so that
    /// Sparkle installs it and relaunches the application.
    pub fn install(&self) -> Result<(), UpdateError> {
        self.engine()?.check_for_updates()
    }

    /// Brings Sparkle's prompt forward for an update that is waiting for
    /// the application to quit, so that Sparkle installs it and relaunches
    /// now.
    pub fn relaunch(&self) -> Result<(), UpdateError> {
        self.engine()?.check_for_updates()
    }

    /// The channel checks select besides the default one, from Sparkle's
    /// allowed channels.
    pub fn channel(&self) -> Option<Channel> {
        let channels = self.engine.as_ref()?.allowed_channels();
        match channels {
            Ok(channels) => channels?.into_iter().next().map(Channel::new),
            Err(error) => {
                tracing::warn!(
                    diagnostic = error.diagnostic(),
                    "could not read Sparkle's channels: {error}"
                );
                None
            }
        }
    }

    /// Lets checks select `channel` in addition to the default channel, or
    /// only the default channel for `None`.
    ///
    /// Sparkle does not persist allowed channels; set the channel again on
    /// every launch.
    pub fn set_channel(&self, channel: Option<Channel>) -> Result<(), UpdateError> {
        self.engine()?
            .set_allowed_channels(channel.map(|c| vec![c.as_str().to_owned()]))
    }

    fn engine(&self) -> Result<&Arc<dyn SparkleEngine>, UpdateError> {
        self.engine.as_ref().ok_or_else(unsupported)
    }

    fn await_check(&self, stream: &EventStream) -> Result<CheckOutcome, UpdateError> {
        let deadline = Instant::now() + self.check_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match stream.recv_timeout(remaining) {
                Ok(event) => {
                    if let Some(result) = mapping::check_resolution(&event) {
                        return result;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(UpdateError::new(ErrorKind::TemporarilyUnavailable)
                        .with_diagnostic(format!(
                            "Sparkle did not finish the update check within {:?}",
                            self.check_timeout
                        )));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(gone()),
            }
        }
    }
}

impl CheckSource for SparkleBackend {
    fn check(&self, request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        let engine = self.engine()?;
        let stream = self.events.subscribe();
        if engine.session_in_progress()? {
            if let Some(update) = self.events.pending_update() {
                if request.kind == CheckKind::Manual {
                    engine.check_for_updates()?;
                }
                return Ok(CheckOutcome::UpdateAvailable(mapping::available_update(
                    &update,
                )));
            }
        }
        match request.kind {
            CheckKind::Manual => engine.check_for_updates()?,
            CheckKind::Background => engine.check_for_updates_in_background()?,
        }
        self.await_check(&stream)
    }
}

impl std::fmt::Debug for SparkleBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparkleBackend")
            .field("capability", &self.capability())
            .field("check_timeout", &self.check_timeout)
            .field("attached", &self.attached.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

/// The automatic-update preference as Sparkle stores it
/// ([`PreferenceOwner::Backend`]).
///
/// Loading reads Sparkle's `automaticallyChecksForUpdates` and
/// `lastUpdateCheckDate` every time, so changes made through Sparkle's own
/// UI (such as its first-launch permission prompt) are mirrored. Saving
/// writes only the automatic-check preference, and only when it changed;
/// Sparkle records check times itself.
#[derive(Clone)]
pub struct SparklePreferences {
    engine: Option<Arc<dyn SparkleEngine>>,
}

impl PreferenceStore for SparklePreferences {
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError> {
        let Some(engine) = &self.engine else {
            return Ok(None);
        };
        let automatic = engine
            .automatically_checks_for_updates()
            .map_err(preference_error)?;
        let last_check = engine.last_update_check().map_err(preference_error)?;
        Ok(Some(
            UpdatePreferences::new(automatic).with_last_check(last_check),
        ))
    }

    fn save(&self, preferences: &UpdatePreferences) -> Result<(), UpdateError> {
        let engine = self.engine.as_ref().ok_or_else(|| {
            UpdateError::new(ErrorKind::Preferences)
                .with_diagnostic("no Sparkle updater is running")
        })?;
        let current = engine
            .automatically_checks_for_updates()
            .map_err(preference_error)?;
        if current != preferences.automatic_checks {
            engine
                .set_automatically_checks_for_updates(preferences.automatic_checks)
                .map_err(preference_error)?;
        }
        Ok(())
    }

    fn owner(&self) -> PreferenceOwner {
        PreferenceOwner::Backend
    }
}

impl std::fmt::Debug for SparklePreferences {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparklePreferences")
            .field("running", &self.engine.is_some())
            .finish()
    }
}

fn apply_phase(coordinator: &UpdateCoordinator, phase: &Phase) {
    for event in mapping::catch_up(&coordinator.state(), phase) {
        if let Err(error) = coordinator.apply(event.clone()) {
            tracing::debug!(?event, kind_of_error = ?error.kind(), "Sparkle step does not fit the update state");
            break;
        }
    }
}

fn preference_error(error: UpdateError) -> UpdateError {
    if error.kind() == ErrorKind::Preferences {
        return error;
    }
    let diagnostic = error.diagnostic().unwrap_or(error.message()).to_owned();
    UpdateError::new(ErrorKind::Preferences)
        .with_diagnostic(format!("Sparkle preferences: {diagnostic}"))
        .with_source(error)
}

fn postponed() -> UpdateError {
    UpdateError::new(ErrorKind::InvalidState)
        .with_message("The update was not installed.")
        .with_diagnostic("the Sparkle update session ended before the update was ready to install")
}

fn unsupported() -> UpdateError {
    UpdateError::new(ErrorKind::UnsupportedInstallation)
        .with_diagnostic("no Sparkle updater is running for this installation")
}

fn gone() -> UpdateError {
    UpdateError::new(ErrorKind::Internal).with_diagnostic("the Sparkle updater was shut down")
}
