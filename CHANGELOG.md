# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-beta.1](https://github.com/thedavidweng/gpui-auto-update/releases/tag/gpui-auto-update-v0.1.0-beta.1) - 2026-10-28

### Other

- Complete the README and documentation (T26, #27)
- Select release channels on Windows and Linux through the facade (T23, #24)
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T10-macos-background
- Adopt Sparkle's scheduled discoveries and coordinate relaunch (T10, #11)
- Merge T21 ([#22](https://github.com/thedavidweng/gpui-auto-update/pull/22)): Linux helper: swap, health confirmation, rollback
- Merge T17 ([#18](https://github.com/thedavidweng/gpui-auto-update/pull/18)): Windows backend
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T21-linux-helper
- Add Linux update helper with swap, health confirmation, and rollback (T21, #22)
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T17-windows-backend
- Add Windows backend with Inno Setup handoff and portable updates (T17, #18)
- Merge remote-tracking branch 'origin/spec-1-integration' into ticket/T9-macos-backend
- Add the Sparkle 2 macOS backend (T9, #10)
- Judge preference base directories by the target platform's path rules
- Merge T25 ([#26](https://github.com/thedavidweng/gpui-auto-update/pull/26)): Project release engineering
- Let backends drive the coordinator directly while staging (T6, #7)
- Add GPUI facade entity, actions, and backend contract (T6, #7)
- Add release-plz release automation and fresh-consumer packaging check (T25, #26)
- Bootstrap workspace, CI, and repository scaffolding (T1, #2)

### Added

- Documentation: a complete README (platform matrix, macOS-first quick start
  with a cross-platform continuation, acknowledgements) and guides for the
  security and trust model, the update state machine, package-manager
  ownership, CI and release recipes, architecture, troubleshooting,
  compatibility (including `[patch]` guidance for GPUI), and migration, indexed
  in `docs/README.md`. The README's Rust snippets are compiled as doctests of
  the facade crate.
- End-to-end updates of the reference application on every platform: through
  Sparkle on macOS (delta and full-archive fallback, with a notarization job
  gated on credentials), Inno Setup and portable on Windows, and a managed
  install with rollback of a broken release on Linux.
- CI builds the documentation with warnings denied on macOS, Windows, and Linux,
  and the facade crate declares its docs.rs targets.
- Cargo workspace with the core, macOS, Windows, and Linux backend, GPUI
  facade, neutral UI, CLI, and reference application crates.
- CI on macOS, Windows, and Linux with rustfmt, clippy, tests, docs, MSRV, and
  cargo-deny checks.
- Dual MIT OR Apache-2.0 licensing, contribution guide, security policy, code
  of conduct, and issue and pull request templates.
- Core: signed native feed for Windows and Linux (Sparkle-compatible fields
  and Ed25519 keys and signatures), fail-closed feed validation, bounded HTTP
  fetching with checked redirects, and order-independent release selection.
  See `docs/feed-format.md`.
- Core: automatic-update preference persisted atomically to a JSON file on
  Windows and Linux (corrupt files fall back to defaults), and a configurable
  automatic-check policy: a launch check that respects a minimum interval,
  optional periodic checks, discovery when automatic checks are enabled, and
  backend-owned preferences for Sparkle.
- Core: verified artifact download and staging (`download` module): streamed
  download with byte progress, declared-length and size-limit enforcement,
  Ed25519 verification before the file gets its final name, and fresh private
  staging directories whose paths never come from the feed. `FeedCheckSource`
  connects the native feed checker to the update coordinator.
- `gpui-auto-update sparkle` commands: fetch a checksum-pinned official
  Sparkle distribution (2.10.0 by default), embed `Sparkle.framework` with its
  license notice and the XPC services for the declared sandbox mode, sign
  nested code in order with the hardened runtime (ad-hoc or Developer ID), and
  validate Info.plist metadata, run paths, sandbox requirements, and
  signatures.
- Scheduled workflow that opens an issue when a newer stable Sparkle release
  is available.
- CLI: `gpui-auto-update keys` generates, imports, inspects, and checks
  Sparkle-compatible Ed25519 signing keys (32-byte seed and legacy 96-byte
  formats), integrates with Sparkle's Keychain workflow on macOS, accepts
  private keys only through standard input, environment variables, or files,
  and fails when a key pair does not match. Core marks a published insecure
  test key that release tooling refuses. See `docs/key-management.md`,
  which includes the key rotation procedure.
- GPUI facade: the `Updater` entity with standard actions (`CheckForUpdates`,
  `InstallUpdate`, `RestartToUpdate`, `DismissUpdate`), background execution
  of all checks, preference saves, and backend calls, an `UpdateBackend`
  install/handoff contract with GPUI restart, helper-owned quit, and
  backend-owned relaunch, asynchronous prepare-to-install hooks, an install
  busy guard, a debug-build install guard, deterministic preview states, and
  per-platform default preference file locations.
- macOS backend on Sparkle 2 (`SPUUpdater` through `sparkle-updater`, behind
  the opt-in `sparkle` feature): manual checks use Sparkle's standard UI,
  background checks use Sparkle's background check, the automatic-update
  preference stays in Sparkle and is mirrored, Sparkle errors become
  structured update errors, and Sparkle-driven downloads, installs, and
  relaunches are reflected in the update state. The facade selects it with
  `UpdaterConfig::sparkle` and hands off with `Handoff::BackendOwned`.
  `UpdateBackend::attach` lets backends report state changes made by their
  native engine. See `docs/adr/0002-sparkle-binding.md`.
- macOS backend: Sparkle's scheduled discoveries are adopted into the
  update state without opening a Sparkle window. Gentle reminders route
  through a `PresentationPolicy`; the default `GpuiPresentation` leaves
  scheduled updates to the app, and apps can override it through
  `SparkleBackend::start_with_policy`. Sparkle's install-and-relaunch waits
  for the facade's `on_prepare_to_install` hooks and then resumes through
  `SparkleEvent::RelaunchRequested` and its one-shot `RelaunchContinuation`.
  Mocked tests cover the routing and the coordination, and real-framework
  tests (including a bundled app served a loopback appcast) verify the
  behavior against Sparkle 2.10.0.
- Windows backend: per-user updates from declared per-architecture feeds,
  verified staging with the release version confirmed from the artifact's PE
  version resource, Inno Setup silent handoff with a configurable switch set
  and `/DIR=` pinned to the running install, portable single-executable
  in-place replacement, an `InstallerStrategy` extension point for MSI and
  other installers, structured errors when an installer cannot start, and
  `UpdaterConfig::windows` in the facade. See `docs/windows-installers.md`.
- Linux update helper (a mode of the application executable, see
  `docs/adr/0003-linux-update-helper.md`): acknowledges the current and
  staged layouts before the application quits, swaps the managed prefix only
  after the application has exited, relaunches, waits for the startup health
  signal, rolls back and relaunches the previous version when the new one
  exits first, and records failures for the next start. `LinuxUpdater`
  stages a checked release and hands it to the helper.
- GPUI facade: `run_update_helper_if_requested`, the
  `Updater::main_window_opened` health signal, previous-update failures
  surfaced through `Updater::previous_update_failure` and
  `UpdaterEvent::PreviousUpdateFailed`, and `UpdaterConfig::native_feed`,
  which selects the Linux backend by default on Linux.
- Release automation with release-plz, gated on a fresh-consumer build of the
  packaged crates on macOS, Windows, and Linux. Published crates include the
  license texts and document platform backends on their own targets on
  docs.rs.
- CLI: `gpui-auto-update feed native` signs Windows and Linux artifacts into
  one native feed per OS and architecture, with immutable versioned https
  artifact URLs and validation by the updater's own parser; `feed sparkle`
  runs Sparkle's `generate_appcast` (including deltas) from a pinned
  distribution. Both refuse unsigned entries and keys that do not match the
  app's public key, and support explicit key-rotation bridge releases. See
  `docs/feed-generation.md`.
