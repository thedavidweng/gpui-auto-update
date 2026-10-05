//! Behavior of the GPUI updater entity, driven through its public API and
//! actions with GPUI's deterministic test executor.

mod support;

use std::sync::{Arc, Mutex};

use gpui::{AppContext as _, TestAppContext};
use gpui_auto_update::core::{
    Capability, CheckKind, CheckOutcome, CheckPolicy, DownloadProgress, ErrorKind,
    MemoryPreferenceStore, PreferenceStore as _, UpdatePreferences, UpdateState,
};
use gpui_auto_update::{
    BuildProfile, CheckForUpdates, Handoff, InstallUpdate, RestartToUpdate, Updater, UpdaterEvent,
};

use support::*;

#[gpui::test]
fn manual_check_action_runs_off_the_foreground_and_notifies_observers(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    source.available("2.0.0");
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);

    cx.update(|cx| cx.dispatch_action(&CheckForUpdates));
    cx.run_until_parked();

    assert_eq!(state(&updater, cx), UpdateState::Available(update("2.0.0")));
    assert_eq!(
        recorder.states(),
        vec![
            UpdateState::Checking,
            UpdateState::Available(update("2.0.0"))
        ]
    );
    assert_eq!(source.calls(), 1);
    assert_eq!(source.requests()[0].kind, CheckKind::Manual);
    assert!(
        !source.ran_on_main_thread(),
        "checks must not block the foreground"
    );
    assert!(
        !backend.ran_on_main_thread(),
        "backend calls must not block the foreground"
    );
    assert!(recorder.events().contains(&UpdaterEvent::CheckFinished {
        kind: CheckKind::Manual,
        result: Ok(CheckOutcome::UpdateAvailable(update("2.0.0"))),
    }));
}

#[gpui::test]
fn background_check_failures_stay_silent_but_manual_failures_are_visible(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    source.push(Err(error(ErrorKind::FeedRetrieval)));
    source.push(Err(error(ErrorKind::FeedRetrieval)));
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            config(&source, &backend).with_policy(CheckPolicy::recommended()),
            cx,
        )
    });
    let recorder = Recorder::new(&updater, cx);
    cx.run_until_parked();

    assert_eq!(source.requests()[0].kind, CheckKind::Background);
    assert_eq!(state(&updater, cx), UpdateState::Idle);
    assert!(updater.read_with(cx, |u, _| u.last_error().is_none()));

    cx.update(|cx| cx.dispatch_action(&CheckForUpdates));
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Failed(error(ErrorKind::FeedRetrieval))
    );
    assert_eq!(
        updater.read_with(cx, |u, _| u.last_error().cloned()),
        Some(error(ErrorKind::FeedRetrieval))
    );
    assert!(recorder.events().contains(&UpdaterEvent::CheckFinished {
        kind: CheckKind::Manual,
        result: Err(error(ErrorKind::FeedRetrieval)),
    }));
}

#[gpui::test]
fn launch_check_respects_the_automatic_preference(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let store = MemoryPreferenceStore::new();
    store.save(&UpdatePreferences::new(false)).unwrap();
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            config(&source, &backend)
                .with_preferences(store)
                .with_policy(CheckPolicy::recommended()),
            cx,
        )
    });
    cx.run_until_parked();

    assert_eq!(source.calls(), 0, "automatic checks are off");
    assert!(updater.read_with(cx, |u, _| u.is_ready() && !u.automatic_checks_enabled()));
}

#[gpui::test]
fn externally_managed_installs_report_why_without_checking(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let homebrew = Capability::ExternallyManaged {
        manager: Some("Homebrew".into()),
    };
    let backend = FakeBackend::new(cx).with_capability(homebrew.clone());
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    let recorder = Recorder::new(&updater, cx);
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Disabled {
            capability: homebrew.clone()
        }
    );
    assert!(updater.read_with(cx, |u, _| !u.is_self_update_supported()));
    assert_eq!(backend.calls(), vec![BackendCall::Capability]);
    assert!(!backend.ran_on_main_thread());

    updater.update(cx, |u, cx| u.check_for_updates(cx));
    cx.run_until_parked();

    assert_eq!(source.calls(), 0);
    let manual = recorder.events().into_iter().find_map(|event| match event {
        UpdaterEvent::CheckFinished {
            kind: CheckKind::Manual,
            result,
        } => Some(result),
        _ => None,
    });
    assert_eq!(
        manual.unwrap().unwrap_err().kind(),
        ErrorKind::ExternallyManaged
    );
}

#[gpui::test]
fn configured_capability_overrides_the_backend(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            config(&source, &backend).with_capability(Capability::Unsupported),
            cx,
        )
    });
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Disabled {
            capability: Capability::Unsupported
        }
    );
    assert!(backend.calls().is_empty());
}

#[gpui::test]
fn manual_check_before_startup_finishes_is_queued(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    source.available("2.0.0");
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    assert!(updater.read_with(cx, |u, _| !u.is_ready()));

    updater.update(cx, |u, cx| u.check_for_updates(cx));
    cx.run_until_parked();

    assert_eq!(source.calls(), 1);
    assert_eq!(state(&updater, cx), UpdateState::Available(update("2.0.0")));
}

