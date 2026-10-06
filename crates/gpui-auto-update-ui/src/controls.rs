//! The neutral update controls.

use gpui::{
    App, Context, Div, Entity, FocusHandle, Global, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px, relative, rgb,
};
use gpui_auto_update::core::{CheckKind, UpdateError};
use gpui_auto_update::{Updater, UpdaterEvent};

use crate::summary::{Tone, UpdateAction, UpdateSummary};

/// The few colors the neutral controls cannot inherit from the surrounding
/// application: the accent for what needs attention and the danger color for
/// failures. Everything else (text, fonts, sizes) inherits from the context
/// the controls render in.
///
/// The defaults read on both light and dark backgrounds. Applications can
/// replace them with their own theme colors through [`set_theme`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlTheme {
    /// Marks what the user can act on and shows progress.
    pub accent: Hsla,
    /// Marks failures.
    pub danger: Hsla,
}

impl Default for ControlTheme {
    fn default() -> Self {
        Self {
            accent: rgb(0x3070d0).into(),
            danger: rgb(0xb02020).into(),
        }
    }
}

impl Global for ControlTheme {}

/// Installs the colors the neutral controls use. Optional; see
/// [`ControlTheme`].
pub fn set_theme(theme: ControlTheme, cx: &mut App) {
    cx.set_global(theme);
}

fn theme(cx: &App) -> ControlTheme {
    cx.try_global::<ControlTheme>().copied().unwrap_or_default()
}

fn neutral_border() -> Hsla {
    rgb(0x808080).into()
}

/// A complete update panel: status text, download or install progress, the
/// actions the state offers, the automatic-updates setting, and error or
/// up-to-date feedback.
///
/// Every control is a tab stop; `Tab` and `Shift-Tab` move through them and
/// `Enter` or `Space` activates the focused one. The panel is a GPUI tab
/// group, so its tab indices do not disturb the host application's tab
/// order. Each control's text is its label; self-contained descriptions for
/// assistive technology are available from
/// [`UpdateSummary::accessible_label`], and each control has a stable
/// element id ([`UpdateAction::element_id`]).
///
/// The panel takes focus when created so keyboard navigation works even
/// when the application never moves focus itself.
pub struct UpdateControls {
    updater: Entity<Updater>,
    current_version: Option<String>,
    root_focus: FocusHandle,
    action_focus: [FocusHandle; 4],
    setting_focus: FocusHandle,
    /// Feedback for the last manual check or failed operation that the state
    /// alone does not show, such as a check refused by an externally managed
    /// installation.
    feedback: Option<SharedString>,
    _observe: Subscription,
    _events: Subscription,
}

impl UpdateControls {
    /// Creates the controls for `updater` and focuses them; see the type
    /// documentation.
    pub fn new(updater: Entity<Updater>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root_focus = cx.focus_handle();
        window.focus(&root_focus);
        let observe = cx.observe(&updater, |_, _, cx| cx.notify());
        let events = cx.subscribe(&updater, |this, _, event: &UpdaterEvent, cx| {
            match event {
                UpdaterEvent::CheckFinished {
                    kind: CheckKind::Manual,
                    result,
                } => {
                    this.feedback = result
                        .as_ref()
                        .err()
                        .map(|error| error.message().to_owned().into());
                }
                UpdaterEvent::Failed(error) => {
                    this.feedback = Some(error.message().to_owned().into());
                }
                _ => return,
            }
            cx.notify();
        });
        Self {
            updater,
            current_version: None,
            root_focus,
            action_focus: [(); 4].map(|()| cx.focus_handle().tab_stop(true)),
            setting_focus: cx.focus_handle().tab_stop(true),
            feedback: None,
            _observe: observe,
            _events: events,
        }
    }

    /// Names this version in the idle and up-to-date headlines.
    pub fn with_current_version(mut self, version: impl Into<String>) -> Self {
        self.current_version = Some(version.into());
        self
    }

    /// What the controls currently show.
    pub fn summary(&self, cx: &App) -> UpdateSummary {
        UpdateSummary::new(self.updater.read(cx), self.current_version.as_deref())
    }

