//! Everything the reference app shows about updates.
//!
//! The update status itself is rendered by the neutral
//! [`UpdateControls`] from `gpui-auto-update-ui`; this panel only adds the
//! preview switcher the app uses to exercise every state. [`UpdatePanel::model`]
//! exposes what is shown, straight from the controls, so the tests below
//! stay a contract for the panel even though the drawing moved into the
//! library.

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    div, prelude::FluentBuilder as _, rgb,
};
use gpui_auto_update::{PreviewState, Updater};
use gpui_auto_update_ui::{UpdateControls, UpdateSummary};

/// The update section of the main window: the neutral controls plus the
/// app's preview switcher.
pub struct UpdatePanel {
    controls: Entity<UpdateControls>,
    updater: Entity<Updater>,
}

impl UpdatePanel {
    pub fn new(
        updater: Entity<Updater>,
        current_version: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let controls = cx.new(|cx| {
            UpdateControls::new(updater.clone(), window, cx)
                .with_current_version(current_version.into())
        });
        Self { controls, updater }
    }

    /// What the panel currently shows.
    pub fn model(&self, cx: &App) -> UpdateSummary {
        self.controls.read(cx).summary(cx)
    }

    /// Feedback that the state does not show, such as a manual check refused
    /// by an externally managed installation. The controls render it; this
    /// accessor exists for tests.
    #[cfg(test)]
    pub fn feedback(&self, cx: &App) -> Option<String> {
        self.controls.read(cx).feedback().map(str::to_owned)
    }

    fn select_preview(&mut self, preview: Option<PreviewState>, cx: &mut Context<Self>) {
        self.updater.update(cx, |updater, cx| match preview {
            Some(preview) => updater.enter_preview(preview, cx),
            None => updater.exit_preview(cx),
        });
    }
}

fn preview_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .px_2()
        .py_1()
        .border_1()
        .border_color(rgb(0x8a8a8a))
        .rounded_md()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xe8e8e8)))
        .child(label.into())
}

fn preview_label(preview: PreviewState) -> &'static str {
    match preview {
        PreviewState::UpdateAvailable => "Available",
        PreviewState::Downloading => "Downloading",
        PreviewState::ReadyToInstall => "Ready",
        PreviewState::Error => "Error",
        PreviewState::ExternallyManaged => "Managed",
        PreviewState::RestartRequired => "Restart",
        _ => "Other",
    }
}

