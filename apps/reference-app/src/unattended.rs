//! Unattended mode for end-to-end tests (`REFERENCE_APP_E2E_REPORT`).
//!
//! An unattended build checks for updates as soon as the updater is ready
//! and installs whatever is offered without waiting for a click, so a test
//! can drive version N to N+1 on a CI runner. Everything it observes is
//! appended, one line per fact, to the report file:
//!
//! | Line | Meaning |
//! | --- | --- |
//! | `started <version>` | The updater of this build is ready |
//! | `previous-update-failure <kind>: <message>` | The previous update failed after the app quit, for example it was rolled back; nothing is installed on this start |
//! | `update-available <version>` | The check offered this version; it is being installed |
//! | `up-to-date` | The check found nothing newer |
//! | `error <kind>: <message> [(<diagnostic>)]` | A check or another operation failed |
//! | `handoff` | The app is ending so the update can be finished. Under Sparkle the update is installed by Sparkle when the app quits, so this line is written right before unattended mode quits the app itself |
//! | `failed-to-start <version>` | A build made with `REFERENCE_APP_E2E_FAIL_TO_START` ran and exited |
//!
//! Under Sparkle (the `sparkle` feature on macOS, see [`Route::Sparkle`]) a
//! manual check is presented by Sparkle's own standard window, which nobody
//! can click on an unattended run. Unattended mode therefore runs a
//! background check instead and leaves the installation to Sparkle's silent
//! automatic update, which waits for the app to quit.
//!
//! The app keeps running after `up-to-date` or `error`; the test ends it.
//! Installers may append their own lines to the same report, which is how a
//! test sees the order in which the old version, the installer, and the new
//! version ran.

use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::rc::Rc;

use gpui::{App, AppContext as _, Entity, Subscription};
use gpui_auto_update::core::{CheckKind, CheckOutcome, UpdateError, UpdateState};
use gpui_auto_update::{Updater, UpdaterEvent};

/// How the build's backend wants an unattended update driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// A manual check, then stage and install the offered update.
    #[cfg_attr(all(target_os = "macos", feature = "sparkle"), allow(dead_code))]
    Manual,
    /// Sparkle: a background check; Sparkle downloads the update and
    /// installs it when the app quits, which unattended mode then causes.
    Sparkle,
}

/// Keeps unattended mode running; drop it to stop.
pub struct Unattended {
    _events: Subscription,
    _state: Subscription,
}

impl gpui::Global for Unattended {}

impl Unattended {
    /// Drives `updater` of the build `version` unattended, reporting to
    /// `report`. Call it right after creating the updater, before it is
    /// ready.
    pub fn start(
        updater: &Entity<Updater>,
        report: &Path,
        version: &str,
        route: Route,
        cx: &mut App,
    ) -> Self {
        let report = report.to_path_buf();
        let version = version.to_owned();
        // Set once this mode's own check offered an update, so that only an
        // update it saw being offered is installed.
        let installing = Rc::new(Cell::new(false));
        let events = {
            let report = report.clone();
            let installing = installing.clone();
            cx.subscribe(updater, move |updater, event, cx| match event {
                UpdaterEvent::Ready => {
                    append(&report, &format!("started {version}"));
                    // The failure, if any, is already set when `Ready` is
                    // delivered; it is reported by its own event below.
                    if updater.read(cx).previous_update_failure().is_some() {
                        return;
                    }
                    match route {
                        Route::Manual => updater.update(cx, |updater, cx| {
                            updater.check_for_updates(cx);
                        }),
                        Route::Sparkle => {
                            let coordinator = updater.read(cx).coordinator().clone();
                            let report = report.clone();
                            cx.background_spawn(async move {
                                match coordinator.check(CheckKind::Background) {
                                    Ok(CheckOutcome::UpToDate) => append(&report, "up-to-date"),
                                    Ok(CheckOutcome::UpdateAvailable(update)) => append(
                                        &report,
                                        &format!("update-available {}", update.version),
                                    ),
                                    Err(error) => {
                                        append(&report, &format!("error {}", describe(&error)));
                                    }
                                }
                            })
                            .detach();
                        }
                    }
                }
                UpdaterEvent::PreviousUpdateFailed(error) => {
                    append(
                        &report,
                        &format!("previous-update-failure {}", describe(error)),
                    );
                }
                UpdaterEvent::CheckFinished {
                    kind: CheckKind::Manual,
                    result,
                } => match result {
                    Ok(CheckOutcome::UpToDate) => append(&report, "up-to-date"),
                    Ok(CheckOutcome::UpdateAvailable(update)) => {
                        append(&report, &format!("update-available {}", update.version));
                        installing.set(true);
                        install_when_possible(&updater, &report, &installing, cx);
                    }
                    Err(error) => append(&report, &format!("error {}", describe(error))),
                },
                UpdaterEvent::Failed(error) => {
                    append(&report, &format!("error {}", describe(error)));
                }
                UpdaterEvent::Handoff(_) => append(&report, "handoff"),
                _ => {}
            })
        };
        let quitting = Cell::new(false);
        let state = cx.observe(updater, move |updater, cx| {
            if route == Route::Sparkle {
                if matches!(updater.read(cx).state(), UpdateState::WaitingForQuit(_))
                    && !quitting.replace(true)
                {
                    append(&report, "handoff");
                    cx.quit();
                }
                return;
            }
            if installing.get() {
                install_when_possible(&updater, &report, &installing, cx);
            }
        });
        Self {
            _events: events,
            _state: state,
        }
    }
}

