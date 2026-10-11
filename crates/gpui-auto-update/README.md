# gpui-auto-update

**Sparkle-quality automatic updates for GPUI.**

**Sparkle on macOS. Signed native updates on Windows and Linux. One observable GPUI API.**

This is the crate GPUI applications depend on. It exposes the updater as an
observable GPUI entity (`Updater`) with standard actions (`CheckForUpdates`,
`InstallUpdate`, `RestartToUpdate`, `DismissUpdate`), runs all network and
filesystem work off the foreground thread, and selects the backend for the
target: Sparkle 2 on macOS, and signed native feeds on Windows and Linux.

Adding this dependency lets your application check for and install updates. It
does not embed Sparkle, package your app, sign anything, or publish a release;
those steps are done with the `gpui-auto-update` command-line tool from
[`gpui-auto-update-cli`](https://crates.io/crates/gpui-auto-update-cli) and are
described in the project documentation.

> **Pre-release.** The crates have not been published to crates.io yet, and the
> API can still change. Verification is fail-closed: shipping an updater without
> artifact signature verification is not a supported production configuration.

- Quick start, platform setup, and the release workflow: the
  [project README](https://github.com/thedavidweng/gpui-auto-update#readme)
- API reference: [docs.rs/gpui-auto-update](https://docs.rs/gpui-auto-update)
- Guides: [documentation index](https://github.com/thedavidweng/gpui-auto-update/tree/main/docs)
- Features: `sparkle` links `Sparkle.framework` on macOS (it needs
  `SPARKLE_FRAMEWORK_PATH` at build time).

Licensed under either of MIT or Apache-2.0 at your option.
