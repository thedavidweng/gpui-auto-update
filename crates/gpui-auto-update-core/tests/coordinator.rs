//! Behavioral tests for the check coordinator, driven through the public API
//! with deterministic mock check sources.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use gpui_auto_update_core::{
    AvailableUpdate, Capability, Channel, CheckKind, CheckOutcome, CheckRequest, CheckSource,
    ErrorKind, ReleaseNotes, UpdateCoordinator, UpdateError, UpdateEvent, UpdateState,
};

/// Returns a fixed result and counts how often it was asked.
#[derive(Clone)]
struct Fixed {
    result: Result<CheckOutcome, UpdateError>,
    calls: Arc<AtomicUsize>,
}

impl Fixed {
    fn new(result: Result<CheckOutcome, UpdateError>) -> Self {
        Self {
            result,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CheckSource for Fixed {
    fn check(&self, _request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

/// Blocks inside `check` until the test releases it, so the test controls
/// exactly when a check is in flight.
struct Gated {
    entered: Mutex<mpsc::Sender<CheckKind>>,
    release: Mutex<mpsc::Receiver<Result<CheckOutcome, UpdateError>>>,
    calls: Arc<AtomicUsize>,
}

struct Gate {
    entered: mpsc::Receiver<CheckKind>,
    release: mpsc::Sender<Result<CheckOutcome, UpdateError>>,
    calls: Arc<AtomicUsize>,
}

fn gated() -> (Gated, Gate) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Gated {
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
            calls: calls.clone(),
        },
        Gate {
            entered: entered_rx,
            release: release_tx,
            calls,
        },
    )
}

impl CheckSource for Gated {
    fn check(&self, request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.lock().unwrap().send(request.kind).unwrap();
        self.release.lock().unwrap().recv().unwrap()
    }
}

fn update() -> AvailableUpdate {
    AvailableUpdate::new("2.0.0")
        .with_build("200")
        .with_channel(Channel::new("beta"))
        .with_release_notes(ReleaseNotes::Link(
            "https://example.invalid/notes/2.0.0".into(),
        ))
        .with_published("Tue, 02 Jun 2026 10:00:00 +0000")
        .with_critical(true)
}

fn feed_error() -> UpdateError {
    UpdateError::new(ErrorKind::FeedRetrieval)
        .with_diagnostic("GET https://user:secret@example.invalid/feed.xml timed out")
}

/// Waits until the coordinator reports `expected`, failing after a timeout.
fn wait_for_state(coordinator: &UpdateCoordinator, expected: &UpdateState) {
    for _ in 0..500 {
        if &coordinator.state() == expected {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!(
        "state never became {expected:?}; it is {:?}",
        coordinator.state()
    );
}

fn self_managed(source: impl CheckSource) -> UpdateCoordinator {
    UpdateCoordinator::new(source, Capability::SelfManaged)
}

#[test]
fn manual_check_reports_up_to_date() {
    let coordinator = self_managed(Fixed::new(Ok(CheckOutcome::UpToDate)));

    let outcome = coordinator.check(CheckKind::Manual);

    assert_eq!(outcome, Ok(CheckOutcome::UpToDate));
    assert_eq!(coordinator.state(), UpdateState::UpToDate);
}

#[test]
fn background_check_that_finds_an_update_exposes_its_metadata() {
    let coordinator = self_managed(Fixed::new(Ok(CheckOutcome::UpdateAvailable(update()))));

    let outcome = coordinator.check(CheckKind::Background);

    assert_eq!(outcome, Ok(CheckOutcome::UpdateAvailable(update())));
    let state = coordinator.state();
    let available = state.update().expect("state carries the update");
    assert_eq!(state, UpdateState::Available(update()));
    assert_eq!(available.version, "2.0.0");
    assert_eq!(available.build.as_deref(), Some("200"));
    assert_eq!(available.channel, Some(Channel::new("beta")));
    assert_eq!(
        available.release_notes,
        Some(ReleaseNotes::Link(
            "https://example.invalid/notes/2.0.0".into()
        ))
    );
    assert_eq!(
        available.published.as_deref(),
        Some("Tue, 02 Jun 2026 10:00:00 +0000")
    );
    assert!(available.critical);
    assert!(!available.major);
}

#[test]
fn manual_check_failure_is_visible_as_failed_state() {
    let coordinator = self_managed(Fixed::new(Err(feed_error())));

    let outcome = coordinator.check(CheckKind::Manual);

    assert_eq!(outcome, Err(feed_error()));
    assert_eq!(coordinator.state(), UpdateState::Failed(feed_error()));
}

#[test]
fn background_check_failure_keeps_the_previous_state() {
    let (source, gate) = gated();
    let coordinator = self_managed(source);

    gate.release
        .send(Ok(CheckOutcome::UpdateAvailable(update())))
        .unwrap();
    coordinator.check(CheckKind::Background).unwrap();

    gate.release.send(Err(feed_error())).unwrap();
    let outcome = coordinator.check(CheckKind::Background);

    assert_eq!(outcome, Err(feed_error()));
    assert_eq!(coordinator.state(), UpdateState::Available(update()));
}

#[test]
fn background_check_failure_from_idle_returns_to_idle() {
    let coordinator = self_managed(Fixed::new(Err(feed_error())));

    let _ = coordinator.check(CheckKind::Background);

    assert_eq!(coordinator.state(), UpdateState::Idle);
}

#[test]
fn concurrent_checks_coalesce_into_one_source_call() {
    let (source, gate) = gated();
    let coordinator = self_managed(source);

    let first = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Background)
    });
    assert_eq!(gate.entered.recv().unwrap(), CheckKind::Background);
    assert_eq!(coordinator.state(), UpdateState::Checking);

    let attached: Vec<_> = (0..3)
        .map(|_| {
            let coordinator = coordinator.clone();
            thread::spawn(move || coordinator.check(CheckKind::Background))
        })
        .collect();
    thread::sleep(Duration::from_millis(50));
    gate.release
        .send(Ok(CheckOutcome::UpdateAvailable(update())))
        .unwrap();

    let expected = Ok(CheckOutcome::UpdateAvailable(update()));
    assert_eq!(first.join().unwrap(), expected);
    for handle in attached {
        assert_eq!(handle.join().unwrap(), expected);
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn manual_check_during_background_check_receives_a_visible_failure() {
    let (source, gate) = gated();
    let coordinator = self_managed(source);

    let background = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Background)
    });
    gate.entered.recv().unwrap();
    let manual = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Manual)
    });
    thread::sleep(Duration::from_millis(50));
    gate.release.send(Err(feed_error())).unwrap();

    assert_eq!(manual.join().unwrap(), Err(feed_error()));
    assert_eq!(background.join().unwrap(), Err(feed_error()));
    assert_eq!(coordinator.state(), UpdateState::Failed(feed_error()));
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn manual_check_during_background_check_receives_up_to_date() {
    let (source, gate) = gated();
    let coordinator = self_managed(source);

    let background = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Background)
    });
    gate.entered.recv().unwrap();
    let manual = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Manual)
    });
    thread::sleep(Duration::from_millis(50));
    gate.release.send(Ok(CheckOutcome::UpToDate)).unwrap();

    assert_eq!(manual.join().unwrap(), Ok(CheckOutcome::UpToDate));
    background.join().unwrap().unwrap();
    assert_eq!(coordinator.state(), UpdateState::UpToDate);
}

