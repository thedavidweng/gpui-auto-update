//! The facade on macOS with the Sparkle backend, driven by a scripted
//! Sparkle engine (no framework needed).

#![cfg(target_os = "macos")]

mod support;

use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use gpui::TestAppContext;
use gpui_auto_update::core::{
    AvailableUpdate, CheckKind, CheckOutcome, PreferenceOwner, UpdateError, UpdateState,
};
use gpui_auto_update::macos::{
    SparkleBackend, SparkleEngine, SparkleEvent, SparkleEvents, SparkleUpdate,
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
}

impl ScriptedSparkle {
    fn new(events: &SparkleEvents, automatic: bool) -> Self {
        Self {
            events: events.clone(),
            state: Arc::new(Mutex::new(Script {
                calls: Vec::new(),
                replies: Vec::new(),
                automatic,
            })),
        }
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
        Ok(false)
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
