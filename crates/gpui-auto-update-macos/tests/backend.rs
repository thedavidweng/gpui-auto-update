//! The Sparkle backend's lifecycle through a scripted engine: check routing,
//! outcome and error mapping, mirrored preferences, and state tracking.
//! Nothing here needs the Sparkle framework.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use gpui_auto_update_core::{
    AutomaticChecks, AvailableUpdate, Capability, Channel, CheckKind, CheckOutcome, CheckPolicy,
    ErrorKind, PreferenceOwner, PreferenceStore, ReleaseNotes, ReleaseNotesFormat,
    UpdateCoordinator, UpdateError, UpdatePreferences, UpdateState,
};
use gpui_auto_update_macos::{
    GpuiPresentation, NoUpdateReason, PresentationPolicy, RelaunchContinuation, SessionState,
    SparkleBackend, SparkleEngine, SparkleError, SparkleEvent, SparkleEvents, SparkleUpdate,
    UpdateStage, UserChoice,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    StandardUi,
    Background,
    SetAutomatic(bool),
    SetChannels(Option<Vec<String>>),
}

/// A Sparkle stand-in that publishes scripted events when a check starts,
/// either before the call returns or later from another thread, as the
/// real delegate does from the main thread.
#[derive(Clone)]
struct MockEngine {
    events: SparkleEvents,
    state: Arc<Mutex<MockState>>,
}

struct MockState {
    calls: Vec<Call>,
    scripts: VecDeque<Vec<SparkleEvent>>,
    asynchronous: bool,
    session_in_progress: bool,
    automatic: bool,
    last_check: Option<SystemTime>,
    channels: Option<Vec<String>>,
}

impl MockEngine {
    fn new(events: &SparkleEvents) -> Self {
        Self {
            events: events.clone(),
            state: Arc::new(Mutex::new(MockState {
                calls: Vec::new(),
                scripts: VecDeque::new(),
                asynchronous: false,
                session_in_progress: false,
                automatic: true,
                last_check: None,
                channels: None,
            })),
        }
    }

    /// Events published by the next check Sparkle runs.
    fn script(&self, events: Vec<SparkleEvent>) -> &Self {
        self.state.lock().unwrap().scripts.push_back(events);
        self
    }

    fn asynchronous(&self) -> &Self {
        self.state.lock().unwrap().asynchronous = true;
        self
    }

    fn with_session_in_progress(&self) -> &Self {
        self.state.lock().unwrap().session_in_progress = true;
        self
    }

    /// Changes Sparkle's stored preference behind the backend's back, as
    /// Sparkle's own permission prompt or settings would.
    fn set_stored_automatic(&self, enabled: bool) {
        self.state.lock().unwrap().automatic = enabled;
    }

    fn set_stored_last_check(&self, last_check: SystemTime) {
        self.state.lock().unwrap().last_check = Some(last_check);
    }

    fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    fn run_check(&self, call: Call) -> Result<(), UpdateError> {
        let (script, asynchronous) = {
            let mut state = self.state.lock().unwrap();
            state.calls.push(call);
            (
                state.scripts.pop_front().unwrap_or_default(),
                state.asynchronous,
            )
        };
        let events = self.events.clone();
        let publish = move || {
            for event in script {
                events.publish(event);
            }
        };
        if asynchronous {
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(20));
                publish();
            });
        } else {
            publish();
        }
        Ok(())
    }
}

impl SparkleEngine for MockEngine {
    fn check_for_updates(&self) -> Result<(), UpdateError> {
        self.run_check(Call::StandardUi)
    }

    fn check_for_updates_in_background(&self) -> Result<(), UpdateError> {
        self.run_check(Call::Background)
    }

    fn session_in_progress(&self) -> Result<bool, UpdateError> {
        Ok(self.state.lock().unwrap().session_in_progress)
    }

    fn automatically_checks_for_updates(&self) -> Result<bool, UpdateError> {
        Ok(self.state.lock().unwrap().automatic)
    }

    fn set_automatically_checks_for_updates(&self, enabled: bool) -> Result<(), UpdateError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(Call::SetAutomatic(enabled));
        state.automatic = enabled;
        Ok(())
    }

    fn last_update_check(&self) -> Result<Option<SystemTime>, UpdateError> {
        Ok(self.state.lock().unwrap().last_check)
    }

    fn allowed_channels(&self) -> Result<Option<Vec<String>>, UpdateError> {
        Ok(self.state.lock().unwrap().channels.clone())
    }

    fn set_allowed_channels(&self, channels: Option<Vec<String>>) -> Result<(), UpdateError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(Call::SetChannels(channels.clone()));
        state.channels = channels;
        Ok(())
    }
}

