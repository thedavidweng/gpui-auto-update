//! Behavioral tests for the automatic-check policy.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui_auto_update_core::{
    AutomaticChecks, Capability, CheckKind, CheckOutcome, CheckPolicy, CheckRequest, CheckSource,
    Clock, ErrorKind, FilePreferenceStore, MemoryPreferenceStore, PreferenceOwner, PreferenceStore,
    UpdateCoordinator, UpdateError, UpdatePreferences, UpdateState,
};

const HOUR: Duration = Duration::from_secs(60 * 60);

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

const START: u64 = 1_700_000_000;

#[derive(Clone)]
struct TestClock(Arc<Mutex<SystemTime>>);

impl TestClock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(at(START))))
    }
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
    fn rewind(&self, by: Duration) {
        *self.0.lock().unwrap() -= by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

#[derive(Clone, Default)]
struct Source {
    calls: Arc<AtomicUsize>,
    fail: bool,
}

impl Source {
    fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl CheckSource for Source {
    fn check(&self, _request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(UpdateError::new(ErrorKind::FeedRetrieval))
        } else {
            Ok(CheckOutcome::UpToDate)
        }
    }
}

struct Harness {
    source: Source,
    clock: TestClock,
    store: MemoryPreferenceStore,
    checks: AutomaticChecks,
}

impl Harness {
    fn new(policy: CheckPolicy) -> Self {
        Self::with(Source::default(), MemoryPreferenceStore::new(), policy)
    }

    fn with(source: Source, store: MemoryPreferenceStore, policy: CheckPolicy) -> Self {
        let clock = TestClock::new();
        let coordinator = UpdateCoordinator::new(source.clone(), Capability::SelfManaged);
        let checks = AutomaticChecks::with_clock(coordinator, store.clone(), policy, clock.clone());
        Self {
            source,
            clock,
            store,
            checks,
        }
    }
}

#[test]
fn the_recommended_policy_checks_near_launch_with_a_minimum_interval_and_no_periodic_checks() {
    let policy = CheckPolicy::recommended();

    assert!(policy.check_on_launch());
    assert_eq!(policy.minimum_interval(), HOUR);
    assert_eq!(policy.periodic_interval(), None);
    assert!(policy.check_when_enabled());
    assert!(policy.automatic_checks_by_default());
    assert_eq!(CheckPolicy::default(), policy);
}

#[test]
fn a_first_launch_runs_a_background_check_and_persists_its_time() {
    let h = Harness::new(CheckPolicy::recommended());

    assert!(h.checks.launch_check_due());
    let outcome = h.checks.run_launch_check();

    assert_eq!(outcome, Some(Ok(CheckOutcome::UpToDate)));
    assert_eq!(h.source.calls(), 1);
    assert_eq!(h.checks.last_check(), Some(at(START)));
    assert_eq!(
        h.store.load().unwrap(),
        Some(UpdatePreferences::new(true).with_last_check(Some(at(START))))
    );
}

#[test]
fn relaunching_within_the_minimum_interval_skips_the_launch_check() {
    let store = MemoryPreferenceStore::new();
    store
        .save(&UpdatePreferences::new(true).with_last_check(Some(at(START) - HOUR / 2)))
        .unwrap();
    let h = Harness::with(Source::default(), store, CheckPolicy::recommended());

    assert!(!h.checks.launch_check_due());
    assert_eq!(h.checks.run_launch_check(), None);
    assert_eq!(h.source.calls(), 0);

    h.clock.advance(HOUR / 2);
    assert!(h.checks.launch_check_due());
    assert_eq!(
        h.checks.run_launch_check(),
        Some(Ok(CheckOutcome::UpToDate))
    );
    assert_eq!(h.source.calls(), 1);
}

#[test]
fn the_minimum_interval_is_configurable() {
    let store = MemoryPreferenceStore::new();
    store
        .save(&UpdatePreferences::new(true).with_last_check(Some(at(START) - 3 * HOUR)))
        .unwrap();
    let policy = CheckPolicy::recommended().with_minimum_interval(24 * HOUR);
    let h = Harness::with(Source::default(), store, policy);

    assert!(!h.checks.launch_check_due());
}

#[test]
fn a_last_check_in_the_future_does_not_suppress_checks_forever() {
    let h = Harness::new(CheckPolicy::recommended());
    h.checks.run_launch_check();

    h.clock.rewind(30 * 24 * HOUR);

    assert!(h.checks.launch_check_due());
}

#[test]
fn launch_checks_can_be_turned_off_by_policy() {
    let h = Harness::new(CheckPolicy::recommended().with_check_on_launch(false));

    assert!(!h.checks.launch_check_due());
    assert_eq!(h.checks.run_launch_check(), None);
    assert_eq!(h.source.calls(), 0);
}

#[test]
fn disabled_automatic_checks_skip_the_launch_check_but_not_manual_checks() {
    let h = Harness::new(CheckPolicy::recommended());

    let check_now = h.checks.set_automatic_checks(false).unwrap();

    assert!(!check_now);
    assert!(!h.checks.automatic_checks_enabled());
    assert!(!h.checks.launch_check_due());
    assert_eq!(h.checks.run_launch_check(), None);
    assert_eq!(
        h.checks.coordinator().check(CheckKind::Manual),
        Ok(CheckOutcome::UpToDate)
    );
    assert_eq!(h.source.calls(), 1);
    assert_eq!(h.checks.coordinator().state(), UpdateState::UpToDate);
}

#[test]
fn manual_checks_also_record_the_last_check_time() {
    let h = Harness::new(CheckPolicy::recommended());

    h.checks.coordinator().check(CheckKind::Manual).unwrap();

    assert_eq!(h.checks.last_check(), Some(at(START)));
    assert!(!h.checks.launch_check_due());
}

#[test]
fn the_default_preference_comes_from_the_policy() {
    let h = Harness::new(CheckPolicy::recommended().with_automatic_checks_by_default(false));

    assert!(!h.checks.automatic_checks_enabled());
    assert!(!h.checks.launch_check_due());
}

#[test]
fn enabling_automatic_checks_asks_for_discovery_when_the_interval_allows() {
    let h = Harness::new(CheckPolicy::recommended().with_automatic_checks_by_default(false));

    let check_now = h.checks.set_automatic_checks(true).unwrap();

    assert!(check_now);
    assert!(h.checks.automatic_checks_enabled());
}

#[test]
fn enabling_automatic_checks_right_after_a_check_does_not_ask_for_another() {
    let h = Harness::new(CheckPolicy::recommended().with_automatic_checks_by_default(false));
    h.checks.coordinator().check(CheckKind::Manual).unwrap();

    assert!(!h.checks.set_automatic_checks(true).unwrap());
}

#[test]
fn enabling_without_check_when_enabled_never_asks_for_discovery() {
    let h = Harness::new(
        CheckPolicy::recommended()
            .with_automatic_checks_by_default(false)
            .with_check_when_enabled(false),
    );

    assert!(!h.checks.set_automatic_checks(true).unwrap());
}

#[test]
fn re_enabling_an_already_enabled_preference_does_not_ask_for_discovery() {
    let h = Harness::new(CheckPolicy::recommended());

    assert!(!h.checks.set_automatic_checks(true).unwrap());
}

#[test]
fn the_preference_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.json");
    let launch = || {
        let coordinator = UpdateCoordinator::new(Source::default(), Capability::SelfManaged);
        AutomaticChecks::with_clock(
            coordinator,
            FilePreferenceStore::new(&path),
            CheckPolicy::recommended(),
            TestClock::new(),
        )
    };

    let first = launch();
    first.set_automatic_checks(false).unwrap();
    first.coordinator().check(CheckKind::Manual).unwrap();
    drop(first);

    let second = launch();
    assert!(!second.automatic_checks_enabled());
    assert_eq!(second.last_check(), Some(at(START)));
}

