//! Startup health: confirming that the main window opened, and reporting
//! what the previous update attempt left behind.

mod support;

use gpui::TestAppContext;
use gpui_auto_update::UpdaterEvent;
use gpui_auto_update::core::{ErrorKind, UpdateError, UpdateState};

use support::*;

#[gpui::test]
fn a_rolled_back_update_is_reported_once_startup_finishes(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let rolled_back = UpdateError::new(ErrorKind::HealthConfirmation).with_message(
        "Version 2.0.0 did not start correctly, so the previous version was restored.",
    );
    let backend = FakeBackend::new(cx).with_previous_failure(rolled_back.clone());
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    let recorder = Recorder::new(&updater, cx);
    cx.run_until_parked();

    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::PreviousUpdateFailed(rolled_back.clone()))
    );
    updater.read_with(cx, |updater, _| {
        assert_eq!(updater.previous_update_failure(), Some(&rolled_back));
        assert_eq!(updater.last_error(), Some(&rolled_back));
        assert_eq!(updater.state(), UpdateState::Idle);
    });
    assert!(
        !backend.ran_on_main_thread(),
        "reading the diagnostic must not block the foreground"
    );
}

#[gpui::test]
fn nothing_is_reported_without_a_previous_failure(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    let recorder = Recorder::new(&updater, cx);
    cx.run_until_parked();

    assert!(
        !recorder
            .events()
            .iter()
            .any(|event| matches!(event, UpdaterEvent::PreviousUpdateFailed(_)))
    );
    updater.read_with(cx, |updater, _| {
        assert_eq!(updater.previous_update_failure(), None);
    });
}

#[gpui::test]
fn main_window_opened_confirms_startup_once_off_the_foreground(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    cx.run_until_parked();

    updater.update(cx, |updater, cx| updater.main_window_opened(cx));
    updater.update(cx, |updater, cx| updater.main_window_opened(cx));
    cx.run_until_parked();

    assert_eq!(backend.operations(), vec![BackendCall::ConfirmStartup]);
    assert!(!backend.ran_on_main_thread());
}

#[gpui::test]
fn a_failed_startup_confirmation_is_reported(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let failure = error(ErrorKind::HealthConfirmation);
    let backend = FakeBackend::new(cx).with_confirm_result(Err(failure.clone()));
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    let recorder = Recorder::new(&updater, cx);
    cx.run_until_parked();

    updater.update(cx, |updater, cx| updater.main_window_opened(cx));
    cx.run_until_parked();

    assert!(recorder.events().contains(&UpdaterEvent::Failed(failure)));
}
