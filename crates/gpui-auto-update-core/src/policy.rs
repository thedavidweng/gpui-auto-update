//! The automatic-check policy: when background checks run, driven by the
//! persisted automatic-update preference.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use crate::check_source::{CheckKind, CheckOutcome};
use crate::coordinator::{Subscription, UpdateCoordinator};
use crate::error::UpdateError;
use crate::preferences::{PreferenceOwner, PreferenceStore, UpdatePreferences};
use crate::state::UpdateState;

/// When automatic (background) checks run.
///
/// [`CheckPolicy::recommended`] is the default desktop behavior: one
/// unobtrusive check near launch when automatic checks are enabled, at most
/// once per [`Self::minimum_interval`], no periodic checks, and a check right
/// after the user enables automatic checks. Every part can be changed with
/// the `with_*` methods.
///
/// Manual checks are never restricted by the policy.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CheckPolicy {
    check_on_launch: bool,
    minimum_interval: Duration,
    periodic_interval: Option<Duration>,
    check_when_enabled: bool,
    automatic_checks_by_default: bool,
}

impl CheckPolicy {
    /// The recommended desktop policy.
    pub fn recommended() -> Self {
        Self {
            check_on_launch: true,
            minimum_interval: Duration::from_secs(60 * 60),
            periodic_interval: None,
            check_when_enabled: true,
            automatic_checks_by_default: true,
        }
    }

    /// Whether a background check runs near application launch.
    pub fn check_on_launch(&self) -> bool {
        self.check_on_launch
    }

    /// Sets whether a background check runs near application launch.
    pub fn with_check_on_launch(mut self, enabled: bool) -> Self {
        self.check_on_launch = enabled;
        self
    }

    /// The shortest time between the end of one check and the start of an
    /// automatic one, so that repeated launches do not repeat checks.
    pub fn minimum_interval(&self) -> Duration {
        self.minimum_interval
    }

    /// Sets the minimum interval between automatic checks.
    pub fn with_minimum_interval(mut self, interval: Duration) -> Self {
        self.minimum_interval = interval;
        self
    }

    /// How often long-running applications check in the background, or
    /// `None` to check only near launch.
    ///
    /// Periodic checks never run more often than [`Self::minimum_interval`].
    pub fn periodic_interval(&self) -> Option<Duration> {
        self.periodic_interval
    }

    /// Sets the periodic check interval; `None` disables periodic checks.
    pub fn with_periodic_interval(mut self, interval: Option<Duration>) -> Self {
        self.periodic_interval = interval;
        self
    }

    /// Whether enabling automatic checks asks for a background check right
    /// away (subject to the minimum interval).
    pub fn check_when_enabled(&self) -> bool {
        self.check_when_enabled
    }

    /// Sets whether enabling automatic checks asks for a background check.
    pub fn with_check_when_enabled(mut self, enabled: bool) -> Self {
        self.check_when_enabled = enabled;
        self
    }

    /// Whether automatic checks are enabled before the user has chosen.
    pub fn automatic_checks_by_default(&self) -> bool {
        self.automatic_checks_by_default
    }

    /// Sets whether automatic checks are enabled before the user has chosen.
    pub fn with_automatic_checks_by_default(mut self, enabled: bool) -> Self {
        self.automatic_checks_by_default = enabled;
        self
    }
}

impl Default for CheckPolicy {
    fn default() -> Self {
        Self::recommended()
    }
}

/// A source of wall-clock time, replaceable in tests.
pub trait Clock: Send + Sync + 'static {
    /// The current time.
    fn now(&self) -> SystemTime;
}

/// The system wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

impl<T: Clock + ?Sized> Clock for Arc<T> {
    fn now(&self) -> SystemTime {
        (**self).now()
    }
}

/// Applies a [`CheckPolicy`] to an [`UpdateCoordinator`] using the
/// automatic-update preference from a [`PreferenceStore`].
///
/// It records when every check through the coordinator finishes, manual
/// ones included, and persists that time so the minimum interval holds
/// across restarts. It does not own timers or threads: the host decides when
/// to call [`Self::run_launch_check`] and how to wait for
/// [`Self::periodic_check_delay`]. The `run_*` methods block for the duration
/// of the check, like [`UpdateCoordinator::check`], so call them off any UI
/// thread. Background failures are logged by the coordinator, not surfaced
/// as [`UpdateState::Failed`].
///
/// When the store's owner is [`PreferenceOwner::Backend`] (Sparkle), the
/// preference is re-read from the store on every query because the backend
/// may change it, and periodic checks are left to the backend's own
/// scheduler.
///
/// Cloning is cheap and every clone refers to the same policy state.
#[derive(Clone)]
pub struct AutomaticChecks {
    coordinator: UpdateCoordinator,
    inner: Arc<Inner>,
    _subscription: Arc<Subscription>,
}