#[test]
fn a_corrupt_preference_file_falls_back_to_the_policy_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updates.json");
    std::fs::write(&path, b"not json").unwrap();
    let coordinator = UpdateCoordinator::new(Source::default(), Capability::SelfManaged);
    let checks = AutomaticChecks::with_clock(
        coordinator,
        FilePreferenceStore::new(&path),
        CheckPolicy::recommended(),
        TestClock::new(),
    );

    assert!(checks.automatic_checks_enabled());
    assert!(checks.launch_check_due());

    checks.set_automatic_checks(false).unwrap();
    assert_eq!(
        FilePreferenceStore::new(&path).load().unwrap(),
        Some(UpdatePreferences::new(false))
    );
}

struct BrokenStore;

impl PreferenceStore for BrokenStore {
    fn load(&self) -> Result<Option<UpdatePreferences>, UpdateError> {
        Ok(None)
    }
    fn save(&self, _preferences: &UpdatePreferences) -> Result<(), UpdateError> {
        Err(UpdateError::new(ErrorKind::Preferences))
    }
}

#[test]
fn a_preference_that_cannot_be_saved_is_reported_and_left_unchanged() {
    let coordinator = UpdateCoordinator::new(Source::default(), Capability::SelfManaged);
    let checks = AutomaticChecks::with_clock(
        coordinator,
        BrokenStore,
        CheckPolicy::recommended(),
        TestClock::new(),
    );

    let error = checks.set_automatic_checks(false).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::Preferences);
    assert!(checks.automatic_checks_enabled());
}