fn available_updater(
    cx: &mut TestAppContext,
    backend: &FakeBackend,
    config_fn: impl FnOnce(gpui_auto_update::UpdaterConfig) -> gpui_auto_update::UpdaterConfig,
) -> (gpui::Entity<Updater>, FakeSource) {
    let source = FakeSource::new(cx);
    source.available("2.0.0");
    let updater = cx.update(|cx| gpui_auto_update::init(config_fn(config(&source, backend)), cx));
    cx.run_until_parked();
    updater.update(cx, |u, cx| u.check_for_updates(cx));
    cx.run_until_parked();
    assert_eq!(state(&updater, cx), UpdateState::Available(update("2.0.0")));
    (updater, source)
}

fn restarts(cx: &mut TestAppContext) -> (Arc<Mutex<u32>>, gpui::Subscription) {
    let count = Arc::new(Mutex::new(0));
    let counter = count.clone();
    let subscription = cx.update(|cx| cx.on_app_restart(move |_| *counter.lock().unwrap() += 1));
    (count, subscription)
}

#[gpui::test]
fn install_stages_with_progress_then_saves_and_restarts(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx)
        .with_progress(vec![
            DownloadProgress {
                downloaded: 50,
                total: Some(100),
            },
            DownloadProgress {
                downloaded: 100,
                total: Some(100),
            },
        ])
        .with_handoff(Ok(restart_path("/tmp/helper")));
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    let recorder = Recorder::new(&updater, cx);
    let (restart_count, _restart) = restarts(cx);

    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();

    let u2 = update("2.0.0");
    let downloading = |downloaded, total| UpdateState::Downloading {
        update: u2.clone(),
        progress: DownloadProgress { downloaded, total },
    };
    assert_eq!(
        recorder.states(),
        vec![
            downloading(0, None),
            downloading(0, Some(100)),
            downloading(50, Some(100)),
            downloading(100, Some(100)),
            UpdateState::Verifying(u2.clone()),
            UpdateState::Staged(u2.clone()),
        ]
    );
    assert_eq!(*restart_count.lock().unwrap(), 0, "staging never restarts");

    let hook_saw_install = Arc::new(Mutex::new(None));
    let seen = hook_saw_install.clone();
    let hook_backend = backend.clone();
    let _hook = updater.update(cx, |u, _| {
        u.on_prepare_to_install(move |cx| {
            let seen = seen.clone();
            let backend = hook_backend.clone();
            cx.spawn(async move |_| {
                *seen.lock().unwrap() = Some(
                    backend
                        .operations()
                        .iter()
                        .any(|call| matches!(call, BackendCall::Install(_))),
                );
                Ok(())
            })
        })
    });

    cx.update(|cx| cx.dispatch_action(&RestartToUpdate));
    cx.run_until_parked();

    assert_eq!(
        *hook_saw_install.lock().unwrap(),
        Some(false),
        "hook runs before install"
    );
    assert_eq!(
        backend.operations(),
        vec![
            BackendCall::Stage("2.0.0".into()),
            BackendCall::Install("2.0.0".into())
        ]
    );
    assert!(!backend.ran_on_main_thread());
    assert_eq!(
        recorder.states()[6..],
        [
            UpdateState::Installing(u2.clone()),
            UpdateState::Relaunching(u2.clone())
        ]
    );
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::Handoff(restart_path("/tmp/helper")))
    );
    assert_eq!(*restart_count.lock().unwrap(), 1);
}

#[gpui::test]
fn overlapping_install_requests_are_rejected(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx);
    let (updater, _source) = available_updater(cx, &backend, |c| c);

    let (first, second) =
        updater.update(cx, |u, cx| (u.request_install(cx), u.request_install(cx)));
    assert!(first.is_ok());
    assert_eq!(second.unwrap_err().kind(), ErrorKind::OperationInProgress);
    assert!(updater.read_with(cx, |u, _| u.is_busy()));
    cx.run_until_parked();

    assert_eq!(
        backend.operations(),
        vec![BackendCall::Stage("2.0.0".into())]
    );
    assert!(updater.read_with(cx, |u, _| !u.is_busy()));
}

#[gpui::test]
fn failing_save_hook_keeps_the_update_staged(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx);
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    let (restart_count, _restart) = restarts(cx);
    let _hook = updater.update(cx, |u, _| {
        u.on_prepare_to_install(|_| gpui::Task::ready(Err("disk full".into())))
    });

    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(state(&updater, cx), UpdateState::Staged(update("2.0.0")));
    assert_eq!(
        backend.operations(),
        vec![BackendCall::Stage("2.0.0".into())]
    );
    assert_eq!(*restart_count.lock().unwrap(), 0);
    assert!(recorder.events().iter().any(|event| matches!(
        event,
        UpdaterEvent::Failed(error) if error.kind() == ErrorKind::QuitCoordination
    )));
}

