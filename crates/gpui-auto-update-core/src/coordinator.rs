//! The check coordinator: owns the update state, coalesces checks, and
//! notifies observers.

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};

use crate::capability::Capability;
use crate::check_source::{CheckKind, CheckOutcome, CheckRequest, CheckSource};
use crate::error::{ErrorKind, UpdateError};
use crate::state::{UpdateEvent, UpdateState};

type Observer = Arc<dyn Fn(&UpdateState) + Send + Sync>;
type CheckResult = Result<CheckOutcome, UpdateError>;

/// Owns the [`UpdateState`] of one installation and serializes the
/// operations that change it.
///
/// Concurrent checks coalesce: while a check runs, further calls to
/// [`Self::check`] wait for it and receive its result instead of starting
/// another. A manual check that attaches to a background check makes that
/// check's outcome visible: a failure then becomes [`UpdateState::Failed`]
/// instead of being only logged.
///
/// Cloning is cheap and every clone refers to the same coordinator.
#[derive(Clone)]
pub struct UpdateCoordinator {
    inner: Arc<Inner>,
}

struct Inner {
    source: Box<dyn CheckSource>,
    shared: Mutex<Shared>,
}

struct Shared {
    state: UpdateState,
    capability: Capability,
    flight: Option<Arc<Flight>>,
    state_before_check: UpdateState,
    pending: VecDeque<UpdateState>,
    dispatching: bool,
    observers: Vec<(u64, Observer)>,
    next_observer: u64,
}

struct Flight {
    manual: Mutex<bool>,
    result: Mutex<Option<CheckResult>>,
    done: Condvar,
}

impl UpdateCoordinator {
    /// Creates a coordinator that resolves updates with `source`.
    ///
    /// The initial state is [`UpdateState::Idle`] for a self-managed
    /// installation and [`UpdateState::Disabled`] otherwise.
    pub fn new(source: impl CheckSource, capability: Capability) -> Self {
        let state = initial_state(&capability);
        Self {
            inner: Arc::new(Inner {
                source: Box::new(source),
                shared: Mutex::new(Shared {
                    state_before_check: state.clone(),
                    state,
                    capability,
                    flight: None,
                    pending: VecDeque::new(),
                    dispatching: false,
                    observers: Vec::new(),
                    next_observer: 0,
                }),
            }),
        }
    }

    /// The current update state.
    pub fn state(&self) -> UpdateState {
        self.inner.lock().state.clone()
    }

    /// The current update capability.
    pub fn capability(&self) -> Capability {
        self.inner.lock().capability.clone()
    }

    /// Changes the update capability.
    ///
    /// Becoming unable to self-update moves the state to
    /// [`UpdateState::Disabled`]; a running check still completes and
    /// reports its result to its callers. Becoming self-managed again moves a
    /// disabled state to [`UpdateState::Idle`].
    ///
    /// Fails with [`ErrorKind::OperationInProgress`] while a download,
    /// verification, or installation is running.
    pub fn set_capability(&self, capability: Capability) -> Result<(), UpdateError> {
        let mut shared = self.inner.lock();
        if shared.state.is_busy() && !matches!(shared.state, UpdateState::Checking) {
            return Err(UpdateError::new(ErrorKind::OperationInProgress));
        }
        let next = if !capability.can_self_update() {
            Some(UpdateState::Disabled {
                capability: capability.clone(),
            })
        } else if matches!(shared.state, UpdateState::Disabled { .. }) {
            Some(UpdateState::Idle)
        } else {
            None
        };
        shared.capability = capability;
        if let Some(next) = next {
            shared.set_state(next);
        }
        self.dispatch(shared);
        Ok(())
    }