fn backend() -> (SparkleBackend, MockEngine) {
    let events = SparkleEvents::new();
    let engine = MockEngine::new(&events);
    (SparkleBackend::new(engine.clone(), events), engine)
}

fn coordinator(backend: &SparkleBackend) -> UpdateCoordinator {
    UpdateCoordinator::new(backend.clone(), backend.capability())
}

fn found(version: &str) -> SparkleEvent {
    SparkleEvent::UpdateFound(SparkleUpdate::new(version))
}

/// Waits for the state tracker, which applies Sparkle events on its own
/// thread.
fn wait_for(coordinator: &UpdateCoordinator, expected: impl Fn(&UpdateState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !expected(&coordinator.state()) {
        assert!(
            Instant::now() < deadline,
            "state did not settle: {:?}",
            coordinator.state()
        );
        thread::sleep(Duration::from_millis(5));
    }
}

/// A coordinator whose state is `Available(version)` after a background
/// check, with the tracker attached.
fn available(version: &str) -> (SparkleBackend, MockEngine, UpdateCoordinator) {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    backend.attach(&coordinator);
    engine.script(vec![found(version)]);
    coordinator.check(CheckKind::Background).unwrap();
    (backend, engine, coordinator)
}

// --- Checks -----------------------------------------------------------------

#[test]
fn manual_check_uses_sparkles_standard_ui_and_reports_the_found_update() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.script(vec![
        SparkleEvent::AppcastLoaded,
        SparkleEvent::UpdateFound(
            SparkleUpdate::new("2.0.0")
                .with_channel("beta")
                .with_release_notes("<p>Faster.</p>", Some("html"))
                .with_published("Mon, 05 Oct 2026 10:00:00 +0000")
                .with_critical(true)
                .with_major(true),
        ),
    ]);

    let outcome = coordinator.check(CheckKind::Manual).unwrap();

    let expected = AvailableUpdate::new("2.0.0")
        .with_channel(Channel::new("beta"))
        .with_release_notes(ReleaseNotes::Inline {
            content: "<p>Faster.</p>".into(),
            format: ReleaseNotesFormat::Html,
        })
        .with_published("Mon, 05 Oct 2026 10:00:00 +0000")
        .with_critical(true)
        .with_major(true);
    assert_eq!(outcome, CheckOutcome::UpdateAvailable(expected.clone()));
    assert_eq!(coordinator.state(), UpdateState::Available(expected));
    assert_eq!(engine.calls(), vec![Call::StandardUi]);
}

#[test]
fn background_check_uses_sparkles_background_check() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.script(vec![
        SparkleEvent::AppcastLoaded,
        SparkleEvent::NoUpdateFound(NoUpdateReason::OnLatestVersion),
        SparkleEvent::Aborted(SparkleError::sparkle(1001, "You're up to date!")),
        SparkleEvent::CycleFinished { error: None },
    ]);

    let outcome = coordinator.check(CheckKind::Background).unwrap();

    assert_eq!(outcome, CheckOutcome::UpToDate);
    assert_eq!(engine.calls(), vec![Call::Background]);
}

#[test]
fn events_delivered_later_from_another_thread_resolve_the_check() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.asynchronous().script(vec![found("3.1")]);

    let outcome = coordinator.check(CheckKind::Manual).unwrap();

    assert_eq!(
        outcome,
        CheckOutcome::UpdateAvailable(AvailableUpdate::new("3.1"))
    );
}

#[test]
fn plain_text_and_linked_release_notes_are_preserved() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine
        .script(vec![SparkleEvent::UpdateFound(
            SparkleUpdate::new("2.0").with_release_notes("Faster.", Some("plain-text")),
        )])
        .script(vec![SparkleEvent::UpdateFound(
            SparkleUpdate::new("2.1").with_release_notes_url("https://example.com/notes/2.1"),
        )]);

    let first = coordinator.check(CheckKind::Manual).unwrap();
    let second = coordinator.check(CheckKind::Manual).unwrap();

    let notes = |outcome: CheckOutcome| match outcome {
        CheckOutcome::UpdateAvailable(update) => update.release_notes,
        CheckOutcome::UpToDate => None,
    };
    assert_eq!(
        notes(first),
        Some(ReleaseNotes::Inline {
            content: "Faster.".into(),
            format: ReleaseNotesFormat::PlainText,
        })
    );
    assert_eq!(
        notes(second),
        Some(ReleaseNotes::Link("https://example.com/notes/2.1".into()))
    );
}

