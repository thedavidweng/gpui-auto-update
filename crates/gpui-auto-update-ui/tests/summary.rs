//! What the neutral controls show for each update state, derived from the
//! facade's observable `Updater` alone.

use gpui::{Entity, TestAppContext};
use gpui_auto_update::{PreviewState, Updater, UpdaterConfig};
use gpui_auto_update_ui::{Tone, UpdateAction, UpdateSummary};

fn preview_updater(cx: &mut TestAppContext, preview: PreviewState) -> Entity<Updater> {
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            UpdaterConfig::preview("dev.example.ui-summary", preview),
            cx,
        )
    });
    cx.run_until_parked();
    updater
}

fn summary(cx: &mut TestAppContext, preview: PreviewState) -> UpdateSummary {
    let updater = preview_updater(cx, preview);
    updater.read_with(cx, |updater, _| UpdateSummary::new(updater, Some("1.0.0")))
}

#[gpui::test]
fn available_update_offers_download_with_release_notes(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::UpdateAvailable);
    assert_eq!(summary.headline, "Version 0.0.0-preview is available.");
    assert!(summary.detail.unwrap().starts_with("Preview:"));
    assert_eq!(
        summary.actions,
        vec![UpdateAction::Download, UpdateAction::Dismiss]
    );
    assert_eq!(summary.tone, Tone::Attention);
    assert_eq!(summary.attention, Some(UpdateAction::Download));
    assert_eq!(summary.preview, Some(PreviewState::UpdateAvailable));
    assert_eq!(
        summary.automatic_updates, None,
        "no setting while previewing"
    );
}

#[gpui::test]
fn downloading_reports_known_progress(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::Downloading);
    assert_eq!(summary.headline, "Downloading version 0.0.0-preview…");
    let progress = summary.progress.expect("downloading shows progress");
    assert_eq!(progress.fraction, Some(0.42));
    assert_eq!(progress.label, "42.0 of 100.0 MiB");
    assert!(summary.actions.is_empty());
    assert_eq!(summary.attention, None);
}

#[gpui::test]
fn staged_update_offers_restart(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::ReadyToInstall);
    assert_eq!(
        summary.headline,
        "Version 0.0.0-preview is ready to install."
    );
    assert_eq!(summary.actions, vec![UpdateAction::RestartToUpdate]);
    assert_eq!(summary.attention, Some(UpdateAction::RestartToUpdate));
}

#[gpui::test]
fn restart_required_offers_restart(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::RestartRequired);
    assert_eq!(
        summary.headline,
        "Restart to finish updating to version 0.0.0-preview."
    );
    assert_eq!(summary.actions, vec![UpdateAction::RestartToUpdate]);
    assert_eq!(summary.attention, Some(UpdateAction::RestartToUpdate));
}

#[gpui::test]
fn failure_shows_the_message_and_allows_retry(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::Error);
    assert_eq!(summary.headline, "The update failed.");
    assert_eq!(
        summary.detail.as_ref().map(|detail| detail.as_ref()),
        Some("Preview: the update server could not be reached.")
    );
    assert_eq!(summary.tone, Tone::Critical);
    assert_eq!(
        summary.actions,
        vec![UpdateAction::CheckForUpdates, UpdateAction::Dismiss]
    );
}

#[gpui::test]
fn externally_managed_names_the_manager(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::ExternallyManaged);
    assert_eq!(
        summary.headline,
        "Updates are managed by Preview package manager."
    );
    assert_eq!(
        summary.detail.as_ref().map(|detail| detail.as_ref()),
        Some("Use Preview package manager to update this app.")
    );
    assert_eq!(summary.attention, None);
}

#[gpui::test]
fn live_state_reports_the_current_version_and_the_setting(cx: &mut TestAppContext) {
    let updater = preview_updater(cx, PreviewState::UpdateAvailable);
    updater.update(cx, |updater, cx| updater.exit_preview(cx));
    cx.run_until_parked();
    let summary = updater.read_with(cx, |updater, _| UpdateSummary::new(updater, Some("1.0.0")));
    assert_eq!(summary.preview, None);
    // A preview configuration has no backend, so the live state is unsupported.
    assert_eq!(summary.headline, "This installation cannot update itself.");
    assert_eq!(summary.actions, vec![UpdateAction::CheckForUpdates]);
    assert_eq!(summary.automatic_updates, Some(true));
}

#[gpui::test]
fn action_labels_describe_the_update(cx: &mut TestAppContext) {
    let summary = summary(cx, PreviewState::UpdateAvailable);
    assert_eq!(UpdateAction::Download.label(), "Download Update");
    assert_eq!(
        summary.accessible_label(UpdateAction::Download),
        "Download version 0.0.0-preview"
    );
    assert_eq!(
        summary.accessible_label(UpdateAction::Dismiss),
        "Dismiss: Version 0.0.0-preview is available."
    );
}

/// A backend whose previous update was rolled back after the last quit.
struct RolledBackBackend {
    failure: std::sync::Mutex<Option<gpui_auto_update::core::UpdateError>>,
}

impl gpui_auto_update::UpdateBackend for RolledBackBackend {
    fn capability(&self) -> gpui_auto_update::core::Capability {
        gpui_auto_update::core::Capability::SelfManaged
    }

    fn stage(
        &self,
        _: &gpui_auto_update::core::AvailableUpdate,
        _: &gpui_auto_update::ProgressSink,
    ) -> Result<(), gpui_auto_update::core::UpdateError> {
        unreachable!("tests never stage")
    }

    fn install(
        &self,
        _: &gpui_auto_update::core::AvailableUpdate,
        _: &gpui_auto_update::ProgressSink,
    ) -> Result<gpui_auto_update::Handoff, gpui_auto_update::core::UpdateError> {
        unreachable!("tests never install")
    }

    fn take_previous_failure(&self) -> Option<gpui_auto_update::core::UpdateError> {
        self.failure.lock().unwrap().take()
    }
}

#[gpui::test]
fn a_rolled_back_previous_update_is_surfaced(cx: &mut TestAppContext) {
    use gpui_auto_update::core::{CheckPolicy, ErrorKind, MemoryPreferenceStore, UpdateError};

    struct UpToDate;
    impl gpui_auto_update::core::CheckSource for UpToDate {
        fn check(
            &self,
            _: &gpui_auto_update::core::CheckRequest,
        ) -> Result<gpui_auto_update::core::CheckOutcome, gpui_auto_update::core::UpdateError>
        {
            Ok(gpui_auto_update::core::CheckOutcome::UpToDate)
        }
    }

    let failure = UpdateError::new(ErrorKind::HealthConfirmation).with_message(
        "Version 2.0.0 did not start correctly, so the previous version was restored.",
    );
    let updater = cx.update(|cx| {
        gpui_auto_update::init(
            UpdaterConfig::new("dev.example.ui-rollback", UpToDate)
                .with_backend(RolledBackBackend {
                    failure: std::sync::Mutex::new(Some(failure)),
                })
                .with_preferences(MemoryPreferenceStore::new())
                .with_policy(CheckPolicy::recommended().with_check_on_launch(false)),
            cx,
        )
    });
    cx.run_until_parked();
    let summary = updater.read_with(cx, |updater, _| UpdateSummary::new(updater, Some("1.0.0")));
    assert_eq!(
        summary
            .previous_update_failure
            .as_ref()
            .map(|failure| failure.as_ref()),
        Some("Version 2.0.0 did not start correctly, so the previous version was restored."),
        "rollback diagnostics are shown to the user"
    );
}
