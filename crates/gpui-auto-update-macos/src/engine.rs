//! The operations the backend needs from Sparkle's `SPUUpdater`.

use std::time::SystemTime;

use gpui_auto_update_core::UpdateError;

/// The Sparkle updater operations the backend uses.
///
/// The real implementation (the `sparkle` feature) wraps Sparkle 2's
/// `SPUStandardUpdaterController` and `SPUUpdater`, hops to the main thread
/// for every call, and reports Sparkle's delegate notifications through the
/// [`SparkleEvents`](crate::SparkleEvents) it was created with. Other
/// implementations exist for tests and must behave the same way: events that
/// a call causes are published to that channel, possibly before the call
/// returns.
///
/// Every method may be called from any thread, so implementations must not
/// assume they run on the main thread.
pub trait SparkleEngine: Send + Sync + 'static {
    /// Starts a user-initiated check presented with Sparkle's standard UI
    /// (`-[SPUStandardUpdaterController checkForUpdates:]`). When an update
    /// session is already running, Sparkle brings its window forward
    /// instead.
    fn check_for_updates(&self) -> Result<(), UpdateError>;

    /// Starts a check that shows UI only if an update is found
    /// (`-[SPUUpdater checkForUpdatesInBackground]`).
    fn check_for_updates_in_background(&self) -> Result<(), UpdateError>;

    /// Whether an update session (check, download, or install prompt) is
    /// running (`sessionInProgress`).
    fn session_in_progress(&self) -> Result<bool, UpdateError>;

    /// Sparkle's persisted automatic-check preference
    /// (`automaticallyChecksForUpdates`).
    fn automatically_checks_for_updates(&self) -> Result<bool, UpdateError>;

    /// Changes Sparkle's persisted automatic-check preference.
    fn set_automatically_checks_for_updates(&self, enabled: bool) -> Result<(), UpdateError>;

    /// When Sparkle last checked (`lastUpdateCheckDate`).
    fn last_update_check(&self) -> Result<Option<SystemTime>, UpdateError>;

    /// The channels checks may select besides the default channel
    /// (`allowedChannelsForUpdater:`); `None` is only the default channel.
    fn allowed_channels(&self) -> Result<Option<Vec<String>>, UpdateError>;

    /// Changes the channels checks may select.
    fn set_allowed_channels(&self, channels: Option<Vec<String>>) -> Result<(), UpdateError>;
}
