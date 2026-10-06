//! Everything the reference app shows about updates.
//!
//! [`panel_model`] decides what to show from the updater alone; the
//! [`UpdatePanel`] view only draws that model with plain GPUI elements, so
//! the drawing can be replaced (for example by neutral controls) without
//! touching the rest of the app.

use gpui::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px, relative, rgb,
};
use gpui_auto_update::core::{Capability, CheckKind, ReleaseNotes, UpdateError, UpdateState};
use gpui_auto_update::{
    CheckForUpdates, DismissUpdate, InstallUpdate, PreviewState, RestartToUpdate, Updater,
    UpdaterEvent,
};

/// A button the panel offers for the current state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    CheckForUpdates,
    /// Download and stage the available update.
    Download,
    RestartToUpdate,
    Dismiss,
}

impl PanelAction {
    fn label(self) -> &'static str {
        match self {
            Self::CheckForUpdates => "Check for Updates",
            Self::Download => "Download Update",
            Self::RestartToUpdate => "Restart to Update",
            Self::Dismiss => "Dismiss",
        }
    }

    fn dispatch(self, cx: &mut App) {
        match self {
            Self::CheckForUpdates => cx.dispatch_action(&CheckForUpdates),
            Self::Download => cx.dispatch_action(&InstallUpdate),
            Self::RestartToUpdate => cx.dispatch_action(&RestartToUpdate),
            Self::Dismiss => cx.dispatch_action(&DismissUpdate),
        }
    }
}

/// What the update panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelModel {
    /// One line saying what the updater is doing.
    pub status: String,
    /// Supporting text: release notes, an error message, or advice.
    pub detail: Option<String>,
    /// Download progress in `0.0..=1.0`, while it is known.
    pub progress: Option<f64>,
    pub actions: Vec<PanelAction>,
    /// The automatic-update preference, once it can be changed.
    pub automatic_checks: Option<bool>,
    /// Why this build will not install updates, when it will not.
    pub install_notice: Option<String>,
    pub preview: Option<PreviewState>,
}

/// What to show for `updater`, given the running `current_version`.
pub fn panel_model(updater: &Updater, current_version: &str) -> PanelModel {
    use PanelAction::*;

    let (status, detail, progress, actions) = match updater.state() {
        UpdateState::Disabled { capability } => {
            let (status, detail) = disabled(&capability);
            (status, detail, None, vec![CheckForUpdates])
        }
        UpdateState::Idle => (
            format!("Version {current_version}"),
            None,
            None,
            vec![CheckForUpdates],
        ),
        UpdateState::Checking => ("Checking for updates…".into(), None, None, vec![]),
        UpdateState::UpToDate => (
            format!("Version {current_version} is up to date."),
            None,
            None,
            vec![CheckForUpdates, Dismiss],
        ),
        UpdateState::Available(update) => (
            format!("Version {} is available.", update.version),
            notes(update.release_notes.as_ref()),
            None,
            vec![Download, Dismiss],
        ),
        UpdateState::Downloading { update, progress } => {
            const MIB: f64 = 1024.0 * 1024.0;
            let downloaded = progress.downloaded as f64 / MIB;
            let detail = match progress.total {
                Some(total) => format!("{downloaded:.1} of {:.1} MiB", total as f64 / MIB),
                None => format!("{downloaded:.1} MiB"),
            };
            (
                format!("Downloading version {}…", update.version),
                Some(detail),
                progress.fraction(),
                vec![],
            )
        }
        UpdateState::Verifying(update) => (
            format!("Verifying version {}…", update.version),
            None,
            None,
            vec![],
        ),
        UpdateState::Staged(update) => (
            format!("Version {} is ready to install.", update.version),
            Some("Your document is saved before the app restarts.".into()),
            None,
            vec![RestartToUpdate],
        ),
        UpdateState::Installing(update) => (
            format!("Installing version {}…", update.version),
            None,
            None,
            vec![],
        ),
        UpdateState::WaitingForQuit(update) => (
            format!("Restart to finish updating to version {}.", update.version),
            None,
            None,
            vec![RestartToUpdate],
        ),
        UpdateState::Relaunching(update) => (
            format!("Restarting into version {}…", update.version),
            None,
            None,
            vec![],
        ),
        UpdateState::RolledBack { update, error } => (
            format!("Version {} was rolled back.", update.version),
            Some(error.message().to_owned()),
            None,
            vec![CheckForUpdates, Dismiss],
        ),
        UpdateState::Completed(update) => (
            format!("Updated to version {}.", update.version),
            None,
            None,
            vec![RestartToUpdate, Dismiss],
        ),
        UpdateState::Failed(error) => (
            "The update failed.".into(),
            Some(error.message().to_owned()),
            None,
            vec![CheckForUpdates, Dismiss],
        ),
        state => (format!("{state:?}"), None, None, vec![CheckForUpdates]),
    };

    let previewing = updater.is_preview();
    let install_notice =
        (!previewing && updater.is_self_update_supported() && !updater.installs_allowed())
            .then(|| "This development build checks for updates but does not install them.".into());

    PanelModel {
        status,
        detail,
        progress,
        actions,
        automatic_checks: (updater.is_ready() && !previewing)
            .then(|| updater.automatic_checks_enabled()),
        install_notice,
        preview: updater.preview(),
    }
}