#[test]
fn sparkle_failures_become_structured_errors() {
    let cases = [
        (
            SparkleError::sparkle(1002, "appcast"),
            ErrorKind::FeedRetrieval,
        ),
        (SparkleError::sparkle(1000, "parse"), ErrorKind::FeedParsing),
        (SparkleError::sparkle(2001, "download"), ErrorKind::Download),
        (
            SparkleError::sparkle(3001, "signature"),
            ErrorKind::Signature,
        ),
        (
            SparkleError::sparkle(3000, "unarchive"),
            ErrorKind::ArchiveValidation,
        ),
        (SparkleError::sparkle(4004, "relaunch"), ErrorKind::Relaunch),
        (
            SparkleError::sparkle(4005, "install"),
            ErrorKind::Replacement,
        ),
        (
            SparkleError::sparkle(1003, "disk image"),
            ErrorKind::UnsupportedInstallation,
        ),
        (
            SparkleError::sparkle(3, "insecure feed"),
            ErrorKind::Configuration,
        ),
        (
            SparkleError::new("NSURLErrorDomain", -1009, "offline"),
            ErrorKind::FeedRetrieval,
        ),
    ];
    for (sparkle_error, kind) in cases {
        let (backend, engine) = backend();
        let coordinator = coordinator(&backend);
        engine.script(vec![SparkleEvent::Aborted(sparkle_error.clone())]);

        let error = coordinator.check(CheckKind::Manual).unwrap_err();

        assert_eq!(error.kind(), kind, "for {sparkle_error:?}");
        let diagnostic = error.diagnostic().unwrap_or_default();
        assert!(
            diagnostic.contains(&sparkle_error.domain)
                && diagnostic.contains(&sparkle_error.code.to_string()),
            "diagnostic {diagnostic:?} should identify {sparkle_error:?}"
        );
        assert_eq!(coordinator.state(), UpdateState::Failed(error));
    }
}

#[test]
fn a_cycle_that_ends_with_an_error_fails_the_check() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.script(vec![SparkleEvent::CycleFinished {
        error: Some(SparkleError::sparkle(1002, "appcast")),
    }]);

    let error = coordinator.check(CheckKind::Manual).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::FeedRetrieval);
}

#[test]
fn check_fails_when_sparkle_never_answers() {
    let (backend, engine) = backend();
    let backend = backend.with_check_timeout(Duration::from_millis(50));
    let coordinator = coordinator(&backend);

    let error = coordinator.check(CheckKind::Manual).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::TemporarilyUnavailable);
    assert_eq!(engine.calls(), vec![Call::StandardUi]);
}

#[test]
fn manual_check_during_a_session_brings_sparkle_forward_with_the_found_update() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    backend.events().publish(found("4.0"));
    engine.with_session_in_progress();

    let manual = coordinator.check(CheckKind::Manual).unwrap();
    let background = coordinator.check(CheckKind::Background).unwrap();

    let expected = CheckOutcome::UpdateAvailable(AvailableUpdate::new("4.0"));
    assert_eq!(manual, expected);
    assert_eq!(background, expected);
    assert_eq!(engine.calls(), vec![Call::StandardUi]);
}

// --- Scheduled discoveries ---------------------------------------------------

#[test]
fn a_scheduled_discovery_is_adopted_into_the_available_state() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    backend.attach(&coordinator);
    // Sparkle's scheduler found an update outside any facade check and is
    // holding the session open for the gentle reminder.
    engine.with_session_in_progress();

    backend.events().publish(found("5.0"));

    wait_for(&coordinator, |s| {
        *s == UpdateState::Available(AvailableUpdate::new("5.0"))
    });
    assert_eq!(engine.calls(), vec![], "no new Sparkle check runs");
}

#[test]
fn a_discovery_matching_the_current_update_is_not_adopted_again() {
    let (backend, engine, coordinator) = available("5.0");
    engine.with_session_in_progress();

    backend.events().publish(found("5.0"));

    // Nothing to wait for: adoption must not happen. Give the tracker a
    // moment, then assert the state and the calls did not change.
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        coordinator.state(),
        UpdateState::Available(AvailableUpdate::new("5.0"))
    );
    assert_eq!(engine.calls(), vec![Call::Background]);
}