    /// Runs a check, or attaches to the check that is already running, and
    /// returns its outcome.
    ///
    /// This blocks until the check finishes; call it off any UI thread.
    ///
    /// - A manual check always ends in a visible state: [`UpdateState::UpToDate`],
    ///   [`UpdateState::Available`], or [`UpdateState::Failed`].
    /// - A background check that fails is logged with [`tracing`] and the
    ///   state returns to what it was before the check, unless a manual check
    ///   attached in the meantime.
    /// - A check is rejected with the capability's error when the
    ///   installation cannot self-update, with
    ///   [`ErrorKind::OperationInProgress`] while a download or install runs,
    ///   and with [`ErrorKind::InvalidState`] while an update is staged.
    pub fn check(&self, kind: CheckKind) -> Result<CheckOutcome, UpdateError> {
        let mut shared = self.inner.lock();

        if let Some(flight) = shared.flight.clone() {
            // Marked while holding `shared` so the running check, which reads
            // the flag under the same lock, cannot miss it.
            if kind == CheckKind::Manual {
                *lock(&flight.manual) = true;
            }
            drop(shared);
            tracing::debug!(?kind, "attaching to running update check");
            return flight.wait();
        }

        let rejection = shared
            .capability
            .denial()
            .or_else(|| shared.state.check_rejection());
        if let Some(error) = rejection {
            drop(shared);
            tracing::debug!(?kind, kind_of_error = ?error.kind(), "update check rejected");
            return Err(error);
        }

        let flight = Arc::new(Flight {
            manual: Mutex::new(kind == CheckKind::Manual),
            result: Mutex::new(None),
            done: Condvar::new(),
        });
        shared.flight = Some(flight.clone());
        shared.state_before_check = shared.state.clone();
        shared.set_state(UpdateState::Checking);
        self.dispatch(shared);

        let request = CheckRequest::new(kind);
        let result = panic::catch_unwind(AssertUnwindSafe(|| self.inner.source.check(&request)));
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                tracing::error!("update check source panicked");
                Err(UpdateError::new(ErrorKind::Internal)
                    .with_diagnostic("update check source panicked"))
            }
        };

        let mut shared = self.inner.lock();
        shared.flight = None;
        let visible = *lock(&flight.manual);
        if matches!(shared.state, UpdateState::Checking) {
            let next = match &result {
                Ok(CheckOutcome::UpToDate) => UpdateState::UpToDate,
                Ok(CheckOutcome::UpdateAvailable(update)) => UpdateState::Available(update.clone()),
                Err(error) if visible => {
                    tracing::info!(
                        kind_of_error = ?error.kind(),
                        diagnostic = error.diagnostic(),
                        "manual update check failed: {error}"
                    );
                    UpdateState::Failed(error.clone())
                }
                Err(error) => {
                    tracing::warn!(
                        kind_of_error = ?error.kind(),
                        diagnostic = error.diagnostic(),
                        "background update check failed: {error}"
                    );
                    shared.state_before_check.clone()
                }
            };
            shared.set_state(next);
        }
        *lock(&flight.result) = Some(result.clone());
        flight.done.notify_all();
        self.dispatch(shared);

        result
    }

    /// Applies a backend-reported step to the state.
    ///
    /// Fails without changing the state when the event is incompatible with
    /// the current state; see [`UpdateState::apply`].
    pub fn apply(&self, event: UpdateEvent) -> Result<(), UpdateError> {
        let mut shared = self.inner.lock();
        let next = shared.state.apply(event)?;
        shared.set_state(next);
        self.dispatch(shared);
        Ok(())
    }

    /// Calls `observer` with every new state, in order, until the returned
    /// [`Subscription`] is dropped.
    ///
    /// Observers run on whichever thread changed the state. They may call
    /// back into the coordinator; resulting changes are delivered after the
    /// current notification returns.
    pub fn subscribe(
        &self,
        observer: impl Fn(&UpdateState) + Send + Sync + 'static,
    ) -> Subscription {
        let mut shared = self.inner.lock();
        let id = shared.next_observer;
        shared.next_observer += 1;
        shared.observers.push((id, Arc::new(observer)));
        Subscription {
            inner: Arc::downgrade(&self.inner),
            id: Some(id),
        }
    }

    /// Delivers pending state notifications unless another thread already is.
    fn dispatch(&self, mut shared: MutexGuard<'_, Shared>) {
        if shared.dispatching {
            return;
        }
        shared.dispatching = true;
        let mut guard = DispatchGuard {
            inner: &self.inner,
            shared: Some(shared),
        };
        while let Some(state) = guard.shared().pending.pop_front() {
            let observers: Vec<Observer> = guard
                .shared()
                .observers
                .iter()
                .map(|(_, observer)| observer.clone())
                .collect();
            guard.shared = None;
            for observer in observers {
                observer(&state);
            }
            guard.shared = Some(self.inner.lock());
        }
    }
}

impl std::fmt::Debug for UpdateCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let shared = self.inner.lock();
        f.debug_struct("UpdateCoordinator")
            .field("state", &shared.state)
            .field("capability", &shared.capability)
            .finish_non_exhaustive()
    }
}

/// Keeps an observer registered; dropping it unregisters the observer.
#[must_use = "dropping a Subscription unregisters its observer"]
pub struct Subscription {
    inner: Weak<Inner>,
    id: Option<u64>,
}

impl Subscription {
    /// Keeps the observer registered for the coordinator's lifetime.
    pub fn detach(mut self) {
        self.id = None;
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let (Some(id), Some(inner)) = (self.id, self.inner.upgrade()) {
            inner.lock().observers.retain(|(other, _)| *other != id);
        }
    }
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription").finish_non_exhaustive()
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        lock(&self.shared)
    }
}

impl Shared {
    fn set_state(&mut self, state: UpdateState) {
        if self.state != state {
            self.state = state.clone();
            self.pending.push_back(state);
        }
    }
}

impl Flight {
    fn wait(&self) -> CheckResult {
        let mut result = lock(&self.result);
        loop {
            if let Some(result) = result.as_ref() {
                return result.clone();
            }
            result = self
                .done
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Clears the dispatching flag even if an observer panics, so later
/// notifications are not lost.
struct DispatchGuard<'a> {
    inner: &'a Inner,
    shared: Option<MutexGuard<'a, Shared>>,
}

impl<'a> DispatchGuard<'a> {
    fn shared(&mut self) -> &mut MutexGuard<'a, Shared> {
        self.shared.get_or_insert_with(|| self.inner.lock())
    }
}

impl Drop for DispatchGuard<'_> {
    fn drop(&mut self) {
        self.shared().dispatching = false;
    }
}

fn initial_state(capability: &Capability) -> UpdateState {
    if capability.can_self_update() {
        UpdateState::Idle
    } else {
        UpdateState::Disabled {
            capability: capability.clone(),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
