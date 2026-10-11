//! The facade on macOS with the Sparkle backend, driven by a scripted
//! Sparkle engine (no framework needed).

#![cfg(target_os = "macos")]

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use gpui::TestAppContext;
use gpui_auto_update::core::{
    AvailableUpdate, CheckKind, CheckOutcome, ErrorKind, PreferenceOwner, UpdateError, UpdateState,
};
use gpui_auto_update::macos::{
    RelaunchContinuation, SparkleBackend, SparkleEngine, SparkleEvent, SparkleEvents, SparkleUpdate,
};
use gpui_auto_update::{BuildProfile, Handoff, UpdaterConfig, UpdaterEvent};

use support::{APP_ID, Recorder, state};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    StandardUi,
    Background,
    SetAutomatic(bool),
}

/// Publishes the next scripted events, synchronously, whenever Sparkle is
/// asked to check or present an update.
#[derive(Clone)]
struct ScriptedSparkle {
    events: SparkleEvents,
    state: Arc<Mutex<Script>>,
}

struct Script {
    calls: Vec<Call>,
    replies: Vec<Vec<SparkleEvent>>,
    automatic: bool,
    session_in_progress: bool,
}

impl ScriptedSparkle {
    fn new(events: &SparkleEvents, automatic: bool) -> Self {
        Self {
            events: events.clone(),
            state: Arc::new(Mutex::new(Script {
                calls: Vec::new(),
                replies: Vec::new(),
                automatic,
                session_in_progress: false,
            })),
        }
    }

    /// Whether Sparkle reports an update session as running.
    fn set_session_in_progress(&self, in_progress: bool) {
        self.state.lock().unwrap().session_in_progress = in_progress;
    }

    fn reply(&self, events: Vec<SparkleEvent>) -> &Self {
        self.state.lock().unwrap().replies.push(events);
        self
    }

    fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    fn run(&self, call: Call) -> Result<(), UpdateError> {
        let reply = {
            let mut state = self.state.lock().unwrap();
            state.calls.push(call);
            if state.replies.is_empty() {
                Vec::new()
            } else {
                state.replies.remove(0)
            }
        };
        for event in reply {
            self.events.publish(event);
        }
        Ok(())
    }
}

impl SparkleEngine for ScriptedSparkle {
    fn check_for_updates(&self) -> Result<(), UpdateError> {
        self.run(Call::StandardUi)
    }
    fn check_for_updates_in_background(&self) -> Result<(), UpdateError> {
        self.run(Call::Background)
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
        Ok(None)
    }
    fn allowed_channels(&self) -> Result<Option<Vec<String>>, UpdateError> {
        Ok(None)
    }
    fn set_allowed_channels(&self, _: Option<Vec<String>>) -> Result<(), UpdateError> {
        Ok(())
    }
}

fn sparkle(automatic: bool) -> (SparkleBackend, ScriptedSparkle) {
    let events = SparkleEvents::new();
    let engine = ScriptedSparkle::new(&events, automatic);
    (SparkleBackend::new(engine.clone(), events), engine)
}

fn config(backend: SparkleBackend) -> UpdaterConfig {
    UpdaterConfig::for_sparkle(APP_ID, backend).with_build_profile(BuildProfile::Release)
}

