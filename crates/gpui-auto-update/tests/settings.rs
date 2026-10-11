//! Automatic-update preference, channel, and periodic checks through the
//! facade.

mod support;

use std::time::Duration;

use gpui::TestAppContext;
use gpui_auto_update::UpdaterEvent;
use gpui_auto_update::core::{
    Channel, CheckKind, CheckPolicy, ErrorKind, MemoryPreferenceStore, PreferenceStore as _,
    UpdatePreferences,
};

use support::*;

#[gpui::test]
fn automatic_preference_is_saved_off_the_foreground(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let store = MemoryPreferenceStore::new();
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            config(&source, &backend).with_preferences(store.clone()),
            cx,
        )
    });
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    assert!(updater.read_with(cx, |u, _| u.automatic_checks_enabled()));

    updater
        .update(cx, |u, cx| u.set_automatic_checks(false, cx))
        .unwrap();
    cx.run_until_parked();

    assert!(updater.read_with(cx, |u, _| !u.automatic_checks_enabled()));
    assert_eq!(
        store.load().unwrap().map(|p| p.automatic_checks),
        Some(false)
    );
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::AutomaticChecksChanged(false))
    );
    assert_eq!(source.calls(), 0);
}

#[gpui::test]
fn enabling_automatic_checks_starts_a_background_check(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    source.available("2.0.0");
    let backend = FakeBackend::new(cx);
    let store = MemoryPreferenceStore::new();
    store.save(&UpdatePreferences::new(false)).unwrap();
    let updater = cx
        .update(|cx| gpui_auto_update::init(config(&source, &backend).with_preferences(store), cx));
    cx.run_until_parked();

    updater
        .update(cx, |u, cx| u.set_automatic_checks(true, cx))
        .unwrap();
    cx.run_until_parked();

    assert_eq!(source.requests().len(), 1);
    assert_eq!(source.requests()[0].kind, CheckKind::Background);
    assert!(!source.ran_on_main_thread());
}

#[gpui::test]
fn channel_is_read_and_changed_through_the_backend(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx).with_channel(Some("beta"));
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    assert_eq!(
        updater.read_with(cx, |u, _| u.channel().cloned()),
        Some(Channel::new("beta"))
    );

    updater.update(cx, |u, cx| u.set_channel(None, cx)).unwrap();
    cx.run_until_parked();

    assert_eq!(updater.read_with(cx, |u, _| u.channel().cloned()), None);
    assert!(
        recorder
            .events()
            .contains(&UpdaterEvent::ChannelChanged(None))
    );
    assert!(
        backend
            .operations()
            .contains(&BackendCall::SetChannel(None))
    );
    assert!(!backend.ran_on_main_thread());
}

#[gpui::test]
fn backends_without_channels_report_a_configuration_error(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            gpui_auto_update::UpdaterConfig::new(APP_ID, source.clone())
                .with_preferences(MemoryPreferenceStore::new()),
            cx,
        )
    });
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);

    updater
        .update(cx, |u, cx| u.set_channel(Some(Channel::new("beta")), cx))
        .unwrap();
    cx.run_until_parked();

    assert!(recorder.events().iter().any(|event| matches!(
        event,
        UpdaterEvent::Failed(error) if error.kind() == ErrorKind::Configuration
    )));
}

#[gpui::test]
fn periodic_checks_follow_the_policy_interval(cx: &mut TestAppContext) {
    let hour = Duration::from_secs(60 * 60);
    let source = FakeSource::new(cx);
    let backend = FakeBackend::new(cx);
    let clock = ManualClock::new();
    let _updater = cx.update(|cx| {
        gpui_auto_update::init(
            config(&source, &backend)
                .with_clock(clock.clone())
                .with_policy(
                    CheckPolicy::recommended()
                        .with_check_on_launch(false)
                        .with_minimum_interval(hour)
                        .with_periodic_interval(Some(2 * hour)),
                ),
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(source.calls(), 1, "no check yet, so one is due right away");

    clock.advance(hour);
    cx.executor().advance_clock(hour);
    cx.run_until_parked();
    assert_eq!(source.calls(), 1);

    clock.advance(hour);
    cx.executor().advance_clock(hour);
    cx.run_until_parked();
    assert_eq!(source.calls(), 2);
    assert!(
        source
            .requests()
            .iter()
            .all(|r| r.kind == CheckKind::Background)
    );
}