/// Stages an available update, or installs a staged one, unless something
/// is already running; the observer calls again when that finishes. A
/// request the updater refuses is reported and ends the attempt; failures of
/// the operation itself arrive as `UpdaterEvent::Failed`.
fn install_when_possible(
    updater: &Entity<Updater>,
    report: &Path,
    installing: &Cell<bool>,
    cx: &mut App,
) {
    let requested = updater.update(cx, |updater, cx| {
        let installable = matches!(
            updater.state(),
            UpdateState::Available(_) | UpdateState::Staged(_)
        );
        if installable && !updater.is_busy() {
            updater.request_install(cx)
        } else {
            Ok(())
        }
    });
    if let Err(error) = requested {
        installing.set(false);
        append(report, &format!("error {}", describe(&error)));
    }
}

/// Records that a deliberately broken build (`REFERENCE_APP_E2E_FAIL_TO_START`)
/// ran before it exits.
pub fn record_failed_start(report: &Path, version: &str) {
    append(report, &format!("failed-to-start {version}"));
}

fn describe(error: &UpdateError) -> String {
    let mut line = format!("{:?}: {}", error.kind(), error.message());
    if let Some(diagnostic) = error.diagnostic() {
        line.push_str(&format!(" ({diagnostic})"));
    }
    line
}