#[test]
fn a_new_check_starts_after_the_previous_one_finished() {
    let source = Fixed::new(Ok(CheckOutcome::UpToDate));
    let coordinator = self_managed(source.clone());

    coordinator.check(CheckKind::Background).unwrap();
    coordinator.check(CheckKind::Manual).unwrap();

    assert_eq!(source.calls(), 2);
}

#[test]
fn externally_managed_installation_rejects_checks_without_contacting_the_source() {
    let source = Fixed::new(Ok(CheckOutcome::UpToDate));
    let capability = Capability::ExternallyManaged {
        manager: Some("Homebrew".into()),
    };
    let coordinator = UpdateCoordinator::new(source.clone(), capability.clone());

    assert_eq!(
        coordinator.state(),
        UpdateState::Disabled {
            capability: capability.clone()
        }
    );
    let error = coordinator.check(CheckKind::Manual).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::ExternallyManaged);
    assert!(error.message().contains("Homebrew"));
    assert_eq!(source.calls(), 0);
    assert_eq!(coordinator.state(), UpdateState::Disabled { capability });
}

#[test]
fn each_non_self_managed_capability_has_its_own_error_class() {
    let cases = [
        (
            Capability::ExternallyManaged { manager: None },
            ErrorKind::ExternallyManaged,
        ),
        (Capability::Unsupported, ErrorKind::UnsupportedInstallation),
        (
            Capability::TemporarilyUnavailable,
            ErrorKind::TemporarilyUnavailable,
        ),
    ];
    for (capability, kind) in cases {
        assert!(!capability.can_self_update());
        let coordinator =
            UpdateCoordinator::new(Fixed::new(Ok(CheckOutcome::UpToDate)), capability);
        assert_eq!(
            coordinator.check(CheckKind::Manual).unwrap_err().kind(),
            kind
        );
        assert_eq!(
            coordinator.check(CheckKind::Background).unwrap_err().kind(),
            kind
        );
    }
    assert!(Capability::SelfManaged.can_self_update());
}

#[test]
fn capability_changes_enable_and_disable_updating() {
    let coordinator = self_managed(Fixed::new(Ok(CheckOutcome::UpToDate)));

    coordinator
        .set_capability(Capability::TemporarilyUnavailable)
        .unwrap();
    assert_eq!(coordinator.capability(), Capability::TemporarilyUnavailable);
    assert_eq!(
        coordinator.state(),
        UpdateState::Disabled {
            capability: Capability::TemporarilyUnavailable
        }
    );
    assert_eq!(
        coordinator.check(CheckKind::Manual).unwrap_err().kind(),
        ErrorKind::TemporarilyUnavailable
    );

    coordinator.set_capability(Capability::SelfManaged).unwrap();
    assert_eq!(coordinator.state(), UpdateState::Idle);
    assert_eq!(
        coordinator.check(CheckKind::Manual),
        Ok(CheckOutcome::UpToDate)
    );
}

