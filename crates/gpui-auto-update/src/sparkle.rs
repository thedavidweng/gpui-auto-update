//! The Sparkle backend on macOS: its `UpdateBackend` implementation and
//! configuration constructors.

use gpui_auto_update_core::{
    AvailableUpdate, Capability, Channel, CheckPolicy, UpdateCoordinator, UpdateError,
};
use gpui_auto_update_macos::{RelaunchContinuation, SparkleBackend};

use crate::backend::{Handoff, HandoffGate, PostponedHandoff, ProgressSink, UpdateBackend};
use crate::config::UpdaterConfig;

/// Sparkle's postponed relaunch resumes through the facade's gate.
impl PostponedHandoff for RelaunchContinuation {
    fn resume(self: Box<Self>) {
        (*self).resume();
    }
}

/// Sparkle installs and relaunches the application itself, so every
/// handoff is [`Handoff::BackendOwned`].
impl UpdateBackend for SparkleBackend {
    fn capability(&self) -> Capability {
        SparkleBackend::capability(self)
    }

    fn attach(&self, coordinator: &UpdateCoordinator) {
        SparkleBackend::attach(self, coordinator);
    }

    fn stage(&self, _: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError> {
        SparkleBackend::stage(self, progress.coordinator())
    }

    fn install(&self, _: &AvailableUpdate, _: &ProgressSink) -> Result<Handoff, UpdateError> {
        SparkleBackend::install(self)?;
        Ok(Handoff::BackendOwned)
    }

    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        SparkleBackend::relaunch(self)?;
        Ok(Handoff::BackendOwned)
    }

    fn set_handoff_gate(&self, gate: HandoffGate) {
        SparkleBackend::set_handoff_gate(self, move |continuation| {
            gate(Box::new(continuation));
        });
    }

    fn channel(&self) -> Option<Channel> {
        SparkleBackend::channel(self)
    }

    fn set_channel(&self, channel: Option<Channel>) -> Result<(), UpdateError> {
        SparkleBackend::set_channel(self, channel)
    }
}

impl UpdaterConfig {
    /// A configuration in which `backend` (Sparkle) does everything: it
    /// resolves checks, presents manual checks with Sparkle's standard UI,
    /// owns the automatic-update preference and its schedule, and installs
    /// and relaunches the application.
    ///
    /// The policy leaves launch and periodic checks to Sparkle's own
    /// scheduler; the facade only mirrors Sparkle's state and preference.
    pub fn for_sparkle(app_id: impl Into<String>, backend: SparkleBackend) -> Self {
        Self::new(app_id, backend.clone())
            .with_preferences(backend.preferences())
            .with_backend(backend)
            .with_policy(
                CheckPolicy::recommended()
                    .with_check_on_launch(false)
                    .with_check_when_enabled(false)
                    .with_periodic_interval(None),
            )
    }

    /// The default macOS configuration: starts Sparkle for the running
    /// application bundle and uses it as in [`Self::for_sparkle`].
    ///
    /// Call it on the main thread, for example inside GPUI's
    /// `Application::run`. Outside an application bundle the updater
    /// reports an unsupported installation. Fails when Sparkle cannot start,
    /// for example because `SUFeedURL` is missing from `Info.plist`.
    ///
    /// Sparkle's scheduled discoveries never open a Sparkle window. To
    /// override when Sparkle may present, start the backend with
    /// [`gpui_auto_update_macos::SparkleBackend::start_with_policy`] and
    /// pass it to [`Self::for_sparkle`].
    ///
    /// Available with the `sparkle` feature, which links
    /// `Sparkle.framework`; see the crate documentation.
    #[cfg(feature = "sparkle")]
    pub fn sparkle(app_id: impl Into<String>) -> Result<Self, UpdateError> {
        Ok(Self::for_sparkle(app_id, SparkleBackend::start()?))
    }
}