#[gpui::test]
fn check_for_updates_action_opens_sparkles_standard_ui(cx: &mut TestAppContext) {
    let (backend, engine) = sparkle(true);
    engine.reply(vec![SparkleEvent::UpdateFound(SparkleUpdate::new("2.0.0"))]);
    let updater = cx.update(|cx| gpui_auto_update::init(config(backend), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);

    cx.update(|cx| cx.dispatch_action(&gpui_auto_update::CheckForUpdates));
    cx.run_until_parked();

    assert_eq!(engine.calls(), vec![Call::StandardUi]);
    let update = AvailableUpdate::new("2.0.0");
    assert_eq!(state(&updater, cx), UpdateState::Available(update.clone()));
    assert!(recorder.events().contains(&UpdaterEvent::CheckFinished {
        kind: CheckKind::Manual,
        result: Ok(CheckOutcome::UpdateAvailable(update)),
    }));
}

#[gpui::test]
fn sparkle_owns_the_automatic_update_preference_and_schedule(cx: &mut TestAppContext) {
    let (backend, engine) = sparkle(false);
    let updater = cx.update(|cx| gpui_auto_update::init(config(backend), cx));
    cx.run_until_parked();

    assert_eq!(
        updater.read_with(cx, |u, _| u.preference_owner()),
        Some(PreferenceOwner::Backend)
    );
    assert!(!updater.read_with(cx, |u, _| u.automatic_checks_enabled()));

    updater
        .update(cx, |u, cx| u.set_automatic_checks(true, cx))
        .unwrap();
    cx.run_until_parked();

    assert!(updater.read_with(cx, |u, _| u.automatic_checks_enabled()));
    // Sparkle schedules checks itself: enabling the preference and starting
    // up never trigger a check from the facade.
    assert_eq!(engine.calls(), vec![Call::SetAutomatic(true)]);
}

#[gpui::test]
fn installing_goes_through_sparkle_which_owns_the_relaunch(cx: &mut TestAppContext) {
    let (backend, engine) = sparkle(true);
    engine
        .reply(vec![SparkleEvent::UpdateFound(SparkleUpdate::new("2.0.0"))])
        .reply(vec![
            SparkleEvent::WillDownload {
                version: "2.0.0".into(),
            },
            SparkleEvent::Extracted {
                version: "2.0.0".into(),
            },
        ]);
    let updater = cx.update(|cx| gpui_auto_update::init(config(backend), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    cx.update(|cx| cx.dispatch_action(&gpui_auto_update::CheckForUpdates));
    cx.run_until_parked();

    cx.update(|cx| cx.dispatch_action(&gpui_auto_update::InstallUpdate));
    cx.run_until_parked();
    let update = AvailableUpdate::new("2.0.0");
    assert_eq!(state(&updater, cx), UpdateState::Staged(update.clone()));

    cx.update(|cx| cx.dispatch_action(&gpui_auto_update::RestartToUpdate));
    cx.run_until_parked();

    assert_eq!(
        engine.calls(),
        vec![Call::StandardUi, Call::StandardUi, Call::StandardUi]
    );
    assert_eq!(state(&updater, cx), UpdateState::Installing(update));
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::Handoff(Handoff::BackendOwned))
    );
}

/// Pumps the executors until `condition` holds, while background threads
/// (the backend's tracker and gate) deliver their messages.
fn wait_until(cx: &mut TestAppContext, mut condition: impl FnMut(&mut TestAppContext) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        if condition(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "condition never held");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[gpui::test]
fn a_scheduled_discovery_surfaces_as_available_without_touching_sparkles_ui(
    cx: &mut TestAppContext,
) {
    let (backend, engine) = sparkle(true);
    let updater = cx.update(|cx| gpui_auto_update::init(config(backend.clone()), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    // Sparkle's scheduler found an update outside any facade check and is
    // holding the session open as a gentle reminder, with its window
    // suppressed.
    engine.set_session_in_progress(true);

    backend
        .events()
        .publish(SparkleEvent::UpdateFound(SparkleUpdate::new("3.0")));

    wait_until(cx, |cx| {
        state(&updater, cx) == UpdateState::Available(AvailableUpdate::new("3.0"))
    });
    assert_eq!(
        state(&updater, cx),
        UpdateState::Available(AvailableUpdate::new("3.0"))
    );
    assert_eq!(
        engine.calls(),
        Vec::<Call>::new(),
        "the discovery is adopted without a new Sparkle check or window"
    );
    assert!(recorder.events().contains(&UpdaterEvent::StateChanged));
}

#[gpui::test]
fn sparkles_relaunch_waits_for_the_prepare_hooks(cx: &mut TestAppContext) {
    let (backend, _engine) = sparkle(true);
    let updater = cx.update(|cx| gpui_auto_update::init(config(backend.clone()), cx));
    cx.run_until_parked();

    let hook_started = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = futures::channel::oneshot::channel::<()>();
    let release_rx = Arc::new(Mutex::new(Some(release_rx)));
    let _hook = {
        let hook_started = hook_started.clone();
        let release_rx = release_rx.clone();
        updater.update(cx, |u, _| {
            u.on_prepare_to_install(move |cx| {
                hook_started.store(true, Ordering::SeqCst);
                let release_rx = release_rx
                    .lock()
                    .unwrap()
                    .take()
                    .expect("the hook runs once");
                cx.background_executor().spawn(async move {
                    release_rx
                        .await
                        .map_err(|canceled| Box::new(canceled) as gpui_auto_update::PrepareError)
                })
            })
        })
    };

    let resumes = Arc::new(AtomicUsize::new(0));
    let counting = resumes.clone();
    backend.events().publish(SparkleEvent::RelaunchRequested {
        update: SparkleUpdate::new("2.0"),
        continuation: RelaunchContinuation::from_fn(move || {
            counting.fetch_add(1, Ordering::SeqCst);
        }),
    });

    let started = hook_started.clone();
    wait_until(cx, move |_| started.load(Ordering::SeqCst));
    assert_eq!(
        resumes.load(Ordering::SeqCst),
        0,
        "the relaunch stays postponed while the hook saves"
    );

    release_tx.send(()).unwrap();
    let counting = resumes.clone();
    wait_until(cx, move |_| counting.load(Ordering::SeqCst) == 1);
}

#[gpui::test]
fn a_failing_prepare_hook_leaves_sparkles_relaunch_postponed(cx: &mut TestAppContext) {
    let (backend, _engine) = sparkle(true);
    let updater = cx.update(|cx| gpui_auto_update::init(config(backend.clone()), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    let _hook = updater.update(cx, |u, _| {
        u.on_prepare_to_install(|_| gpui::Task::ready(Err("disk full".into())))
    });

    let resumes = Arc::new(AtomicUsize::new(0));
    let counting = resumes.clone();
    backend.events().publish(SparkleEvent::RelaunchRequested {
        update: SparkleUpdate::new("2.0"),
        continuation: RelaunchContinuation::from_fn(move || {
            counting.fetch_add(1, Ordering::SeqCst);
        }),
    });

    let failed = recorder.events.clone();
    wait_until(cx, move |_| {
        failed.lock().unwrap().iter().any(|event| {
            matches!(event, UpdaterEvent::Failed(error) if error.kind() == ErrorKind::QuitCoordination)
        })
    });
    assert_eq!(
        resumes.load(Ordering::SeqCst),
        0,
        "a failed hook never resumes the relaunch"
    );
}