#[test]
fn a_failing_last_check_save_does_not_surface_from_a_check() {
    let coordinator = UpdateCoordinator::new(Source::default(), Capability::SelfManaged);
    let checks = AutomaticChecks::with_clock(
        coordinator,
        BrokenStore,
        CheckPolicy::recommended(),
        TestClock::new(),
    );

    assert_eq!(checks.run_launch_check(), Some(Ok(CheckOutcome::UpToDate)));
    assert_eq!(checks.coordinator().state(), UpdateState::UpToDate);
}

#[test]
fn a_failed_launch_check_is_not_surfaced_but_still_counts_toward_the_interval() {
    let h = Harness::with(
        Source::failing(),
        MemoryPreferenceStore::new(),
        CheckPolicy::recommended(),
    );

    let outcome = h.checks.run_launch_check();

    assert_eq!(
        outcome.map(|result| result.map_err(|error| error.kind())),
        Some(Err(ErrorKind::FeedRetrieval))
    );
    assert_eq!(h.checks.coordinator().state(), UpdateState::Idle);
    assert_eq!(h.checks.last_check(), Some(at(START)));
    assert!(!h.checks.launch_check_due());
}

#[test]
fn installations_that_cannot_self_update_never_run_automatic_checks() {
    let source = Source::default();
    let coordinator = UpdateCoordinator::new(
        source.clone(),
        Capability::ExternallyManaged { manager: None },
    );
    let checks = AutomaticChecks::with_clock(
        coordinator,
        MemoryPreferenceStore::new(),
        CheckPolicy::recommended().with_periodic_interval(Some(HOUR)),
        TestClock::new(),
    );

    assert!(!checks.launch_check_due());
    assert_eq!(checks.run_launch_check(), None);
    assert_eq!(checks.run_periodic_check(), None);
    assert!(!checks.set_automatic_checks(true).unwrap());
    assert_eq!(source.calls(), 0);
}

#[test]
fn periodic_checks_are_off_by_default() {
    let h = Harness::new(CheckPolicy::recommended());

    assert_eq!(h.checks.periodic_check_delay(), None);
    assert_eq!(h.checks.run_periodic_check(), None);
}

#[test]
fn periodic_checks_run_once_the_interval_has_elapsed() {
    let h = Harness::new(CheckPolicy::recommended().with_periodic_interval(Some(4 * HOUR)));

    assert_eq!(h.checks.periodic_check_delay(), Some(Duration::ZERO));
    assert_eq!(
        h.checks.run_periodic_check(),
        Some(Ok(CheckOutcome::UpToDate))
    );
    assert_eq!(h.checks.periodic_check_delay(), Some(4 * HOUR));

    h.clock.advance(HOUR);
    assert_eq!(h.checks.periodic_check_delay(), Some(3 * HOUR));
    assert_eq!(h.checks.run_periodic_check(), None);

    h.clock.advance(3 * HOUR);
    assert_eq!(
        h.checks.run_periodic_check(),
        Some(Ok(CheckOutcome::UpToDate))
    );
    assert_eq!(h.source.calls(), 2);
}

#[test]
fn periodic_checks_never_run_more_often_than_the_minimum_interval() {
    let h = Harness::new(
        CheckPolicy::recommended()
            .with_minimum_interval(2 * HOUR)
            .with_periodic_interval(Some(Duration::from_secs(60))),
    );
    h.checks.run_periodic_check();

    assert_eq!(h.checks.periodic_check_delay(), Some(2 * HOUR));
}

#[test]
fn periodic_checks_stop_while_automatic_checks_are_disabled() {
    let h = Harness::new(CheckPolicy::recommended().with_periodic_interval(Some(HOUR)));

    h.checks.set_automatic_checks(false).unwrap();

    assert_eq!(h.checks.periodic_check_delay(), None);
    assert_eq!(h.checks.run_periodic_check(), None);
    assert_eq!(h.source.calls(), 0);
}

#[test]
fn backend_owned_preferences_are_read_through_and_leave_periodic_scheduling_to_the_backend() {
    let store = MemoryPreferenceStore::new().with_owner(PreferenceOwner::Backend);
    let h = Harness::with(
        Source::default(),
        store.clone(),
        CheckPolicy::recommended().with_periodic_interval(Some(HOUR)),
    );

    assert_eq!(h.checks.preference_owner(), PreferenceOwner::Backend);
    assert_eq!(h.checks.periodic_check_delay(), None);

    store.save(&UpdatePreferences::new(false)).unwrap();
    assert!(!h.checks.automatic_checks_enabled());
    assert!(!h.checks.launch_check_due());

    h.checks.set_automatic_checks(true).unwrap();
    assert_eq!(store.load().unwrap(), Some(UpdatePreferences::new(true)));
}
