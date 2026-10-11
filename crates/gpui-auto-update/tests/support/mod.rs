//! Fakes for facade tests: a scripted check source and install backend that
//! record whether they ran on the GPUI foreground thread.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui::{App, BackgroundExecutor, Entity, Subscription, TestAppContext};
use gpui_auto_update::core::{
    AvailableUpdate, Capability, CheckOutcome, CheckPolicy, CheckRequest, CheckSource, Clock,
    DownloadProgress, ErrorKind, MemoryPreferenceStore, UpdateError, UpdateEvent, UpdateState,
};
use gpui_auto_update::{
    BuildProfile, Handoff, ProgressSink, UpdateBackend, Updater, UpdaterConfig, UpdaterEvent,
};

pub const APP_ID: &str = "dev.example.facade-tests";

pub fn update(version: &str) -> AvailableUpdate {
    AvailableUpdate::new(version)
}

/// A check source that returns scripted outcomes and records each call.
#[derive(Clone)]
pub struct FakeSource {
    inner: Arc<Mutex<SourceState>>,
    executor: BackgroundExecutor,
}

struct SourceState {
    outcomes: Vec<Result<CheckOutcome, UpdateError>>,
    calls: Vec<(CheckRequest, bool)>,
}

impl FakeSource {
    pub fn new(cx: &TestAppContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SourceState {
                outcomes: Vec::new(),
                calls: Vec::new(),
            })),
            executor: cx.executor(),
        }
    }

    /// Queues the outcome of the next check. When the queue is empty, checks
    /// report that the installation is up to date.
    pub fn push(&self, outcome: Result<CheckOutcome, UpdateError>) -> &Self {
        self.inner.lock().unwrap().outcomes.push(outcome);
        self
    }

    pub fn available(&self, version: &str) -> &Self {
        self.push(Ok(CheckOutcome::UpdateAvailable(update(version))))
    }

    pub fn calls(&self) -> usize {
        self.inner.lock().unwrap().calls.len()
    }

    pub fn requests(&self) -> Vec<CheckRequest> {
        self.inner
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|(request, _)| request.clone())
            .collect()
    }

    /// Whether any call ran on the GPUI foreground (main) thread.
    pub fn ran_on_main_thread(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(_, main)| *main)
    }
}

impl CheckSource for FakeSource {
    fn check(&self, request: &CheckRequest) -> Result<CheckOutcome, UpdateError> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .calls
            .push((request.clone(), self.executor.is_main_thread()));
        if inner.outcomes.is_empty() {
            Ok(CheckOutcome::UpToDate)
        } else {
            inner.outcomes.remove(0)
        }
    }
}

/// Something the fake backend was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackendCall {
    Capability,
    Stage(String),
    Install(String),
    Relaunch,
    SetChannel(Option<String>),
    TakePreviousFailure,
    ConfirmStartup,
}

/// An install backend with scripted results that records each call.
#[derive(Clone)]
pub struct FakeBackend {
    inner: Arc<Mutex<BackendState>>,
    executor: BackgroundExecutor,
}

struct BackendState {
    capability: Capability,
    progress: Vec<DownloadProgress>,
    stage_result: Result<(), UpdateError>,
    handoff: Result<Handoff, UpdateError>,
    channel: Option<String>,
    reporting: Reporting,
    previous_failure: Option<UpdateError>,
    confirm_result: Result<(), UpdateError>,
    calls: Vec<(BackendCall, bool)>,
}

#[derive(Clone, Copy)]
enum Reporting {
    Sink,
    Coordinator,
    Silent,
}

impl FakeBackend {
    pub fn new(cx: &TestAppContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BackendState {
                capability: Capability::SelfManaged,
                progress: Vec::new(),
                stage_result: Ok(()),
                handoff: Ok(Handoff::Restart { restart_path: None }),
                channel: None,
                reporting: Reporting::Sink,
                previous_failure: None,
                confirm_result: Ok(()),
                calls: Vec::new(),
            })),
            executor: cx.executor(),
        }
    }

    pub fn with_capability(self, capability: Capability) -> Self {
        self.inner.lock().unwrap().capability = capability;
        self
    }

    /// Progress reported while staging, in order.
    pub fn with_progress(self, progress: Vec<DownloadProgress>) -> Self {
        self.inner.lock().unwrap().progress = progress;
        self
    }

    pub fn with_stage_result(self, result: Result<(), UpdateError>) -> Self {
        self.inner.lock().unwrap().stage_result = result;
        self
    }

    pub fn with_handoff(self, handoff: Result<Handoff, UpdateError>) -> Self {
        self.inner.lock().unwrap().handoff = handoff;
        self
    }

    /// Reports by applying events to the coordinator directly, as the
    /// core's `ArtifactDownloader::download_and_stage` does.
    pub fn reporting_through_coordinator(self) -> Self {
        self.inner.lock().unwrap().reporting = Reporting::Coordinator;
        self
    }

    /// Reports no progress at all.
    pub fn silent(self) -> Self {
        self.inner.lock().unwrap().reporting = Reporting::Silent;
        self
    }

    /// What the backend reports about the previous update attempt.
    pub fn with_previous_failure(self, error: UpdateError) -> Self {
        self.inner.lock().unwrap().previous_failure = Some(error);
        self
    }

    pub fn with_confirm_result(self, result: Result<(), UpdateError>) -> Self {
        self.inner.lock().unwrap().confirm_result = result;
        self
    }

    pub fn with_channel(self, channel: Option<&str>) -> Self {
        self.inner.lock().unwrap().channel = channel.map(str::to_owned);
        self
    }

    pub fn calls(&self) -> Vec<BackendCall> {
        self.inner
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|(call, _)| call.clone())
            .collect()
    }

    /// Calls other than the queries made at startup.
    pub fn operations(&self) -> Vec<BackendCall> {
        self.calls()
            .into_iter()
            .filter(|call| {
                !matches!(
                    call,
                    BackendCall::Capability | BackendCall::TakePreviousFailure
                )
            })
            .collect()
    }

    pub fn ran_on_main_thread(&self) -> bool {
        self.inner
            .lock()
            .unwrap()
            .calls
            .iter()
            .any(|(_, main)| *main)
    }

    fn record(&self, call: BackendCall) -> std::sync::MutexGuard<'_, BackendState> {
        let mut inner = self.inner.lock().unwrap();
        inner.calls.push((call, self.executor.is_main_thread()));
        inner
    }
}