    /// Feedback that the state does not show; see the field documentation.
    pub fn feedback(&self) -> Option<&str> {
        self.feedback.as_deref().map(|text| &**text)
    }

    fn perform(&mut self, action: UpdateAction, cx: &mut Context<Self>) {
        let result: Result<(), UpdateError> = self.updater.update(cx, |updater, cx| match action {
            UpdateAction::CheckForUpdates => {
                updater.check_for_updates(cx);
                Ok(())
            }
            UpdateAction::Download => updater.request_install(cx),
            UpdateAction::RestartToUpdate => updater.restart_to_update(cx),
            UpdateAction::Dismiss => updater.dismiss(cx),
        });
        if let Err(error) = result {
            self.feedback = Some(error.message().to_owned().into());
        }
        cx.notify();
    }

    fn set_automatic_updates(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let result = self
            .updater
            .update(cx, |updater, cx| updater.set_automatic_checks(enabled, cx));
        if let Err(error) = result {
            self.feedback = Some(error.message().to_owned().into());
        }
        cx.notify();
    }

    fn on_tab(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key != "tab" {
            return;
        }
        if event.keystroke.modifiers.shift {
            window.focus_prev();
        } else {
            window.focus_next();
        }
        cx.stop_propagation();
    }

    fn action_focus(&self, action: UpdateAction) -> &FocusHandle {
        let index = match action {
            UpdateAction::CheckForUpdates => 0,
            UpdateAction::Download => 1,
            UpdateAction::RestartToUpdate => 2,
            UpdateAction::Dismiss => 3,
        };
        &self.action_focus[index]
    }

    fn action_button(
        &self,
        action: UpdateAction,
        position: isize,
        window: &Window,
        accent: Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        // The tracked handle carries the tab index; element-level tab
        // indices are ignored once a handle is tracked.
        let handle = self.action_focus(action).clone().tab_index(position);
        let focused = handle.is_focused(window);
        control_style(
            div().id(action.element_id()).track_focus(&handle),
            focused,
            accent,
        )
        .child(action.label())
        .on_click(cx.listener(move |this, _, _, cx| this.perform(action, cx)))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                this.perform(action, cx);
                cx.stop_propagation();
            }
        }))
    }
}

fn control_style(div: gpui::Stateful<Div>, focused: bool, accent: Hsla) -> gpui::Stateful<Div> {
    div.px_2()
        .py_1()
        .border_1()
        .rounded_md()
        .cursor_pointer()
        .border_color(if focused { accent } else { neutral_border() })
        .hover(|style| style.border_color(accent))
}

impl Render for UpdateControls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary(cx);
        let theme = theme(cx);
        let detail_color: Hsla = match summary.tone {
            Tone::Critical => theme.danger,
            _ => rgb(0x808080).into(),
        };

        let mut actions = div().flex().flex_wrap().gap_2();
        for (position, &action) in summary.actions.iter().enumerate() {
            actions = actions.child(self.action_button(
                action,
                position as isize,
                window,
                theme.accent,
                cx,
            ));
        }

        let setting = summary.automatic_updates.map(|enabled| {
            let mark = if enabled { "[x]" } else { "[ ]" };
            let handle = self
                .setting_focus
                .clone()
                .tab_index(summary.actions.len() as isize);
            let focused = handle.is_focused(window);
            let next = !enabled;
            control_style(
                div().id("update-automatic-setting").track_focus(&handle),
                focused,
                theme.accent,
            )
            .child(format!("{mark} Check for updates automatically"))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_automatic_updates(next, cx);
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.set_automatic_updates(next, cx);
                    cx.stop_propagation();
                }
            }))
        });

        let progress = summary.progress.map(|progress| {
            let bar = div()
                .w_full()
                .h(px(6.))
                .rounded_md()
                .bg(neutral_border().opacity(0.3));
            let bar = match progress.fraction {
                Some(fraction) => bar.child(
                    div()
                        .h_full()
                        .rounded_md()
                        .bg(theme.accent)
                        .w(relative(fraction as f32)),
                ),
                // An unknown total or an unmeasurable step shows a partial
                // segment: progress is happening, but not countable.
                None => bar.child(
                    div()
                        .h_full()
                        .w(relative(0.3))
                        .mx_auto()
                        .rounded_md()
                        .bg(theme.accent.opacity(0.6)),
                ),
            };
            div().flex().flex_col().gap_1().child(bar).child(
                div()
                    .text_sm()
                    .text_color(detail_color)
                    .child(progress.label),
            )
        });

        div()
            .id("update-controls")
            .track_focus(&self.root_focus)
            .tab_group()
            .on_key_down(cx.listener(Self::on_tab))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .border_1()
            .border_color(neutral_border().opacity(0.6))
            .rounded_md()
            .child(div().child(summary.headline.clone()))
            .when_some(summary.detail.clone(), |panel, detail| {
                panel.child(div().text_sm().text_color(detail_color).child(detail))
            })
            .children(progress)
            .when_some(self.feedback.clone(), |panel, feedback| {
                panel.child(div().text_sm().text_color(theme.danger).child(feedback))
            })
            .when_some(summary.install_notice.clone(), |panel, notice| {
                panel.child(div().text_xs().text_color(rgb(0x808080)).child(notice))
            })
            .child(actions)
            .children(setting)
    }
}