impl Render for UpdatePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model(cx);

        let mut previews = div().flex().flex_wrap().gap_1().text_xs().child(
            preview_button("preview-live", "Live")
                .when(model.preview.is_none(), |b| b.bg(rgb(0xd0d0d0)))
                .on_click(cx.listener(|this, _, _, cx| this.select_preview(None, cx))),
        );
        for preview in PreviewState::ALL {
            previews = previews.child(
                preview_button(
                    SharedString::from(format!("preview-{preview:?}")),
                    preview_label(preview),
                )
                .when(model.preview == Some(preview), |b| b.bg(rgb(0xd0d0d0)))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select_preview(Some(preview), cx)),
                ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(rgb(0xb0b0b0))
            .rounded_md()
            .when(model.preview.is_some(), |panel| {
                panel.border_color(rgb(0xd08000)).child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xa06000))
                        .child("PREVIEW — not a real update"),
                )
            })
            .child(self.controls.clone())
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x707070))
                    .child("Preview update UI:"),
            )
            .child(previews)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};
    use gpui_auto_update::UpdaterConfig;

    use super::*;

    const VERSION: &str = "1.0.0";

    fn preview_panel(
        cx: &mut TestAppContext,
        preview: PreviewState,
    ) -> (Entity<Updater>, Entity<UpdatePanel>, &mut VisualTestContext) {
        let updater = cx.update(|cx| {
            gpui_auto_update::init(UpdaterConfig::preview("dev.example.panel", preview), cx)
        });
        cx.run_until_parked();
        let (panel, cx) =
            cx.add_window_view(|window, cx| UpdatePanel::new(updater.clone(), VERSION, window, cx));
        (updater, panel, cx)
    }

    #[gpui::test]
    async fn available_update_offers_download_with_release_notes(cx: &mut TestAppContext) {
        let (_, panel, cx) = preview_panel(cx, PreviewState::UpdateAvailable);
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(model.headline, "Version 0.0.0-preview is available.");
        assert!(model.detail.unwrap().starts_with("Preview:"));
        assert_eq!(
            model.actions,
            vec![
                gpui_auto_update_ui::UpdateAction::Download,
                gpui_auto_update_ui::UpdateAction::Dismiss
            ]
        );
        assert_eq!(model.preview, Some(PreviewState::UpdateAvailable));
        assert_eq!(
            model.automatic_updates, None,
            "no preference while previewing"
        );
    }

    #[gpui::test]
    async fn downloading_shows_progress(cx: &mut TestAppContext) {
        let (_, panel, cx) = preview_panel(cx, PreviewState::Downloading);
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(model.headline, "Downloading version 0.0.0-preview…");
        let progress = model.progress.expect("downloading shows progress");
        assert_eq!(progress.fraction, Some(0.42));
        assert_eq!(progress.label, "42.0 of 100.0 MiB");
        assert!(model.actions.is_empty());
    }

    #[gpui::test]
    async fn staged_update_offers_restart(cx: &mut TestAppContext) {
        let (_, panel, cx) = preview_panel(cx, PreviewState::ReadyToInstall);
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(model.headline, "Version 0.0.0-preview is ready to install.");
        assert_eq!(
            model.actions,
            vec![gpui_auto_update_ui::UpdateAction::RestartToUpdate]
        );
    }

    #[gpui::test]
    async fn error_state_shows_the_message_and_allows_retry(cx: &mut TestAppContext) {
        let (_, panel, cx) = preview_panel(cx, PreviewState::Error);
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(model.headline, "The update failed.");
        assert_eq!(
            model.detail.as_ref().map(|detail| detail.as_ref()),
            Some("Preview: the update server could not be reached.")
        );
        assert_eq!(
            model.actions,
            vec![
                gpui_auto_update_ui::UpdateAction::CheckForUpdates,
                gpui_auto_update_ui::UpdateAction::Dismiss
            ]
        );
    }

    #[gpui::test]
    async fn externally_managed_names_the_manager_and_explains_manual_checks(
        cx: &mut TestAppContext,
    ) {
        let (updater, panel, cx) = preview_panel(cx, PreviewState::ExternallyManaged);
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(
            model.headline,
            "Updates are managed by Preview package manager."
        );
        assert_eq!(
            model.detail.as_ref().map(|detail| detail.as_ref()),
            Some("Use Preview package manager to update this app.")
        );

        updater.update(cx, |updater, cx| updater.check_for_updates(cx));
        cx.run_until_parked();
        assert!(
            panel.read_with(cx, |panel, cx| panel.feedback(cx).is_some()),
            "a manual check always gives visible feedback"
        );
    }

    #[gpui::test]
    async fn switching_preview_states_and_back_to_live(cx: &mut TestAppContext) {
        let (_, panel, cx) = preview_panel(cx, PreviewState::UpdateAvailable);
        panel.update(cx, |panel, cx| {
            panel.select_preview(Some(PreviewState::RestartRequired), cx)
        });
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(
            model.headline,
            "Restart to finish updating to version 0.0.0-preview."
        );
        assert_eq!(
            model.actions,
            vec![gpui_auto_update_ui::UpdateAction::RestartToUpdate]
        );

        panel.update(cx, |panel, cx| panel.select_preview(None, cx));
        cx.run_until_parked();
        let model = panel.read_with(cx, |panel, cx| panel.model(cx));
        assert_eq!(model.preview, None);
        assert_eq!(model.headline, "This installation cannot update itself.");
        assert_eq!(model.automatic_updates, Some(true));
    }
}
