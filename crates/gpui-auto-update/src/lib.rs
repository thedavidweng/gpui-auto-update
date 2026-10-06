//! Sparkle-quality automatic updates for GPUI.
//!
//! `gpui-auto-update` is the crate GPUI applications depend on. It exposes
//! the updater as an observable GPUI entity, [`Updater`], with standard
//! actions, and runs all network and filesystem work on GPUI's background
//! executor. Every state change is applied on the foreground thread and
//! notifies observers, so update UI follows ordinary GPUI patterns.
//!
//! ```no_run
//! use gpui::{App, Application};
//! use gpui_auto_update::{UpdaterConfig, core::{CheckOutcome, CheckRequest, CheckSource, UpdateError}};
//!
//! struct MyFeed;
//! impl CheckSource for MyFeed {
//!     fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
//!         Ok(CheckOutcome::UpToDate)
//!     }
//! }
//!
//! Application::new().run(|cx: &mut App| {
//!     let updater = gpui_auto_update::init(UpdaterConfig::new("com.example.App", MyFeed), cx);
//!     // Render from `updater.read(cx).state()` and dispatch `CheckForUpdates`
//!     // from a menu item.
//! # let _ = updater;
//! });
//! ```
//!
//! # Pieces
//!
//! - [`UpdaterConfig`] collects the check source (from the core crate), the
//!   platform [`UpdateBackend`] that stages and installs updates, the
//!   automatic-check policy, and where preferences live.
//! - [`init`] creates the [`Updater`], keeps it alive for the application's
//!   lifetime, and registers the [`CheckForUpdates`], [`InstallUpdate`],
//!   [`RestartToUpdate`], and [`DismissUpdate`] actions.
//! - [`Updater::on_prepare_to_install`] registers the asynchronous save hook
//!   that runs before the updater quits or restarts the application.
//! - [`PreviewState`] provides deterministic, clearly marked states for
//!   building update UI without a real update.
//!
//! # Debug builds
//!
//! Debug builds check for updates but never install them unless
//! [`UpdaterConfig::allow_debug_self_update`] is set, so running from a
//! development checkout cannot replace it with a production release.
//!
//! The framework-independent contracts are re-exported as [`core`].

#![forbid(unsafe_code)]

mod backend;
mod config;
mod paths;
mod preview;
mod updater;

pub use gpui_auto_update_core as core;

pub use backend::{Handoff, ProgressSink, UnsupportedBackend, UpdateBackend};
pub use config::{BuildProfile, UpdaterConfig};
pub use paths::default_preferences_path;
pub use preview::{PREVIEW_CHANNEL, PREVIEW_VERSION, PreviewState};
pub use updater::{PrepareError, Updater, UpdaterEvent};

use gpui::{App, AppContext as _, Entity, Global};

gpui::actions!(
    auto_update,
    [
        /// Runs a manual update check with visible feedback.
        CheckForUpdates,
        /// Downloads and stages the available update, or installs the
        /// staged one.
        InstallUpdate,
        /// Installs the staged update and restarts the application.
        RestartToUpdate,
        /// Dismisses the current update outcome.
        DismissUpdate,
    ]
);

struct GlobalUpdater(Entity<Updater>);

impl Global for GlobalUpdater {}

/// Creates the application's [`Updater`], stores it in a GPUI global so it
/// lives as long as the application, and registers the standard actions.
///
/// Calling `init` again replaces the global updater; the actions then act on
/// the new one.
pub fn init(config: UpdaterConfig, cx: &mut App) -> Entity<Updater> {
    let updater = cx.new(|cx| Updater::new(config, cx));
    let first = !cx.has_global::<GlobalUpdater>();
    cx.set_global(GlobalUpdater(updater.clone()));
    if first {
        register_actions(cx);
    }
    updater
}

fn global(cx: &App) -> Option<Entity<Updater>> {
    cx.try_global::<GlobalUpdater>()
        .map(|global| global.0.clone())
}

fn register_actions(cx: &mut App) {
    cx.on_action(|_: &CheckForUpdates, cx| {
        if let Some(updater) = global(cx) {
            updater.update(cx, |updater, cx| updater.check_for_updates(cx));
        }
    });
    cx.on_action(|_: &InstallUpdate, cx| {
        if let Some(updater) = global(cx) {
            updater.update(cx, |updater, cx| {
                if let Err(error) = updater.request_install(cx) {
                    updater.report(error, cx);
                }
            });
        }
    });
    cx.on_action(|_: &RestartToUpdate, cx| {
        if let Some(updater) = global(cx) {
            updater.update(cx, |updater, cx| {
                if let Err(error) = updater.restart_to_update(cx) {
                    updater.report(error, cx);
                }
            });
        }
    });
    cx.on_action(|_: &DismissUpdate, cx| {
        if let Some(updater) = global(cx) {
            updater.update(cx, |updater, cx| {
                if let Err(error) = updater.dismiss(cx) {
                    updater.report(error, cx);
                }
            });
        }
    });
}