impl UpdateBackend for FakeBackend {
    fn capability(&self) -> Capability {
        self.record(BackendCall::Capability).capability.clone()
    }

    fn stage(&self, update: &AvailableUpdate, progress: &ProgressSink) -> Result<(), UpdateError> {
        let (steps, result, reporting) = {
            let inner = self.record(BackendCall::Stage(update.version.clone()));
            (
                inner.progress.clone(),
                inner.stage_result.clone(),
                inner.reporting,
            )
        };
        match reporting {
            Reporting::Sink => {}
            Reporting::Silent => return result,
            Reporting::Coordinator => {
                let coordinator = progress.coordinator();
                coordinator.apply(UpdateEvent::DownloadStarted { total: Some(10) })?;
                coordinator.apply(UpdateEvent::Staged)?;
                return result;
            }
        }
        let total = steps.first().and_then(|step| step.total);
        progress.download_started(total);
        for step in steps {
            progress.download_progressed(step);
        }
        progress.verification_started();
        result
    }

    fn install(
        &self,
        update: &AvailableUpdate,
        _progress: &ProgressSink,
    ) -> Result<Handoff, UpdateError> {
        self.record(BackendCall::Install(update.version.clone()))
            .handoff
            .clone()
    }

    fn relaunch(&self) -> Result<Handoff, UpdateError> {
        drop(self.record(BackendCall::Relaunch));
        Ok(Handoff::Restart { restart_path: None })
    }

    fn take_previous_failure(&self) -> Option<UpdateError> {
        self.record(BackendCall::TakePreviousFailure)
            .previous_failure
            .take()
    }

    fn confirm_startup(&self) -> Result<(), UpdateError> {
        self.record(BackendCall::ConfirmStartup)
            .confirm_result
            .clone()
    }

    fn channel(&self) -> Option<gpui_auto_update::core::Channel> {
        self.inner
            .lock()
            .unwrap()
            .channel
            .clone()
            .map(gpui_auto_update::core::Channel::new)
    }

    fn set_channel(
        &self,
        channel: Option<gpui_auto_update::core::Channel>,
    ) -> Result<(), UpdateError> {
        let name = channel.map(|channel| channel.as_str().to_owned());
        let mut inner = self.record(BackendCall::SetChannel(name.clone()));
        inner.channel = name;
        Ok(())
    }
}

/// A clock the test moves by hand.
#[derive(Clone)]
pub struct ManualClock(Arc<Mutex<SystemTime>>);

impl ManualClock {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        )))
    }

    pub fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.0.lock().unwrap()
    }
}

/// A configuration with no launch check, in-memory preferences, and the
/// debug-build guard lifted, so tests opt into each behavior explicitly.
pub fn config(source: &FakeSource, backend: &FakeBackend) -> UpdaterConfig {
    UpdaterConfig::new(APP_ID, source.clone())
        .with_backend(backend.clone())
        .with_preferences(MemoryPreferenceStore::new())
        .with_policy(CheckPolicy::recommended().with_check_on_launch(false))
        .with_build_profile(BuildProfile::Release)
}

/// Records every state the updater notifies observers about, and every
/// event it emits.
pub struct Recorder {
    pub states: Arc<Mutex<Vec<UpdateState>>>,
    pub events: Arc<Mutex<Vec<UpdaterEvent>>>,
    _subscriptions: Vec<Subscription>,
}

impl Recorder {
    pub fn new(updater: &Entity<Updater>, cx: &mut TestAppContext) -> Self {
        let states = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriptions = cx.update(|cx: &mut App| {
            let observed = states.clone();
            let emitted = events.clone();
            let mut last = updater.read(cx).state();
            vec![
                cx.observe(updater, move |updater, cx| {
                    let state = updater.read(cx).state();
                    if state != last {
                        last = state.clone();
                        observed.lock().unwrap().push(state);
                    }
                }),
                cx.subscribe(updater, move |_, event: &UpdaterEvent, _| {
                    emitted.lock().unwrap().push(event.clone());
                }),
            ]
        });
        Self {
            states,
            events,
            _subscriptions: subscriptions,
        }
    }

    pub fn states(&self) -> Vec<UpdateState> {
        self.states.lock().unwrap().clone()
    }

    pub fn events(&self) -> Vec<UpdaterEvent> {
        self.events.lock().unwrap().clone()
    }
}

pub fn state(updater: &Entity<Updater>, cx: &mut TestAppContext) -> UpdateState {
    updater.read_with(cx, |updater, _| updater.state())
}

pub fn error(kind: ErrorKind) -> UpdateError {
    UpdateError::new(kind)
}

pub fn restart_path(path: &str) -> Handoff {
    Handoff::Restart {
        restart_path: Some(PathBuf::from(path)),
    }
}
