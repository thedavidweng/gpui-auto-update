//! Preview states are deterministic, clearly marked, and never install.

mod support;

use gpui::TestAppContext;
use gpui_auto_update::core::{Capability, CheckKind, CheckOutcome, ErrorKind, UpdateState};
use gpui_auto_update::{
    InstallUpdate, PREVIEW_CHANNEL, PREVIEW_VERSION, PreviewState, RestartToUpdate, UpdaterConfig,
    UpdaterEvent,
};

use support::*;

#[test]
fn preview_states_are_marked_as_previews() {
    let update = PreviewState::update();
    assert_eq!(update.version, PREVIEW_VERSION);
    assert_eq!(
        update.channel.as_ref().map(|c| c.as_str()),
        Some(PREVIEW_CHANNEL)
    );

    assert_eq!(
        PreviewState::UpdateAvailable.state(),
        UpdateState::Available(update.clone())
    );
    assert_eq!(
        PreviewState::ReadyToInstall.state(),
        UpdateState::Staged(update.clone())
    );
    assert_eq!(
        PreviewState::RestartRequired.state(),
        UpdateState::WaitingForQuit(update.clone())
    );
    match PreviewState::Downloading.state() {
        UpdateState::Downloading {
            update: u,
            progress,
        } => {
            assert_eq!(u, update);
            assert_eq!(progress.fraction(), Some(0.42));
        }
        other => panic!("unexpected {other:?}"),
    }
    match PreviewState::Error.state() {
        UpdateState::Failed(error) => assert!(error.message().starts_with("Preview:")),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        PreviewState::ExternallyManaged.state(),
        UpdateState::Disabled {
            capability: Capability::ExternallyManaged { manager: Some(_) }
        }
    ));
}

#[gpui::test]
fn previews_never_check_install_or_restart(cx: &mut TestAppContext) {
    let source = FakeSource::new(cx);
    source.available("2.0.0");
    let backend = FakeBackend::new(cx);
    let updater = cx.update(|cx| gpui_auto_update::init(config(&source, &backend), cx));
    cx.run_until_parked();
    let recorder = Recorder::new(&updater, cx);
    let restarted = std::sync::Arc::new(std::sync::Mutex::new(false));
    let flag = restarted.clone();
    let _restart = cx.update(|cx| cx.on_app_restart(move |_| *flag.lock().unwrap() = true));

    for preview in PreviewState::ALL {
        updater.update(cx, |u, cx| u.enter_preview(preview, cx));
        assert_eq!(state(&updater, cx), preview.state());
        assert!(updater.read_with(cx, |u, _| u.is_preview()
            && !u.is_self_update_supported()
            && !u.installs_allowed()));

        let (install, restart) =
            updater.update(cx, |u, cx| (u.request_install(cx), u.restart_to_update(cx)));
        assert_eq!(install.unwrap_err().kind(), ErrorKind::InvalidState);
        assert_eq!(restart.unwrap_err().kind(), ErrorKind::InvalidState);
        cx.update(|cx| {
            cx.dispatch_action(&InstallUpdate);
            cx.dispatch_action(&RestartToUpdate);
        });
        updater.update(cx, |u, cx| u.check_for_updates(cx));
        cx.run_until_parked();
    }

    assert_eq!(source.calls(), 0, "previews never check");
    assert!(backend.operations().is_empty(), "previews never install");
    assert!(!*restarted.lock().unwrap(), "previews never restart");
    assert_eq!(
        recorder.states(),
        PreviewState::ALL
            .iter()
            .map(|p| p.state())
            .collect::<Vec<_>>()
    );

    let checks: Vec<_> = recorder
        .events()
        .into_iter()
        .filter_map(|event| match event {
            UpdaterEvent::CheckFinished {
                kind: CheckKind::Manual,
                result,
            } => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(checks.len(), PreviewState::ALL.len());
    assert_eq!(
        checks[0],
        Ok(CheckOutcome::UpdateAvailable(PreviewState::update()))
    );
    assert_eq!(
        checks[3].clone().unwrap_err().kind(),
        ErrorKind::FeedRetrieval
    );
    assert_eq!(
        checks[4].clone().unwrap_err().kind(),
        ErrorKind::ExternallyManaged
    );

    updater.update(cx, |u, cx| u.exit_preview(cx));
    assert_eq!(state(&updater, cx), UpdateState::Idle);
    assert!(updater.read_with(cx, |u, _| !u.is_preview()));
}

#[gpui::test]
fn preview_configuration_needs_no_source_or_backend(cx: &mut TestAppContext) {
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            UpdaterConfig::preview(APP_ID, PreviewState::Downloading),
            cx,
        )
    });
    cx.run_until_parked();

    assert_eq!(state(&updater, cx), PreviewState::Downloading.state());
    assert_eq!(
        updater.read_with(cx, |u, _| u.preview()),
        Some(PreviewState::Downloading)
    );

    updater.update(cx, |u, cx| u.exit_preview(cx));
    assert_eq!(
        state(&updater, cx),
        UpdateState::Disabled {
            capability: Capability::Unsupported
        }
    );
}