#[test]
fn a_discovery_during_another_operation_is_left_to_it() {
    let (backend, engine, coordinator) = available("2.0");
    engine.with_session_in_progress();
    backend.events().publish(SparkleEvent::WillInstallOnQuit {
        version: "2.0".into(),
    });
    wait_for(&coordinator, |s| {
        *s == UpdateState::WaitingForQuit(AvailableUpdate::new("2.0"))
    });

    backend.events().publish(found("3.0"));

    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        coordinator.state(),
        UpdateState::WaitingForQuit(AvailableUpdate::new("2.0")),
        "the running session owns the state"
    );
}

#[test]
fn a_failed_scheduled_session_surfaces_as_a_failed_state() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    backend.attach(&coordinator);
    engine.with_session_in_progress();
    backend.events().publish(found("2.0"));
    wait_for(&coordinator, |s| {
        *s == UpdateState::Available(AvailableUpdate::new("2.0"))
    });

    backend
        .events()
        .publish(SparkleEvent::Aborted(SparkleError::sparkle(
            3001,
            "bad signature",
        )));

    wait_for(
        &coordinator,
        |s| matches!(s, UpdateState::Failed(error) if error.kind() == ErrorKind::Signature),
    );
}

#[test]
fn a_failed_scheduled_check_without_a_session_stays_silent() {
    let (backend, _) = backend();
    let coordinator = coordinator(&backend);
    backend.attach(&coordinator);

    backend
        .events()
        .publish(SparkleEvent::Aborted(SparkleError::sparkle(
            1002,
            "appcast unreachable",
        )));

    thread::sleep(Duration::from_millis(50));
    assert_eq!(coordinator.state(), UpdateState::Idle);
}

// --- Relaunch coordination ---------------------------------------------------

