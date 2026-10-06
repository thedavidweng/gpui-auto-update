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
//! | `update-available <version>` | A check offered this version; it is being installed |
//! | `up-to-date` | The check found nothing newer |
//! | `error <kind>: <message>` | A check or another operation failed |
//! | `handoff` | The app is ending so the update can be finished |
//!
//! The app keeps running after `up-to-date` or `error`; the test ends it.
//! Installers may append their own lines to the same report, which is how a
//! test sees the order in which the old version, the installer, and the new
//! version ran.

use std::cell::{Cell, RefCell};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

use gpui::{App, Entity, Subscription};
use gpui_auto_update::core::{UpdateError, UpdateState};
use gpui_auto_update::{Updater, UpdaterEvent};

/// Keeps unattended mode running; drop it to stop.
pub struct Unattended {
    _events: Subscription,
    _state: Subscription,
}

impl Unattended {
    /// Drives `updater` of the build `version` unattended, reporting to
    /// `report`. Call it right after creating the updater, before it is
    /// ready.
    pub fn start(updater: &Entity<Updater>, report: &Path, version: &str, cx: &mut App) -> Self {
        let events = {
            let report = report.to_path_buf();
            let version = version.to_owned();
            cx.subscribe(updater, move |updater, event, cx| match event {
                UpdaterEvent::Ready => {
                    append(&report, &format!("started {version}"));
                    // The launch check may already be running or done; a
                    // second check would only repeat it.
                    if matches!(
                        updater.read(cx).coordinator().state(),
                        UpdateState::Idle | UpdateState::Disabled { .. }
                    ) {
                        updater.update(cx, |updater, cx| updater.check_for_updates(cx));
                    }
                }
                UpdaterEvent::CheckFinished {
                    result: Err(error), ..
                }
                | UpdaterEvent::Failed(error) => append(&report, &describe(error)),
                UpdaterEvent::Handoff(_) => append(&report, "handoff"),
                _ => {}
            })
        };

        let report = report.to_path_buf();
        let last = RefCell::new(None::<UpdateState>);
        let handing_off = Cell::new(false);
        let state = cx.observe(updater, move |updater, cx| {
            let (state, busy) = {
                let updater = updater.read(cx);
                (updater.state(), updater.is_busy())
            };
            if last.borrow().as_ref() != Some(&state) {
                *last.borrow_mut() = Some(state.clone());
                match &state {
                    UpdateState::UpToDate => append(&report, "up-to-date"),
                    UpdateState::Available(update) => {
                        append(&report, &format!("update-available {}", update.version));
                        install(&updater, &report, cx);
                    }
                    _ => {}
                }
            }
            // Staging reports `Staged` before it has finished, so install
            // once the updater is no longer busy.
            if matches!(state, UpdateState::Staged(_)) && !busy && !handing_off.replace(true) {
                install(&updater, &report, cx);
            }
        });

        Self {
            _events: events,
            _state: state,
        }
    }
}

fn install(updater: &Entity<Updater>, report: &Path, cx: &mut App) {
    if let Err(error) = updater.update(cx, |updater, cx| updater.request_install(cx)) {
        append(report, &describe(&error));
    }
}

fn describe(error: &UpdateError) -> String {
    format!("error {:?}: {}", error.kind(), error.message())
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
    use std::sync::{Arc, Mutex};

    use gpui::{AppContext as _, TestAppContext};
    use gpui_auto_update::core::{
        AvailableUpdate, Capability, CheckOutcome, CheckRequest, CheckSource, ErrorKind,
        MemoryPreferenceStore,
    };
    use gpui_auto_update::{Handoff, ProgressSink, UpdateBackend, UpdaterConfig};

    use super::*;

    /// A feed that offers `Some` version, or nothing newer.
    struct Feed(Option<String>);

    impl CheckSource for Feed {
        fn check(&self, _: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
            Ok(match &self.0 {
                Some(version) => CheckOutcome::UpdateAvailable(AvailableUpdate::new(version)),
                None => CheckOutcome::UpToDate,
            })
        }
    }

    #[derive(Clone)]
    struct Backend {
        calls: Arc<Mutex<Vec<String>>>,
        stage: Result<(), UpdateError>,
    }

    impl Backend {
        fn new(stage: Result<(), UpdateError>) -> Self {
            Self {
                calls: Arc::default(),
                stage,
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl UpdateBackend for Backend {
        fn capability(&self) -> Capability {
            Capability::SelfManaged
        }
        fn stage(&self, update: &AvailableUpdate, _: &ProgressSink) -> Result<(), UpdateError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("stage {}", update.version));
            self.stage.clone()
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
            Ok(Handoff::Quit)
        }
    }

    /// Runs an unattended 1.0.0 build until it is idle and returns its
    /// report.
    fn run(cx: &mut TestAppContext, offer: Option<&str>, backend: &Backend) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report.log");
        let config = UpdaterConfig::new("dev.example.unattended", Feed(offer.map(str::to_owned)))
            .with_backend(backend.clone())
            .with_preferences(MemoryPreferenceStore::new())
            .allow_debug_self_update(true);
        let updater = cx.new(|cx| Updater::new(config, cx));
        let _unattended = cx.update(|cx| Unattended::start(&updater, &report, "1.0.0", cx));
        cx.run_until_parked();
        std::fs::read_to_string(&report)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[gpui::test]
    fn an_unattended_build_installs_the_offered_update(cx: &mut TestAppContext) {
        let backend = Backend::new(Ok(()));
        let report = run(cx, Some("1.1.0"), &backend);
        assert_eq!(backend.calls(), ["stage 1.1.0", "install 1.1.0"]);
        assert_eq!(
            report,
            ["started 1.0.0", "update-available 1.1.0", "handoff"]
        );
    }

    #[gpui::test]
    fn an_unattended_build_that_is_up_to_date_says_so(cx: &mut TestAppContext) {
        let backend = Backend::new(Ok(()));
        let report = run(cx, None, &backend);
        assert!(backend.calls().is_empty());
        assert_eq!(report, ["started 1.0.0", "up-to-date"]);
    }

    #[gpui::test]
    fn a_failed_update_is_reported_and_nothing_is_installed(cx: &mut TestAppContext) {
        let backend = Backend::new(Err(
            UpdateError::new(ErrorKind::Signature).with_message("The signature is invalid.")
        ));
        let report = run(cx, Some("1.1.0"), &backend);
        assert_eq!(backend.calls(), ["stage 1.1.0"]);
        assert_eq!(
            report,
            [
                "started 1.0.0",
                "update-available 1.1.0",
                "error Signature: The signature is invalid."
            ]
        );
    }
}