fn disabled(capability: &Capability) -> (String, Option<String>) {
    match capability {
        Capability::ExternallyManaged { manager } => {
            let manager = manager.as_deref().unwrap_or("your package manager");
            (
                format!("Updates are managed by {manager}."),
                Some(format!("Use {manager} to update this app.")),
            )
        }
        Capability::Unsupported => (
            "This installation cannot update itself.".into(),
            Some("Install a signed release of the app to receive updates.".into()),
        ),
        Capability::TemporarilyUnavailable => ("Updates are unavailable right now.".into(), None),
        _ => ("Updates are turned off.".into(), None),
    }
}

fn notes(notes: Option<&ReleaseNotes>) -> Option<String> {
    match notes? {
        ReleaseNotes::Inline { content, .. } => Some(content.clone()),
        ReleaseNotes::Link(url) => Some(format!("Release notes: {url}")),
        _ => None,
    }
}

/// The update section of the main window.
pub struct UpdatePanel {
    updater: Entity<Updater>,
    current_version: String,
    /// Feedback for the last manual check or failed operation that the state
    /// alone does not show, such as a check refused by an externally managed
    /// installation.
    notice: Option<String>,
    _observe: Subscription,
    _events: Subscription,
}

impl UpdatePanel {
    pub fn new(
        updater: Entity<Updater>,
        current_version: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&updater, |_, _, cx| cx.notify());
        let events = cx.subscribe(&updater, |this, _, event: &UpdaterEvent, cx| {
            match event {
                UpdaterEvent::CheckFinished {
                    kind: CheckKind::Manual,
                    result,
                } => this.notice = result.as_ref().err().map(describe),
                UpdaterEvent::Failed(error) => this.notice = Some(describe(error)),
                UpdaterEvent::StateChanged => {}
                _ => return,
            }
            cx.notify();
        });
        Self {
            updater,
            current_version: current_version.into(),
            notice: None,
            _observe: observe,
            _events: events,
        }
    }

    pub fn model(&self, cx: &App) -> PanelModel {
        panel_model(self.updater.read(cx), &self.current_version)
    }

    /// Feedback that the state does not show; see the field documentation.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    fn toggle_automatic_checks(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let result = self
            .updater
            .update(cx, |updater, cx| updater.set_automatic_checks(enabled, cx));
        if let Err(error) = result {
            self.notice = Some(describe(&error));
            cx.notify();
        }
    }

    fn select_preview(&mut self, preview: Option<PreviewState>, cx: &mut Context<Self>) {
        self.notice = None;
        self.updater.update(cx, |updater, cx| match preview {
            Some(preview) => updater.enter_preview(preview, cx),
            None => updater.exit_preview(cx),
        });
    }
}