#[gpui::test]
fn dropped_hooks_no_longer_run(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx);
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    let hook = updater.update(cx, |u, _| {
        u.on_prepare_to_install(|_| gpui::Task::ready(Err("should not run".into())))
    });
    drop(hook);

    updater.update(cx, |u, cx| u.restart_to_update(cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Relaunching(update("2.0.0"))
    );
}

#[gpui::test]
fn helper_owned_relaunch_quits_instead_of_restarting(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx).with_handoff(Ok(Handoff::Quit));
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    let (restart_count, _restart) = restarts(cx);

    updater.update(cx, |u, cx| u.restart_to_update(cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::WaitingForQuit(update("2.0.0"))
    );
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::Handoff(Handoff::Quit))
    );
    assert_eq!(
        *restart_count.lock().unwrap(),
        0,
        "the helper owns relaunch"
    );
}

#[gpui::test]
fn backend_owned_relaunch_is_left_to_the_backend(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx).with_handoff(Ok(Handoff::BackendOwned));
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    let (restart_count, _restart) = restarts(cx);

    updater.update(cx, |u, cx| u.restart_to_update(cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Installing(update("2.0.0"))
    );
    assert_eq!(*restart_count.lock().unwrap(), 0);
}

#[gpui::test]
fn staging_and_install_errors_propagate(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx).with_stage_result(Err(error(ErrorKind::Signature)));
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    let recorder = Recorder::new(&updater, cx);

    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Failed(error(ErrorKind::Signature))
    );
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::Failed(error(ErrorKind::Signature)))
    );

    let backend = FakeBackend::new(cx).with_handoff(Err(error(ErrorKind::HelperLaunch)));
    let (updater, _source) = available_updater(cx, &backend, |c| c);
    let (restart_count, _restart) = restarts(cx);
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    updater.update(cx, |u, cx| u.restart_to_update(cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(
        state(&updater, cx),
        UpdateState::Failed(error(ErrorKind::HelperLaunch))
    );
    assert_eq!(*restart_count.lock().unwrap(), 0);
}

#[gpui::test]
fn install_action_without_an_update_reports_a_failure(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);

    cx.update(|cx| cx.dispatch_action(&InstallUpdate));
    cx.run_until_parked();

    assert!(recorder.events().iter().any(|event| matches!(
        event,
        UpdaterEvent::Failed(error) if error.kind() == ErrorKind::InvalidState
    )));
    assert!(backend.operations().is_empty());
}

#[gpui::test]
fn debug_builds_do_not_install_unless_allowed(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx);
    let (updater, _source) =
        available_updater(cx, &backend, |c| c.with_build_profile(BuildProfile::Debug));

    let result = updater.update(cx, |u, cx| u.request_install(cx));
    cx.run_until_parked();

    assert_eq!(result.unwrap_err().kind(), ErrorKind::Configuration);
    assert!(updater.read_with(cx, |u, _| !u.installs_allowed()));
    assert!(backend.operations().is_empty());
    assert_eq!(state(&updater, cx), UpdateState::Available(update("2.0.0")));

    let backend = FakeBackend::new(cx);
    let (updater, _source) = available_updater(cx, &backend, |c| {
        c.with_build_profile(BuildProfile::Debug)
            .allow_debug_self_update(true)
    });
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(state(&updater, cx), UpdateState::Staged(update("2.0.0")));
}

/// An updater outside the GPUI global, so the test controls its lifetime.
fn standalone_available(cx: &mut TestAppContext, backend: &FakeBackend) -> gpui::Entity<Updater> {
    let source = FakeSource::new(cx);
    source.available("2.0.0");
    let updater = cx.new(|cx| Updater::new(config(&source, backend), cx));
    cx.run_until_parked();
    updater.update(cx, |u, cx| u.check_for_updates(cx));
    cx.run_until_parked();
    updater
}

#[gpui::test]
fn dropping_the_updater_cancels_queued_work(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx);
    let updater = standalone_available(cx, &backend);

    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    drop(updater);
    // GPUI releases dropped entities when it flushes effects.
    cx.update(|_| {});
    cx.run_until_parked();

    assert!(backend.operations().is_empty());
}

#[gpui::test]
fn dropped_updater_never_ends_the_app(cx: &mut TestAppContext) {
    let backend = FakeBackend::new(cx);
    let updater = standalone_available(cx, &backend);
    updater.update(cx, |u, cx| u.request_install(cx)).unwrap();
    cx.run_until_parked();
    let (restart_count, _restart) = restarts(cx);

    updater.update(cx, |u, cx| u.restart_to_update(cx)).unwrap();
    drop(updater);
    cx.update(|_| {});
    cx.run_until_parked();

    assert_eq!(*restart_count.lock().unwrap(), 0);
    assert!(
        !backend
            .operations()
            .contains(&BackendCall::Install("2.0.0".into())),
        "install waits for the save hooks, which are cancelled with the updater"
    );
}