#[test]
fn capability_lost_during_a_check_leaves_updating_disabled() {
    let (source, gate) = gated();
    let coordinator = self_managed(source);

    let check = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Manual)
    });
    gate.entered.recv().unwrap();
    coordinator.set_capability(Capability::Unsupported).unwrap();
    gate.release.send(Ok(CheckOutcome::UpToDate)).unwrap();

    assert_eq!(check.join().unwrap(), Ok(CheckOutcome::UpToDate));
    assert_eq!(
        coordinator.state(),
        UpdateState::Disabled {
            capability: Capability::Unsupported
        }
    );
}

#[test]
fn checks_are_rejected_while_an_update_is_downloading_or_staged() {
    let source = Fixed::new(Ok(CheckOutcome::UpdateAvailable(update())));
    let coordinator = self_managed(source.clone());
    coordinator.check(CheckKind::Background).unwrap();

    coordinator
        .apply(UpdateEvent::DownloadStarted { total: None })
        .unwrap();
    let busy = coordinator.check(CheckKind::Manual).unwrap_err();
    assert_eq!(busy.kind(), ErrorKind::OperationInProgress);

    coordinator.apply(UpdateEvent::Staged).unwrap();
    let staged = coordinator.check(CheckKind::Background).unwrap_err();
    assert_eq!(staged.kind(), ErrorKind::InvalidState);

    assert_eq!(source.calls(), 1);
    assert_eq!(coordinator.state(), UpdateState::Staged(update()));
}

#[test]
fn capability_cannot_change_while_installing() {
    let coordinator = self_managed(Fixed::new(Ok(CheckOutcome::UpdateAvailable(update()))));
    coordinator.check(CheckKind::Manual).unwrap();
    coordinator
        .apply(UpdateEvent::DownloadStarted { total: Some(10) })
        .unwrap();

    let error = coordinator
        .set_capability(Capability::Unsupported)
        .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::OperationInProgress);
    assert_eq!(coordinator.capability(), Capability::SelfManaged);
}

#[test]
fn observers_see_every_state_change_in_order_until_unsubscribed() {
    let coordinator = self_managed(Fixed::new(Ok(CheckOutcome::UpdateAvailable(update()))));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let subscription = coordinator.subscribe({
        let seen = seen.clone();
        move |state: &UpdateState| seen.lock().unwrap().push(state.clone())
    });

    coordinator.check(CheckKind::Manual).unwrap();
    coordinator
        .apply(UpdateEvent::DownloadStarted { total: None })
        .unwrap();
    drop(subscription);
    coordinator.apply(UpdateEvent::Staged).unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            UpdateState::Checking,
            UpdateState::Available(update()),
            UpdateState::Downloading {
                update: update(),
                progress: gpui_auto_update_core::DownloadProgress {
                    downloaded: 0,
                    total: None
                },
            },
        ]
    );
}

#[test]
fn observers_may_call_back_into_the_coordinator() {
    let coordinator = self_managed(Fixed::new(Ok(CheckOutcome::UpToDate)));
    let seen = Arc::new(Mutex::new(Vec::new()));
    coordinator
        .subscribe({
            let seen = seen.clone();
            let coordinator = coordinator.clone();
            move |state: &UpdateState| {
                seen.lock().unwrap().push(state.clone());
                if *state == UpdateState::UpToDate {
                    coordinator.apply(UpdateEvent::Dismissed).unwrap();
                }
                let _ = coordinator.state();
            }
        })
        .detach();

    coordinator.check(CheckKind::Manual).unwrap();

    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            UpdateState::Checking,
            UpdateState::UpToDate,
            UpdateState::Idle
        ]
    );
}

#[test]
fn a_panicking_source_fails_the_check_without_wedging_the_coordinator() {
    struct PanicsOnce(AtomicUsize);
    impl CheckSource for PanicsOnce {
        fn check(&self, _request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
            if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("source bug");
            }
            Ok(CheckOutcome::UpToDate)
        }
    }
    let coordinator = self_managed(PanicsOnce(AtomicUsize::new(0)));

    let error = coordinator.check(CheckKind::Manual).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Internal);

    assert_eq!(
        coordinator.check(CheckKind::Manual),
        Ok(CheckOutcome::UpToDate)
    );
}

#[test]
fn manual_check_waits_for_a_slow_background_check() {
    let (source, gate) = gated();
    let coordinator = self_managed(source);
    let background = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Background)
    });
    gate.entered.recv().unwrap();

    let manual = thread::spawn({
        let coordinator = coordinator.clone();
        move || coordinator.check(CheckKind::Manual)
    });
    thread::sleep(Duration::from_millis(50));
    assert!(!manual.is_finished());
    wait_for_state(&coordinator, &UpdateState::Checking);

    gate.release.send(Ok(CheckOutcome::UpToDate)).unwrap();
    assert_eq!(manual.join().unwrap(), Ok(CheckOutcome::UpToDate));
    background.join().unwrap().unwrap();
}