struct Inner {
    store: Box<dyn PreferenceStore>,
    owner: PreferenceOwner,
    clock: Box<dyn Clock>,
    policy: CheckPolicy,
    cache: Mutex<Cache>,
}

struct Cache {
    preferences: UpdatePreferences,
    checking: bool,
}

impl AutomaticChecks {
    /// Creates the policy driver using the system clock.
    ///
    /// Stored preferences are loaded now. When nothing is stored, or the
    /// stored data is unreadable or corrupt, the policy's default applies
    /// and the next save replaces the bad data.
    pub fn new(
        coordinator: UpdateCoordinator,
        store: impl PreferenceStore,
        policy: CheckPolicy,
    ) -> Self {
        Self::with_clock(coordinator, store, policy, SystemClock)
    }

    /// Creates the policy driver with a custom clock.
    pub fn with_clock(
        coordinator: UpdateCoordinator,
        store: impl PreferenceStore,
        policy: CheckPolicy,
        clock: impl Clock,
    ) -> Self {
        let owner = store.owner();
        let defaults = UpdatePreferences::new(policy.automatic_checks_by_default);
        let preferences = load_or(&store, &defaults);
        let inner = Arc::new(Inner {
            store: Box::new(store),
            owner,
            clock: Box::new(clock),
            policy,
            cache: Mutex::new(Cache {
                preferences,
                checking: false,
            }),
        });
        let observer = Arc::clone(&inner);
        let subscription = coordinator.subscribe(move |state| observer.observe(state));
        Self {
            coordinator,
            inner,
            _subscription: Arc::new(subscription),
        }
    }

    /// The coordinator this policy drives.
    pub fn coordinator(&self) -> &UpdateCoordinator {
        &self.coordinator
    }

    /// The policy in effect.
    pub fn policy(&self) -> &CheckPolicy {
        &self.inner.policy
    }

    /// Who persists the automatic-update preference.
    pub fn preference_owner(&self) -> PreferenceOwner {
        self.inner.owner
    }

    /// Whether automatic checks are enabled.
    pub fn automatic_checks_enabled(&self) -> bool {
        self.inner.current().automatic_checks
    }

    /// When the most recent check finished, if one ever has.
    pub fn last_check(&self) -> Option<SystemTime> {
        self.inner.current().last_check
    }

    /// Enables or disables automatic checks and persists the choice.
    ///
    /// Manual checks stay available either way. Returns `true` when the
    /// preference was just turned on and the policy asks for discovery now:
    /// [`CheckPolicy::check_when_enabled`] is set, the installation can
    /// self-update, and the minimum interval has elapsed. The caller should
    /// then start a background check, for example with
    /// `coordinator().check(CheckKind::Background)` on a worker thread.
    ///
    /// On a persistence failure the preference is left unchanged.
    pub fn set_automatic_checks(&self, enabled: bool) -> Result<bool, UpdateError> {
        let inner = &self.inner;
        let can_self_update = self.coordinator.capability().can_self_update();
        let mut cache = inner.lock();
        let previous = inner.refresh(&mut cache);
        let mut next = previous.clone();
        next.automatic_checks = enabled;
        if next != previous {
            inner.store.save(&next)?;
            cache.preferences = next.clone();
        }
        Ok(enabled
            && !previous.automatic_checks
            && inner.policy.check_when_enabled
            && can_self_update
            && inner.interval_elapsed(next.last_check, inner.policy.minimum_interval))
    }

    /// Whether the launch check should run now: launch checks are enabled by
    /// the policy, automatic checks are enabled, the installation can
    /// self-update, and the minimum interval has elapsed.
    pub fn launch_check_due(&self) -> bool {
        let preferences = self.inner.current();
        self.inner.policy.check_on_launch
            && preferences.automatic_checks
            && self.coordinator.capability().can_self_update()
            && self
                .inner
                .interval_elapsed(preferences.last_check, self.inner.policy.minimum_interval)
    }

