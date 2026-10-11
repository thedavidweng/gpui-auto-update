# gpui-auto-update-ui

Optional neutral, theme-inheriting GPUI update controls for
[gpui-auto-update](https://crates.io/crates/gpui-auto-update). The crate
depends only on GPUI and the facade — no design system, and no assumptions
about your layout (no sidebar required).

## Controls

- **`UpdateControls`** — a complete update panel: status text, download and
  install progress, the actions the current state offers (check for updates,
  download, restart to update, dismiss), the automatic-updates setting, and
  error or up-to-date feedback.
- **`UpdateIndicator`** — an unobtrusive affordance for application chrome:
  a dot and one line offering the single action that currently matters, or
  nothing at all.
- **`UpdateSummary`** — the plain-data model behind both, computed from the
  facade's `Updater`; use it as a starting point for your own rendering.

Every control is keyboard navigable (`Tab` / `Shift-Tab` to move, `Enter` or
`Space` to activate) with stable element ids, and
`UpdateSummary::accessible_label` provides self-contained descriptions of
each action. Colors come from the surrounding text styles plus a small
overridable [`ControlTheme`](via `set_theme`); text and fonts inherit from
wherever you render the controls.

```rust,ignore
let controls = cx.new(|cx| {
    UpdateControls::new(updater.clone(), window, cx).with_current_version("1.4.0")
});
```

Applications can replace these controls entirely by rendering
`updater.read(cx).state()` themselves — see
`apps/reference-app/examples/custom_update_ui.rs` in the repository for a
fully custom rendering driven by the same state.

Licensed under either of MIT or Apache-2.0 at your option.