/// An unobtrusive affordance for an application's chrome: a small dot plus
/// one line, visible only when there is an update to download or a restart
/// to finish, offering exactly that one action.
///
/// It is a tab stop like the [`UpdateControls`] controls and activates with
/// `Enter` or `Space` as well as a click. When there is nothing to act on it
/// renders nothing.
pub struct UpdateIndicator {
    updater: Entity<Updater>,
    focus: FocusHandle,
    /// The last activation failure, which the state alone may not show.
    feedback: Option<SharedString>,
    _observe: Subscription,
}

impl UpdateIndicator {
    /// Creates the affordance for `updater`.
    pub fn new(updater: Entity<Updater>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&updater, |_, _, cx| cx.notify());
        Self {
            updater,
            focus: cx.focus_handle().tab_stop(true).tab_index(0),
            feedback: None,
            _observe: observe,
        }
    }

    /// The one action the affordance offers right now, if any. `None` means
    /// the affordance renders nothing.
    pub fn attention(&self, cx: &App) -> Option<UpdateAction> {
        UpdateSummary::new(self.updater.read(cx), None).attention
    }

    /// The last activation failure, if any.
    pub fn feedback(&self) -> Option<&str> {
        self.feedback.as_deref().map(|text| &**text)
    }

    fn activate(&mut self, action: UpdateAction, cx: &mut Context<Self>) {
        let result = self.updater.update(cx, |updater, cx| match action {
            UpdateAction::Download => updater.request_install(cx),
            UpdateAction::RestartToUpdate => updater.restart_to_update(cx),
            _ => Ok(()),
        });
        if let Err(error) = result {
            self.feedback = Some(error.message().to_owned().into());
        }
        cx.notify();
    }
}

impl Render for UpdateIndicator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = UpdateSummary::new(self.updater.read(cx), None);
        let Some(action) = summary.attention else {
            return div().into_any_element();
        };
        let theme = theme(cx);
        let focused = self.focus.is_focused(window);
        let mut element = div()
            .id("update-indicator")
            .track_focus(&self.focus)
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .when(focused, |this| this.border_1().border_color(theme.accent))
            .child(div().size(px(8.)).rounded_full().bg(match summary.tone {
                Tone::Critical => theme.danger,
                _ => theme.accent,
            }))
            .child(summary.accessible_label(action))
            .on_click(cx.listener(move |this, _, _, cx| this.activate(action, cx)))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.activate(action, cx);
                    cx.stop_propagation();
                }
            }));
        if let Some(feedback) = self.feedback.clone() {
            element = element.child(div().text_sm().text_color(theme.danger).child(feedback));
        }
        element.into_any_element()
    }
}