fn append(report: &Path, line: &str) {
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(report)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(error) = result {
        eprintln!(
            "reference-app: cannot write the report {}: {error}",
            report.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use gpui::{Entity, TestAppContext};
    use gpui_auto_update::core::ErrorKind;
    use gpui_auto_update::core::{
        AvailableUpdate, Capability, CheckOutcome, CheckRequest, CheckSource,
        MemoryPreferenceStore, UpdateError,
    };
    use gpui_auto_update::{Handoff, ProgressSink, UpdateBackend, Updater, UpdaterConfig};

    use super::*;

    /// A feed that offers this version, if any, on every check.
    struct Feed(Option<String>);

    impl CheckSource for Feed {
        fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
            Ok(match &self.0 {
                Some(version) => CheckOutcome::UpdateAvailable(AvailableUpdate::new(version)),
                None => CheckOutcome::UpToDate,
            })
        }
    }

    #[derive(Clone, Default)]
    struct Backend {
        calls: Arc<Mutex<Vec<String>>>,
        stage_failure: Arc<Mutex<Option<UpdateError>>>,
        previous_failure: Arc<Mutex<Option<UpdateError>>>,
    }

    impl UpdateBackend for Backend {
        fn capability(&self) -> Capability {
            Capability::SelfManaged
        }
        fn stage(
            &self,
            update: &AvailableUpdate,
            progress: &ProgressSink,
        ) -> Result<(), UpdateError> {
            progress.download_started(None);
            progress.verification_started();
            self.calls
                .lock()
                .unwrap()
                .push(format!("stage {}", update.version));
            self.stage_failure
                .lock()
                .unwrap()
                .clone()
                .map_or(Ok(()), Err)
        }
        fn install(
            &self,
            update: &AvailableUpdate,
            _: &ProgressSink,
        ) -> Result<Handoff, UpdateError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("install {}", update.version));
            Ok(Handoff::BackendOwned)
        }
        fn take_previous_failure(&self) -> Option<UpdateError> {
            self.previous_failure.lock().unwrap().take()
        }
    }

    fn start(
        cx: &mut TestAppContext,
        offer: Option<&str>,
        backend: &Backend,
        report: &Path,
    ) -> (Entity<Updater>, Unattended) {
        let config = UpdaterConfig::new("dev.example.unattended", Feed(offer.map(str::to_owned)))
            .with_backend(backend.clone())
            .with_preferences(MemoryPreferenceStore::new())
            .allow_debug_self_update(true);
        let updater = cx.new(|cx| Updater::new(config, cx));
        let unattended =
            cx.update(|cx| Unattended::start(&updater, report, "1.0.0", Route::Manual, cx));
        cx.run_until_parked();
        (updater, unattended)
    }

    fn lines(report: &Path) -> Vec<String> {
        std::fs::read_to_string(report)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[gpui::test]
    fn an_unattended_build_installs_the_offered_update(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.log");
        let backend = Backend::default();
        let _running = start(cx, Some("1.1.0"), &backend, &report);

        assert_eq!(
            *backend.calls.lock().unwrap(),
            vec!["stage 1.1.0".to_owned(), "install 1.1.0".to_owned()]
        );
        let lines = lines(&report);
        assert_eq!(lines.first().map(String::as_str), Some("started 1.0.0"));
        assert!(lines.contains(&"update-available 1.1.0".to_owned()));
        assert_eq!(lines.last().map(String::as_str), Some("handoff"));
    }

    #[gpui::test]
    fn an_unattended_build_that_is_up_to_date_says_so(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.log");
        let backend = Backend::default();
        let _running = start(cx, None, &backend, &report);

        assert!(backend.calls.lock().unwrap().is_empty());
        assert_eq!(lines(&report), vec!["started 1.0.0", "up-to-date"]);
    }

    #[gpui::test]
    fn after_a_rollback_the_failure_is_reported_and_not_retried(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.log");
        let backend = Backend::default();
        *backend.previous_failure.lock().unwrap() = Some(
            UpdateError::new(ErrorKind::HealthConfirmation)
                .with_message("Version 1.2.0 did not start, so 1.0.0 was restored."),
        );
        let _running = start(cx, Some("1.2.0"), &backend, &report);

        assert!(backend.calls.lock().unwrap().is_empty());
        assert_eq!(
            lines(&report),
            vec![
                "started 1.0.0",
                "previous-update-failure HealthConfirmation: Version 1.2.0 did not start, so 1.0.0 was restored.",
            ]
        );
    }

    #[gpui::test]
    fn a_failed_update_is_reported_and_nothing_is_installed(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.log");
        let backend = Backend::default();
        *backend.stage_failure.lock().unwrap() = Some(
            UpdateError::new(ErrorKind::Signature)
                .with_message("The signature is invalid.")
                .with_diagnostic("signature does not verify"),
        );
        let _running = start(cx, Some("1.1.0"), &backend, &report);

        assert_eq!(
            *backend.calls.lock().unwrap(),
            vec!["stage 1.1.0".to_owned()]
        );
        assert_eq!(
            lines(&report),
            vec![
                "started 1.0.0",
                "update-available 1.1.0",
                "error Signature: The signature is invalid. (signature does not verify)",
            ]
        );
    }

    #[test]
    fn a_broken_build_records_that_it_ran() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.log");
        record_failed_start(&report, "1.2.0");
        assert_eq!(lines(&report), vec!["failed-to-start 1.2.0"]);
    }
}
