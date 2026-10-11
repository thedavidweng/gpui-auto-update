# gpui-auto-update-macos

macOS backend for [gpui-auto-update](https://crates.io/crates/gpui-auto-update). Update mechanics are delegated to Sparkle 2 (`SPUUpdater`) through the [`sparkle-updater`](https://crates.io/crates/sparkle-updater) binding. Most applications should depend on `gpui-auto-update` with its `sparkle` feature instead.

- Manual checks use Sparkle's standard update UI. Background checks use Sparkle's background check.
- Sparkle's scheduled discoveries are adopted into the update state without opening a Sparkle window: Sparkle's gentle reminders route through a `PresentationPolicy`, and the default `GpuiPresentation` leaves scheduled updates to the app. Applications can override the policy with `SparkleBackend::start_with_policy`.
- Sparkle's install-and-relaunch waits for the application's save hooks: when a handoff gate is installed, Sparkle's request to relaunch arrives as `SparkleEvent::RelaunchRequested`, and resuming the one-shot `RelaunchContinuation` lets Sparkle proceed. The `gpui-auto-update` facade installs the gate and resumes once its `on_prepare_to_install` hooks have run.
- Sparkle stores the automatic-update preference, and this crate mirrors it.
- Sparkle-driven downloads, installs, relaunches, and the user's choices in Sparkle's windows appear in the update state.

The real Sparkle engine is behind the opt-in `sparkle` feature because it links `Sparkle.framework`. To build with the feature, set `SPARKLE_FRAMEWORK_PATH` to the directory that contains the framework (`gpui-auto-update sparkle fetch` provides a checksum-verified copy). Without the feature, the crate is pure Rust and builds on every target. See `docs/adr/0002-sparkle-binding.md` in the repository.

Licensed under either of MIT or Apache-2.0 at your option.
