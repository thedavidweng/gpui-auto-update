//! How Sparkle presents update sessions the user did not ask for.

use crate::event::{SessionState, SparkleUpdate};

/// How Sparkle presents a scheduled (not user-initiated) update session:
/// Sparkle's gentle reminders hooks, with the framework's types translated.
///
/// Installed with
/// [`SparkleBackend::start_with_policy`](crate::SparkleBackend); the default
/// is [`GpuiPresentation`]. Manual checks always use Sparkle's standard UI;
/// this policy governs only sessions the user did not ask for. Sparkle
/// calls every method on the main thread in the middle of its update
/// cycle, so implementations must return quickly and report through their
/// own state rather than drive Sparkle from here.
///
/// Sparkle only consults the policy at all while one is installed, and the
/// backend always installs one.
pub trait PresentationPolicy: Send + Sync + 'static {
    /// Whether Sparkle's own window may present the scheduled update
    /// (`standardUserDriverShouldHandleShowingScheduledUpdate:andInImmediateFocus:`).
    ///
    /// Returning `false` keeps the discovery in the application: the
    /// backend retains it in the update state, and a later manual check
    /// brings Sparkle's standard dialog forward. `immediate_focus` is true
    /// when Sparkle wants to present the update right away, for example
    /// for a critical update or an impatient reminder. Keep the decision
    /// free of side effects; Sparkle may ask more than once per session.
    fn should_show_scheduled_update(&self, update: &SparkleUpdate, immediate_focus: bool) -> bool;

    /// The session's update is about to be presented — by Sparkle
    /// (`handled_by_sparkle`) or left to the application
    /// (`standardUserDriverWillHandleShowingUpdate:forUpdate:state:`).
    fn will_show_update(
        &self,
        handled_by_sparkle: bool,
        update: &SparkleUpdate,
        session: SessionState,
    );

    /// The user interacted with a reminder the application presented;
    /// clear attention indicators such as badges
    /// (`standardUserDriverDidReceiveUserAttentionForUpdate:`).
    fn did_receive_user_attention(&self, update: &SparkleUpdate) {
        let _ = update;
    }

    /// The update session ended; remove any reminder UI
    /// (`standardUserDriverWillFinishUpdateSession`).
    fn will_finish_update_session(&self) {}
}

/// The default presentation policy: a scheduled discovery never opens a
/// Sparkle window. The backend surfaces it in the update state (the
/// facade's `Available` state) instead, and a manual check presents
/// Sparkle's standard UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct GpuiPresentation;

impl PresentationPolicy for GpuiPresentation {
    fn should_show_scheduled_update(&self, _: &SparkleUpdate, _: bool) -> bool {
        false
    }

    fn will_show_update(&self, _: bool, _: &SparkleUpdate, _: SessionState) {}
}
