//! Optional neutral GPUI controls for `gpui-auto-update`.
//!
//! These reference controls (update available, progress, automatic-updates
//! setting, check for updates, restart to update, and error or up-to-date
//! feedback) inherit the host application's theme and depend only on GPUI,
//! not on a third-party design system. Applications can replace them
//! entirely by rendering their own UI from the facade's observable state.

#![forbid(unsafe_code)]

mod controls;
mod summary;

pub use controls::{ControlTheme, UpdateControls, UpdateIndicator, set_theme};
pub use summary::{Tone, UpdateAction, UpdateProgress, UpdateSummary};
