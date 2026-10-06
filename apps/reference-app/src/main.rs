//! Reference application for validating `gpui-auto-update`.
//!
//! This is intentionally minimal: it exists to exercise the updater, not to
//! demonstrate UI. It uses only the public facade, and everything that
//! differs between version N and N+1 is fixed at build time; see
//! [`build_config`].
//!
//! `reference-app --version` prints the compiled version and exits, so
//! end-to-end tests can tell which build is installed.

// `actions!` generates undocumented structs, and a binary has no public API.
#![allow(missing_docs)]

mod build_config;
mod document;
mod setup;
mod unattended;
mod update_ui;

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, Context, Entity, IntoElement, Menu, MenuItem, ParentElement, Render,
    Styled, Subscription, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
    size,
};
use gpui_auto_update::CheckForUpdates;

use build_config::{BuildConfig, BuildConfigError, DEFAULT_APP_ID, DEFAULT_VERSION};
use document::Document;
use unattended::Unattended;
use update_ui::UpdatePanel;

actions!(reference_app, [Quit]);

struct Root {
    title: String,
    config_error: Option<String>,
    document: Entity<Document>,
    panel: Entity<UpdatePanel>,
    _document: Subscription,
    _save_before_install: Subscription,
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.document.read(cx);
        let document_status = format!(
            "Document: {} edit(s), {}.",
            document.edits(),
            if document.is_dirty() {
                "unsaved"
            } else {
                "saved"
            },
        );
        let document_path = format!("Saved before updating to {}", document.path().display());
        let edit = div()
            .id("edit-document")
            .px_2()
            .py_1()
            .border_1()
            .border_color(rgb(0x8a8a8a))
            .rounded_md()
            .cursor_pointer()
            .child("Make an Edit")
            .on_click(cx.listener(|this, _, _, cx| {
                this.document.update(cx, |document, cx| document.edit(cx));
            }));

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(0xffffff))
            .text_color(rgb(0x202020))
            .child(div().text_lg().child(self.title.clone()))
            .when_some(self.config_error.clone(), |root, error| {
                root.child(div().text_sm().text_color(rgb(0xb02020)).child(error))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(edit)
                    .child(div().text_sm().child(document_status)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x707070))
                    .child(document_path),
            )
            .child(self.panel.clone())
    }
}

fn document_path(app_id: &str) -> PathBuf {
    gpui_auto_update::default_preferences_path(app_id)
        .map(|path| path.with_file_name("reference-document.txt"))
        .unwrap_or_else(|_| {
            std::env::temp_dir()
                .join(app_id)
                .join("reference-document.txt")
        })
}

fn main() {
    // On Linux, an update is finished by this executable started as the
    // update helper; that must happen before anything else.
    gpui_auto_update::run_update_helper_if_requested();
    let build: Result<BuildConfig, BuildConfigError> = BuildConfig::compiled();
    let version = build
        .as_ref()
        .map(|build| build.version.as_str().to_owned())
        .unwrap_or_else(|_| DEFAULT_VERSION.to_owned());
    if std::env::args().skip(1).any(|arg| arg == "--version") {
        println!("{version}");
        return;
    }
    if let Err(error) = &build {
        eprintln!("reference-app: {error}");
    }
    if let Ok(build) = &build
        && build.e2e_fail_to_start
    {
        if let Some(report) = &build.e2e_report {
            unattended::record_failed_start(report, &version);
        }
        eprintln!("reference-app: this build is deliberately broken and does not start");
        std::process::exit(1);
    }
    let e2e_report = build
        .as_ref()
        .ok()
        .and_then(|build| build.e2e_report.clone());
    let app_id = build
        .as_ref()
        .map(|build| build.app_id.clone())
        .unwrap_or_else(|_| DEFAULT_APP_ID.to_owned());

    Application::new().run(move |cx: &mut App| {
        let updater = gpui_auto_update::init(setup::updater_config(&build), cx);
        if let Some(report) = &e2e_report {
            let unattended = Unattended::start(&updater, report, &version, cx);
            cx.set_global(unattended);
        }

        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "Reference App".into(),
            items: vec![
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Quit", Quit),
            ],
        }]);

        let config_error = build.as_ref().err().map(ToString::to_string);
        let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let document = cx.new(|_| Document::new(document_path(&app_id)));
                    Root {
                        title: format!("Reference App {version}"),
                        config_error,
                        panel: cx.new(|cx| {
                            UpdatePanel::new(updater.clone(), version.clone(), window, cx)
                        }),
                        _document: cx.observe(&document, |_, _, cx| cx.notify()),
                        _save_before_install: document::save_before_install(
                            &updater, &document, cx,
                        ),
                        document,
                    }
                })
            },
        )
        .expect("failed to open the main window");
        cx.activate(true);
        // The health signal: after an update, the helper keeps the previous
        // version until this arrives and restores it if this one exits first.
        updater.update(cx, |updater, cx| updater.main_window_opened(cx));
    });
}