    /// Runs a background check if [`Self::launch_check_due`], returning its
    /// outcome, or `None` when no check was due.
    pub fn run_launch_check(&self) -> Option<Result<CheckOutcome, UpdateError>> {
        self.launch_check_due()
            .then(|| self.coordinator.check(CheckKind::Background))
    }

    /// How long until the next periodic check is due, or `None` when
    /// periodic checks do not apply: the policy has no periodic interval,
    /// automatic checks are disabled, or the backend schedules them.
    ///
    /// Returns [`Duration::ZERO`] when a check is due now. Hosts typically
    /// wait this long, call [`Self::run_periodic_check`], and repeat; a
    /// `None` means stop until the preference or policy changes.
    pub fn periodic_check_delay(&self) -> Option<Duration> {
        let periodic = self.inner.policy.periodic_interval?;
        if self.inner.owner == PreferenceOwner::Backend {
            return None;
        }
        let preferences = self.inner.current();
        if !preferences.automatic_checks {
            return None;
        }
        let interval = periodic.max(self.inner.policy.minimum_interval);
        let Some(last_check) = preferences.last_check else {
            return Some(Duration::ZERO);
        };
        Some(match self.inner.clock.now().duration_since(last_check) {
            Ok(elapsed) => interval.saturating_sub(elapsed),
            Err(_) => Duration::ZERO,
        })
    }

    /// Runs a background check if a periodic check is due now and the
    /// installation can self-update, returning its outcome, or `None` when
    /// no check ran.
    pub fn run_periodic_check(&self) -> Option<Result<CheckOutcome, UpdateError>> {
        let due = self.periodic_check_delay() == Some(Duration::ZERO)
            && self.coordinator.capability().can_self_update();
        due.then(|| self.coordinator.check(CheckKind::Background))
    }
}

impl std::fmt::Debug for AutomaticChecks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutomaticChecks")
            .field("policy", &self.inner.policy)
            .field("owner", &self.inner.owner)
            .field("preferences", &self.inner.lock().preferences)
            .finish_non_exhaustive()
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn current(&self) -> UpdatePreferences {
        let mut cache = self.lock();
        self.refresh(&mut cache)
    }

    /// Re-reads backend-owned preferences, which can change outside this
    /// library; library-owned preferences are authoritative in the cache.
    fn refresh(&self, cache: &mut Cache) -> UpdatePreferences {
        if self.owner == PreferenceOwner::Backend {
            let defaults = UpdatePreferences::new(self.policy.automatic_checks_by_default);
            cache.preferences = match self.store.load() {
                Ok(Some(preferences)) => preferences,
                Ok(None) => defaults.with_last_check(cache.preferences.last_check),
                Err(error) => {
                    log_load_failure(&error);
                    cache.preferences.clone()
                }
            };
        }
        cache.preferences.clone()
    }

    fn interval_elapsed(&self, last_check: Option<SystemTime>, interval: Duration) -> bool {
        let Some(last_check) = last_check else {
            return true;
        };
        // A last check in the future means the clock moved backwards; treat
        // the interval as elapsed rather than suppressing checks until then.
        match self.clock.now().duration_since(last_check) {
            Ok(elapsed) => elapsed >= interval,
            Err(_) => true,
        }
    }

    fn observe(&self, state: &UpdateState) {
        let mut cache = self.lock();
        if matches!(state, UpdateState::Checking) {
            cache.checking = true;
            return;
        }
        if !std::mem::take(&mut cache.checking) {
            return;
        }
        let next = self
            .refresh(&mut cache)
            .with_last_check(Some(self.clock.now()));
        if let Err(error) = self.store.save(&next) {
            tracing::warn!(
                kind_of_error = ?error.kind(),
                diagnostic = error.diagnostic(),
                "could not persist the last update check time: {error}"
            );
        }
        cache.preferences = next;
    }
}

fn load_or(store: &dyn PreferenceStore, defaults: &UpdatePreferences) -> UpdatePreferences {
    match store.load() {
        Ok(Some(preferences)) => preferences,
        Ok(None) => defaults.clone(),
        Err(error) => {
            log_load_failure(&error);
            defaults.clone()
        }
    }
}

fn log_load_failure(error: &UpdateError) {
    tracing::warn!(
        kind_of_error = ?error.kind(),
        diagnostic = error.diagnostic(),
        "could not load update preferences, using defaults: {error}"
    );
}