#[test]
fn the_handoff_gate_receives_postponed_relaunches_and_resumes_them_once() {
    let (backend, _) = backend();
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_gate = received.clone();
    backend.set_handoff_gate(move |continuation| {
        received_by_gate.lock().unwrap().push(continuation);
    });
    let resumes = Arc::new(AtomicUsize::new(0));
    let counting = resumes.clone();

    backend.events().publish(SparkleEvent::RelaunchRequested {
        update: SparkleUpdate::new("2.0"),
        continuation: RelaunchContinuation::from_fn(move || {
            counting.fetch_add(1, Ordering::SeqCst);
        }),
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    while received.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the gate never ran");
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(resumes.load(Ordering::SeqCst), 0, "the gate decides when");

    let continuation = received.lock().unwrap().remove(0);
    let clone = continuation.clone();
    continuation.resume();
    clone.resume();
    assert_eq!(resumes.load(Ordering::SeqCst), 1, "resuming is one-shot");
}

#[test]
fn a_dropped_continuation_stays_postponed() {
    let resumes = Arc::new(AtomicUsize::new(0));
    let counting = resumes.clone();
    let continuation = RelaunchContinuation::from_fn(move || {
        counting.fetch_add(1, Ordering::SeqCst);
    });

    drop(continuation);

    assert_eq!(resumes.load(Ordering::SeqCst), 0);
}

// --- Presentation ------------------------------------------------------------

#[test]
fn the_default_policy_keeps_scheduled_updates_out_of_sparkles_window() {
    let policy = GpuiPresentation;
    let update = SparkleUpdate::new("2.0");

    assert!(!policy.should_show_scheduled_update(&update, false));
    assert!(
        !policy.should_show_scheduled_update(&update, true),
        "not even in immediate focus (critical updates, impatient reminders)"
    );
    policy.will_show_update(
        false,
        &update,
        SessionState {
            stage: UpdateStage::NotDownloaded,
            user_initiated: false,
        },
    );
    policy.did_receive_user_attention(&update);
    policy.will_finish_update_session();
}

#[test]
fn ending_a_session_clears_the_pending_update() {
    let events = SparkleEvents::new();
    events.publish(found("2.0"));
    assert!(events.pending_update().is_some());

    events.publish(SparkleEvent::SessionWillFinish);

    assert_eq!(events.pending_update(), None);
}

// --- Capability -------------------------------------------------------------

#[test]
fn a_running_sparkle_updater_is_self_managed() {
    let (backend, _) = backend();
    assert_eq!(backend.capability(), Capability::SelfManaged);
}

#[test]
fn without_sparkle_the_installation_is_unsupported() {
    let backend = SparkleBackend::unsupported();
    let coordinator = coordinator(&backend);

    assert_eq!(backend.capability(), Capability::Unsupported);
    assert_eq!(
        coordinator.check(CheckKind::Manual).unwrap_err().kind(),
        ErrorKind::UnsupportedInstallation
    );
    assert_eq!(backend.preferences().load().unwrap(), None);
    assert_eq!(
        backend.install().unwrap_err().kind(),
        ErrorKind::UnsupportedInstallation
    );
}

// --- Preferences ------------------------------------------------------------

#[test]
fn preferences_are_owned_by_sparkle_and_mirror_its_stored_values() {
    let (backend, engine) = backend();
    let checked = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    engine.set_stored_automatic(false);
    engine.set_stored_last_check(checked);
    let store = backend.preferences();

    assert_eq!(store.owner(), PreferenceOwner::Backend);
    assert_eq!(
        store.load().unwrap(),
        Some(UpdatePreferences::new(false).with_last_check(Some(checked)))
    );
}

#[test]
fn automatic_checks_are_written_through_sparkle_and_follow_its_changes() {
    let (backend, engine) = backend();
    let automatic = AutomaticChecks::new(
        coordinator(&backend),
        backend.preferences(),
        CheckPolicy::recommended(),
    );
    assert!(automatic.automatic_checks_enabled());

    automatic.set_automatic_checks(false).unwrap();
    assert!(!engine.automatically_checks_for_updates().unwrap());
    assert_eq!(engine.calls(), vec![Call::SetAutomatic(false)]);

    engine.set_stored_automatic(true);
    assert!(automatic.automatic_checks_enabled());
    assert_eq!(automatic.periodic_check_delay(), None);
}

#[test]
fn saving_unchanged_preferences_does_not_touch_sparkle() {
    let (backend, engine) = backend();
    let store = backend.preferences();

    store
        .save(&UpdatePreferences::new(true).with_last_check(Some(SystemTime::now())))
        .unwrap();

    assert_eq!(engine.calls(), vec![]);
}

// --- Channels ---------------------------------------------------------------

#[test]
fn channel_selects_sparkles_allowed_channels() {
    let (backend, engine) = backend();
    assert_eq!(backend.channel(), None);

    backend.set_channel(Some(Channel::new("beta"))).unwrap();
    assert_eq!(backend.channel(), Some(Channel::new("beta")));

    backend.set_channel(None).unwrap();
    assert_eq!(backend.channel(), None);
    assert_eq!(
        engine.calls(),
        vec![
            Call::SetChannels(Some(vec!["beta".into()])),
            Call::SetChannels(None),
        ]
    );
}

// --- Lifecycle tracking -----------------------------------------------------

#[test]
fn sparkle_driven_download_and_install_are_mirrored_in_the_state() {
    let (backend, _, coordinator) = available("2.0");
    let update = AvailableUpdate::new("2.0");
    let events = backend.events();

    events.publish(SparkleEvent::UserChoice {
        choice: UserChoice::Install,
        stage: UpdateStage::NotDownloaded,
        version: "2.0".into(),
    });
    events.publish(SparkleEvent::WillDownload {
        version: "2.0".into(),
    });
    wait_for(&coordinator, |s| {
        matches!(s, UpdateState::Downloading { .. })
    });

    events.publish(SparkleEvent::Downloaded {
        version: "2.0".into(),
    });
    events.publish(SparkleEvent::WillExtract {
        version: "2.0".into(),
    });
    wait_for(&coordinator, |s| {
        *s == UpdateState::Verifying(update.clone())
    });

    events.publish(SparkleEvent::Extracted {
        version: "2.0".into(),
    });
    wait_for(&coordinator, |s| *s == UpdateState::Staged(update.clone()));

    events.publish(SparkleEvent::WillInstall {
        version: "2.0".into(),
    });
    wait_for(&coordinator, |s| {
        *s == UpdateState::Installing(update.clone())
    });

    events.publish(SparkleEvent::WillRelaunch);
    wait_for(&coordinator, |s| {
        *s == UpdateState::Relaunching(update.clone())
    });
}

#[test]
fn a_silently_downloaded_update_waits_for_quit() {
    let (backend, _, coordinator) = available("2.0");

    backend.events().publish(SparkleEvent::WillInstallOnQuit {
        version: "2.0".into(),
    });

    wait_for(&coordinator, |s| {
        *s == UpdateState::WaitingForQuit(AvailableUpdate::new("2.0"))
    });
}

#[test]
fn a_failed_download_is_reported() {
    let (backend, _, coordinator) = available("2.0");
    let events = backend.events();

    events.publish(SparkleEvent::WillDownload {
        version: "2.0".into(),
    });
    events.publish(SparkleEvent::DownloadFailed {
        version: "2.0".into(),
        error: SparkleError::sparkle(2001, "network lost"),
    });

    wait_for(
        &coordinator,
        |s| matches!(s, UpdateState::Failed(error) if error.kind() == ErrorKind::Download),
    );
}

#[test]
fn skipping_or_dismissing_the_update_in_sparkle_returns_to_idle() {
    for choice in [UserChoice::Skip, UserChoice::Dismiss] {
        let (backend, _, coordinator) = available("2.0");

        backend.events().publish(SparkleEvent::UserChoice {
            choice,
            stage: UpdateStage::NotDownloaded,
            version: "2.0".into(),
        });

        wait_for(&coordinator, |s| *s == UpdateState::Idle);
    }
}

#[test]
fn dismissing_an_installing_update_still_installs_it_on_quit() {
    let (backend, _, coordinator) = available("2.0");
    let events = backend.events();
    events.publish(SparkleEvent::Extracted {
        version: "2.0".into(),
    });
    events.publish(SparkleEvent::WillInstall {
        version: "2.0".into(),
    });

    events.publish(SparkleEvent::UserChoice {
        choice: UserChoice::Dismiss,
        stage: UpdateStage::Installing,
        version: "2.0".into(),
    });

    wait_for(&coordinator, |s| {
        *s == UpdateState::WaitingForQuit(AvailableUpdate::new("2.0"))
    });
}

// --- Staging and installing -------------------------------------------------

#[test]
fn staging_presents_the_update_in_sparkle_and_returns_once_it_is_ready() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.script(vec![found("2.0")]);
    coordinator.check(CheckKind::Background).unwrap();
    engine.asynchronous().script(vec![
        found("2.0"),
        SparkleEvent::UserChoice {
            choice: UserChoice::Install,
            stage: UpdateStage::NotDownloaded,
            version: "2.0".into(),
        },
        SparkleEvent::CycleFinished { error: None },
        SparkleEvent::WillDownload {
            version: "2.0".into(),
        },
        SparkleEvent::Downloaded {
            version: "2.0".into(),
        },
        SparkleEvent::WillExtract {
            version: "2.0".into(),
        },
        SparkleEvent::Extracted {
            version: "2.0".into(),
        },
    ]);

    backend.stage(&coordinator).unwrap();

    assert_eq!(
        coordinator.state(),
        UpdateState::Staged(AvailableUpdate::new("2.0"))
    );
    assert_eq!(engine.calls(), vec![Call::Background, Call::StandardUi]);
}

#[test]
fn staging_ends_without_an_update_when_the_user_postpones_it_in_sparkle() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.script(vec![found("2.0")]);
    coordinator.check(CheckKind::Background).unwrap();
    engine.script(vec![
        found("2.0"),
        SparkleEvent::UserChoice {
            choice: UserChoice::Dismiss,
            stage: UpdateStage::NotDownloaded,
            version: "2.0".into(),
        },
        SparkleEvent::CycleFinished { error: None },
    ]);

    let error = backend.stage(&coordinator).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::InvalidState);
    assert_eq!(coordinator.state(), UpdateState::Idle);
}

#[test]
fn staging_reports_sparkles_failure() {
    let (backend, engine) = backend();
    let coordinator = coordinator(&backend);
    engine.script(vec![found("2.0")]);
    coordinator.check(CheckKind::Background).unwrap();
    engine.script(vec![
        SparkleEvent::WillDownload {
            version: "2.0".into(),
        },
        SparkleEvent::Aborted(SparkleError::sparkle(3001, "bad signature")),
    ]);

    let error = backend.stage(&coordinator).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Signature);
    assert!(matches!(coordinator.state(), UpdateState::Failed(_)));
}

#[test]
fn installing_hands_the_staged_update_to_sparkles_standard_ui() {
    let (backend, engine) = backend();

    backend.install().unwrap();
    backend.relaunch().unwrap();

    assert_eq!(engine.calls(), vec![Call::StandardUi, Call::StandardUi]);
}