fn describe(error: &UpdateError) -> String {
    error.message().to_owned()
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

fn button(
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

impl Render for UpdatePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model(cx);

        let actions = model.actions.iter().map(|&action| {
            button(action.label(), action.label()).on_click(move |_, _, cx| action.dispatch(cx))
        });

        let preference = model.automatic_checks.map(|enabled| {
            let mark = if enabled { "[x]" } else { "[ ]" };
            button(
                "automatic-checks",
                format!("{mark} Check for updates automatically"),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_automatic_checks(!enabled, cx);
            }))
        });

        let mut previews = div().flex().flex_wrap().gap_1().text_xs().child(
            button("preview-live", "Live")
                .when(model.preview.is_none(), |b| b.bg(rgb(0xd0d0d0)))
                .on_click(cx.listener(|this, _, _, cx| this.select_preview(None, cx))),
        );
        for preview in PreviewState::ALL {
            previews = previews.child(
                button(
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
            .child(div().child(model.status))
            .when_some(model.detail, |panel, detail| {
                panel.child(div().text_sm().text_color(rgb(0x505050)).child(detail))
            })
            .when_some(model.progress, |panel, fraction| {
                panel.child(
                    div()
                        .w_full()
                        .h(px(6.))
                        .rounded_md()
                        .bg(rgb(0xdddddd))
                        .child(
                            div()
                                .h_full()
                                .rounded_md()
                                .bg(rgb(0x3070d0))
                                .w(relative(fraction as f32)),
                        ),
                )
            })
            .when_some(self.notice().map(str::to_owned), |panel, notice| {
                panel.child(div().text_sm().text_color(rgb(0xb02020)).child(notice))
            })
            .when_some(model.install_notice, |panel, notice| {
                panel.child(div().text_xs().text_color(rgb(0x707070)).child(notice))
            })
            .child(div().flex().gap_2().children(actions))
            .children(preference)
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
    use gpui::{AppContext as _, TestAppContext};
    use gpui_auto_update::UpdaterConfig;

    use super::*;

    const VERSION: &str = "1.0.0";

    fn preview_panel(cx: &mut TestAppContext, preview: PreviewState) -> Entity<UpdatePanel> {
        let updater = cx.update(|cx| {
            gpui_auto_update::init(UpdaterConfig::preview("dev.example.panel", preview), cx)
        });
        cx.run_until_parked();
        cx.new(|cx| UpdatePanel::new(updater, VERSION, cx))
    }

    fn read_model(cx: &mut TestAppContext, panel: &Entity<UpdatePanel>) -> PanelModel {
        panel.read_with(cx, |panel, cx| panel.model(cx))
    }

    #[gpui::test]
    fn available_update_offers_download_with_release_notes(cx: &mut TestAppContext) {
        let panel = preview_panel(cx, PreviewState::UpdateAvailable);
        let model = read_model(cx, &panel);
        assert_eq!(model.status, "Version 0.0.0-preview is available.");
        assert!(model.detail.unwrap().starts_with("Preview:"));
        assert_eq!(
            model.actions,
            vec![PanelAction::Download, PanelAction::Dismiss]
        );
        assert_eq!(model.preview, Some(PreviewState::UpdateAvailable));
        assert_eq!(
            model.automatic_checks, None,
            "no preference while previewing"
        );
    }

    #[gpui::test]
    fn downloading_shows_progress(cx: &mut TestAppContext) {
        let panel = preview_panel(cx, PreviewState::Downloading);
        let model = read_model(cx, &panel);
        assert_eq!(model.status, "Downloading version 0.0.0-preview…");
        assert_eq!(model.detail.as_deref(), Some("42.0 of 100.0 MiB"));
        assert_eq!(model.progress, Some(0.42));
        assert!(model.actions.is_empty());
    }

    #[gpui::test]
    fn staged_update_offers_restart(cx: &mut TestAppContext) {
        let panel = preview_panel(cx, PreviewState::ReadyToInstall);
        let model = read_model(cx, &panel);
        assert_eq!(model.status, "Version 0.0.0-preview is ready to install.");
        assert_eq!(model.actions, vec![PanelAction::RestartToUpdate]);
    }

    #[gpui::test]
    fn error_state_shows_the_message_and_allows_retry(cx: &mut TestAppContext) {
        let panel = preview_panel(cx, PreviewState::Error);
        let model = read_model(cx, &panel);
        assert_eq!(model.status, "The update failed.");
        assert_eq!(
            model.detail.as_deref(),
            Some("Preview: the update server could not be reached.")
        );
        assert_eq!(
            model.actions,
            vec![PanelAction::CheckForUpdates, PanelAction::Dismiss]
        );
    }

    #[gpui::test]
    fn externally_managed_names_the_manager_and_explains_manual_checks(cx: &mut TestAppContext) {
        let panel = preview_panel(cx, PreviewState::ExternallyManaged);
        let model = read_model(cx, &panel);
        assert_eq!(
            model.status,
            "Updates are managed by Preview package manager."
        );
        assert_eq!(
            model.detail.as_deref(),
            Some("Use Preview package manager to update this app.")
        );

        cx.update(|cx| cx.dispatch_action(&CheckForUpdates));
        cx.run_until_parked();
        assert!(
            panel.read_with(cx, |panel, _| panel.notice().is_some()),
            "a manual check always gives visible feedback"
        );
    }

    #[gpui::test]
    fn switching_preview_states_and_back_to_live(cx: &mut TestAppContext) {
        let panel = preview_panel(cx, PreviewState::UpdateAvailable);
        panel.update(cx, |panel, cx| {
            panel.select_preview(Some(PreviewState::RestartRequired), cx)
        });
        let model = read_model(cx, &panel);
        assert_eq!(
            model.status,
            "Restart to finish updating to version 0.0.0-preview."
        );
        assert_eq!(model.actions, vec![PanelAction::RestartToUpdate]);

        panel.update(cx, |panel, cx| panel.select_preview(None, cx));
        cx.run_until_parked();
        let model = read_model(cx, &panel);
        assert_eq!(model.preview, None);
        assert_eq!(model.status, "This installation cannot update itself.");
        assert_eq!(model.automatic_checks, Some(true));
    }
}
