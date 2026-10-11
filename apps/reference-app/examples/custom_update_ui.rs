//! Fully custom update UI rendered from the same `Updater` state the
//! neutral controls in `gpui-auto-update-ui` use.
//!
//! Applications can ignore the neutral controls entirely and render the
//! facade's observable state however they like; this example does exactly
//! that with its own wording, layout, and colors, calling the `Updater`
//! methods directly instead of dispatching the standard actions.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p gpui-auto-update-reference-app --example custom_update_ui
//! ```
//!
//! It drives the updater through the marked [`PreviewState`]s, so nothing
//! is checked, downloaded, or installed.

// A binary example has no public API.
#![allow(missing_docs)]

use gpui::{
    App, Application, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Subscription, Window, WindowBounds, WindowOptions, div, prelude::*, px, relative, rgb, size,
};
use gpui_auto_update::core::UpdateState;
use gpui_auto_update::{PreviewState, Updater, UpdaterConfig};

struct CustomUpdateCard {
    updater: Entity<Updater>,
    preview_index: usize,
    _observe: Subscription,
}

impl CustomUpdateCard {
    fn new(updater: Entity<Updater>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&updater, |_, _, cx| cx.notify()),
            updater,
            preview_index: 0,
        }
    }

    fn next_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_index = (self.preview_index + 1) % PreviewState::ALL.len();
        let preview = PreviewState::ALL[self.preview_index];
        self.updater
            .update(cx, |updater, cx| updater.enter_preview(preview, cx));
    }
}

fn pill(text: impl IntoElement, color: u32) -> gpui::Div {
    div()
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(color))
        .text_xs()
        .text_color(rgb(0xffffff))
        .child(text)
}

fn custom_button(
    id: &'static str,
    label: &'static str,
    updater: Entity<Updater>,
    action: impl Fn(&mut Updater, &mut Context<Updater>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0x334155))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x475569)))
        .child(label)
        .on_click(move |_, _, cx| {
            updater.update(cx, |updater, cx| action(updater, cx));
        })
}

impl Render for CustomUpdateCard {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let updater = self.updater.read(cx);
        let state = updater.state();

        // The app's own wording and accent colors per state.
        let (headline, pill_label, pill_color) = match &state {
            UpdateState::Idle => ("All quiet.".to_owned(), "IDLE", 0x64748b),
            UpdateState::Checking => ("Pinging the mothership…".to_owned(), "CHECKING", 0x0ea5e9),
            UpdateState::UpToDate => ("Nothing newer out there.".to_owned(), "CURRENT", 0x22c55e),
            UpdateState::Available(update) => (
                format!("{} just dropped.", update.version),
                "NEW RELEASE",
                0xf59e0b,
            ),
            UpdateState::Downloading { update, .. } => (
                format!("Fetching {}…", update.version),
                "INCOMING",
                0x0ea5e9,
            ),
            UpdateState::Verifying(update) => (
                format!("Checking {}'s papers…", update.version),
                "VERIFYING",
                0x0ea5e9,
            ),
            UpdateState::Staged(update) => (
                format!("{} is on the launchpad.", update.version),
                "READY",
                0xf59e0b,
            ),
            UpdateState::Installing(update) => (
                format!("Bolting in {}…", update.version),
                "INSTALLING",
                0x0ea5e9,
            ),
            UpdateState::WaitingForQuit(update) => (
                format!("One restart between you and {}.", update.version),
                "RESTART",
                0xf59e0b,
            ),
            UpdateState::Relaunching(update) => (
                format!("See you in {}.", update.version),
                "RESTARTING",
                0x0ea5e9,
            ),
            UpdateState::RolledBack { update, .. } => (
                format!("{} didn't make it; we went back.", update.version),
                "ROLLED BACK",
                0xef4444,
            ),
            UpdateState::Completed(update) => {
                (format!("Welcome to {}.", update.version), "DONE", 0x22c55e)
            }
            UpdateState::Failed(error) => (error.message().to_owned(), "FAILED", 0xef4444),
            UpdateState::Disabled { .. } => {
                ("Updates handled elsewhere.".to_owned(), "MANAGED", 0x64748b)
            }
            state => (format!("{state:?}"), "STATE", 0x64748b),
        };

        let mut buttons = div().flex().gap_2();
        match &state {
            UpdateState::Available(_) => {
                buttons = buttons
                    .child(custom_button(
                        "grab",
                        "Grab it",
                        self.updater.clone(),
                        |updater, cx| {
                            // Rejected while previewing; a real app would let it
                            // download and stage the update.
                            let _ = updater.request_install(cx);
                        },
                    ))
                    .child(custom_button(
                        "later",
                        "Later",
                        self.updater.clone(),
                        |updater, cx| {
                            let _ = updater.dismiss(cx);
                        },
                    ));
            }
            UpdateState::Staged(_) | UpdateState::WaitingForQuit(_) => {
                buttons = buttons.child(custom_button(
                    "restart",
                    "Restart now",
                    self.updater.clone(),
                    |updater, cx| {
                        let _ = updater.restart_to_update(cx);
                    },
                ));
            }
            _ => {
                buttons = buttons.child(custom_button(
                    "check",
                    "Check again",
                    self.updater.clone(),
                    |updater, cx| {
                        updater.check_for_updates(cx);
                    },
                ));
            }
        }

        let progress = match &state {
            UpdateState::Downloading { progress, .. } => progress.fraction(),
            _ => None,
        };

        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0x0f172a))
            .text_color(rgb(0xe2e8f0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .w(px(420.))
                    .p_6()
                    .rounded_xl()
                    .bg(rgb(0x1e293b))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_lg().child(headline))
                            .child(pill(pill_label, pill_color)),
                    )
                    .when_some(progress, |card, fraction| {
                        card.child(
                            div()
                                .h(px(4.))
                                .w_full()
                                .rounded_full()
                                .bg(rgb(0x334155))
                                .child(
                                    div()
                                        .h_full()
                                        .rounded_full()
                                        .bg(rgb(0x38bdf8))
                                        .w(relative(fraction as f32)),
                                ),
                        )
                    })
                    .child(buttons)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x94a3b8))
                            .child("Custom rendering; previews only — nothing is installed."),
                    )
                    .child(
                        div()
                            .id("next-preview")
                            .text_xs()
                            .text_color(rgb(0x38bdf8))
                            .cursor_pointer()
                            .child("Next preview state →")
                            .on_click(cx.listener(|this, _, _, cx| this.next_preview(cx))),
                    ),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let updater = gpui_auto_update::init(
            UpdaterConfig::preview(
                "dev.example.custom-update-ui",
                PreviewState::UpdateAvailable,
            ),
            cx,
        );
        let bounds = Bounds::centered(None, size(px(560.), px(320.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| CustomUpdateCard::new(updater, cx)),
        )
        .expect("failed to open the example window");
        cx.activate(true);
    });
}
