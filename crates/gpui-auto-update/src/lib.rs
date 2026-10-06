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
//! - [`UpdaterConfig::native_feed`] configures a signed native feed together
//!   with the platform's default backend (on Linux, the managed-install
//!   backend in `linux`).
//!
//! # Startup health and the Linux helper
//!
//! Applications that install from native feeds must call
//! [`run_update_helper_if_requested`] first thing in `main`, and
//! [`Updater::main_window_opened`] once the main window is shown. On Linux
//! the update helper keeps the previous version until that confirmation
//! arrives and restores it if the new version exits first; what happened is
//! reported on the next start through [`Updater::previous_update_failure`]
//! and [`UpdaterEvent::PreviousUpdateFailed`].
//!
//! # Platform backends
//!
//! ## Windows
//!
//! On Windows, `UpdaterConfig::windows` builds the default configuration
//! from a `gpui_auto_update::windows::WindowsUpdateConfig`: the declared
//! architecture's signed feed is the check source, and the Windows backend
//! stages verified installers or portable executables and hands off to them
//! (see `docs/windows-installers.md` in the repository).
//!
//! ## macOS: Sparkle
//!
//! On macOS, updates are performed by Sparkle 2 through the
//! `gpui-auto-update-macos` backend, re-exported as `macos`. With the
//! `sparkle` feature, `UpdaterConfig::sparkle(app_id)` starts Sparkle for
//! the running application bundle and selects it as the check source,
//! preference store, and backend. Manual checks then use Sparkle's standard
//! update UI, the automatic-update preference stays in Sparkle and is only
//! mirrored, and Sparkle installs and relaunches the application
//! ([`Handoff::BackendOwned`]). `UpdaterConfig::for_sparkle` does the same
//! with a backend you created yourself.
//!
//! The feature is opt-in because it links `Sparkle.framework`: building
//! requires `SPARKLE_FRAMEWORK_PATH` to point at the directory containing
//! it, and the application bundle must embed it in `Contents/Frameworks`
//! with an `@executable_path/../Frameworks` run-path (see
//! `docs/sparkle-packaging.md` in the repository).
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
#[cfg(any(target_os = "linux", all(test, unix)))]
pub mod linux;
mod native;
mod paths;
#[cfg(windows)]
mod platform_windows;
mod preview;
#[cfg(target_os = "macos")]
mod sparkle;
mod updater;

pub use gpui_auto_update_core as core;
/// The Sparkle 2 backend used on macOS.
#[cfg(target_os = "macos")]
pub use gpui_auto_update_macos as macos;
/// The Windows backend; [`UpdaterConfig::windows`] builds the default
/// Windows configuration from its [`WindowsUpdateConfig`](windows::WindowsUpdateConfig).
#[cfg(windows)]
pub use gpui_auto_update_windows as windows;

pub use backend::{
    Handoff, HandoffGate, PostponedHandoff, ProgressSink, UnsupportedBackend, UpdateBackend,
};
pub use config::{BuildProfile, UpdaterConfig};
pub use native::NativeFeed;
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

/// Finishes an update and exits the process if this process was started as
/// the Linux update helper; otherwise returns immediately.
///
/// Call it first thing in `main`, before creating the GPUI application. On
/// Linux the backend finishes an update by starting the application's own
/// executable in helper mode after the application quits; this call is
/// what runs that helper. It does nothing on other platforms and costs only
/// an argument check.
///
/// ```no_run
/// // First thing in `main`:
/// gpui_auto_update::run_update_helper_if_requested();
/// // ...then create the GPUI application as usual.
/// ```
pub fn run_update_helper_if_requested() {
    #[cfg(target_os = "linux")]
    gpui_auto_update_linux::run_helper_if_requested();
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
