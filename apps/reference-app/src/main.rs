//! Reference application for validating `gpui-auto-update`.
//!
//! This is intentionally minimal: it exists to exercise the updater, not to
//! demonstrate UI. Updater integration is added as the facade lands.

// `actions!` generates undocumented structs, and a binary has no public API.
#![allow(missing_docs)]

use gpui::{
    App, Application, Bounds, Context, IntoElement, Menu, MenuItem, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, size,
};

actions!(reference_app, [Quit]);

struct Root;

impl Render for Root {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(format!(
                "gpui-auto-update reference app {}",
                env!("CARGO_PKG_VERSION")
            ))
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "Reference App".into(),
            items: vec![MenuItem::action("Quit", Quit)],
        }]);
        let bounds = Bounds::centered(None, size(px(480.), px(240.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Root),
        )
        .expect("failed to open the main window");
        cx.activate(true);
    });
}
