# ADR 0002: Sparkle binding for the macOS backend

- Status: accepted
- Date: 2026-10-05

## Context

The macOS backend delegates every update mechanic to Sparkle 2 (spec, *macOS
backend*). The spec asks us to reuse the maintained, modern Sparkle Rust
binding where its API is sufficient, and to contribute missing capabilities
upstream before maintaining our own compatibility layer (spec, *Modern Sparkle
Rust foundation* and *Relationship to prior art*). The old
`hankbao/sparkle-updater` project is prior art only and is not a dependency.

The maintained binding is the `sparkle-updater` crate (0.1.0, MIT), which the
Tauri Sparkle plugin (`ahonn/tauri-plugin-sparkle-updater`) extracted as a
standalone crate. It wraps `SPUStandardUpdaterController` and `SPUUpdater`
with objc2, exposes typed delegate events, and is main-thread only
(`SparkleUpdater` is `!Send`/`!Sync`). It was developed against Sparkle
2.9.6. We pin 2.10.0, and because the binding links dynamically against the
embedded framework, it works with either version.

Its build behavior affects the workspace (ADR 0001, rule 6):

- On macOS, its build script panics unless it finds `Sparkle.framework`
  (through `SPARKLE_FRAMEWORK_PATH` or an ancestor of the build directories).
- It links the framework normally rather than lazily or with `dlopen`, so
  every binary and test executable that depends on it needs the framework
  at load time.
- It adds no run path. The host executable needs
  `-Wl,-rpath,@executable_path/../Frameworks`, and tests need
  `DYLD_FRAMEWORK_PATH`.

## Gap analysis

The table compares the spec's macOS backend requirements with what
`sparkle-updater` 0.1.0 offers.

| Spec requirement | `sparkle-updater` 0.1.0 | Status |
| --- | --- | --- |
| Modern `SPUUpdater`-generation API, no `SUUpdater` | `SPUStandardUpdaterController` + `SPUUpdater` only | Met |
| Observe update lifecycle events | Typed `UpdateEvent` for all informational delegate callbacks | Met |
| Route manual checks through Sparkle's standard UI | `check_for_updates()` (standard controller) | Met (T9) |
| Automatic-update preference stored by Sparkle | `automatically_checks_for_updates` / setter, interval, auto-download, `last_update_check_date` | Met (T9) |
| Release notes | Inline description, its format, release-notes and full-release-notes links | Met |
| Channels | `allowedChannels` through the delegate (not persisted by Sparkle) | Met; the app sets the channel on every launch |
| Surface useful errors | `ErrorPayload` with domain, code, and localized text; typed no-update reasons | Met |
| Keep scheduled discovery in GPUI (no Sparkle window) | Gentle-reminder hooks: return `false` from `should_show_scheduled_update` | Met with gentle reminders (T10) |
| Retain or reproduce an available-update state | `last_found_update()`, and `check_for_updates()` brings an existing session forward | Mostly met: no handle to the pending user-driver reply |
| Coordinate installation and relaunch with app saving | `RelaunchHandler` + one-shot `RelaunchContinuation::resume` | Met for relaunch (T10). Not a universal quit veto |
| Fully custom GPUI presentation (Sparkle as engine only) | No custom `SPUUserDriver`; the standard user driver is always used | **Gap 1** |
| Byte-level download and extraction progress | Only available through `SPUUserDriver` callbacks | **Gap 1** |
| Sparkle's update permission flow | `updaterShouldPromptForPermissionToCheckForUpdates` is hard-coded to `YES` | **Gap 2** (workaround: `SUEnableAutomaticChecks` in Info.plist) |
| "Restart to update now" for a silently downloaded update | `willInstallUpdateOnQuit:immediateInstallationBlock:` returns `NO`; the block is not exposed | **Gap 3** |
| Per-check decisions (may check, should proceed) | Static booleans, not callbacks | **Gap 4** (not needed by the spec today) |
| Skip and dismiss semantics | Only through Sparkle's own UI. `userDidMakeChoice` is observable | Partial: observed and mirrored, not initiated from GPUI |
| Machine build version of a found update | `UpdateInfo` has the display version only (no `versionString`) | **Gap 5** (`AvailableUpdate::build` stays `None`) |
| Build without the framework | Build script panics, non-lazy link, no run path | **Gap 6** |

## Decision

1. **Use `sparkle-updater` 0.1.0 as the Sparkle binding.** We do not write a
   second set of Objective-C bindings. Our crate contains no `unsafe` code of
   its own: the binding owns the interoperability, and calls from other
   threads reach the main thread through `dispatch2`'s safe `MainThreadBound`
   and main-queue APIs.
2. **The binding is an optional dependency behind the `sparkle` feature** of
   `gpui-auto-update-macos`. The `gpui-auto-update` facade forwards it as its
   own `sparkle` feature. Without the feature, both crates are pure Rust on
   every target, and default workspace builds and tests never need the
   framework (gap 6).
3. **The backend talks to Sparkle through our `SparkleEngine` trait** and
   receives Sparkle's notifications as our own `SparkleEvent` type through
   `SparkleEvents`. The real engine (`SparkleBackend::start`, feature-gated)
   is a thin adapter over `sparkle-updater`. All lifecycle logic (check
   routing, outcome and error mapping, preference mirroring, and state
   tracking) is portable and tested with scripted engines. When gap 1 is
   closed, a custom user driver becomes a second engine and the rest of the
   backend stays the same.
4. **Default UX.** A manual check calls the standard controller's
   `checkForUpdates:`, so Sparkle's standard presentation is used. A
   background check calls `checkForUpdatesInBackground`. Sparkle's scheduler
   runs periodic checks, so the facade's Sparkle configuration
   (`UpdaterConfig::for_sparkle`/`UpdaterConfig::sparkle`) disables the
   facade's launch, periodic, and on-enable checks. Suppressing Sparkle's
   window for scheduled discoveries (gentle reminders) and postponing
   relaunch until the save hooks finish (relaunch continuation) are added by
   T10 on the same structure.
5. **Preferences.** `SparkleBackend::preferences` is a `PreferenceStore` with
   `PreferenceOwner::Backend`. Every load reads Sparkle's stored values. A
   save writes only the automatic-check flag, and only when it changed.
   Sparkle stays the single source of truth, and the facade mirrors it.
6. **Install and relaunch.** Sparkle installs and relaunches the
   application, so every handoff is `Handoff::BackendOwned`. Staging and
   installing bring Sparkle's prompt forward and mirror its progress. The
   state follows Sparkle's lifecycle events through the tracker that
   `UpdateBackend::attach` starts.

## Upstream contribution plan

Following the spec's policy, we contribute to upstream first, adopt the
released capability next, and keep a local layer only when upstream reuse is
impractical. We plan to propose these changes to
`ahonn/tauri-plugin-sparkle-updater` in this order:

1. **Lazy or optional linking (gap 6).** Offer a build mode that does not
   panic when the framework is missing (for example, emit a warning and skip
   linking under a `check-only` feature or when `SPARKLE_SKIP_LINK` is set),
   and document or emit the `@executable_path/../Frameworks` run path. This
   would let downstream crates enable the binding by default.
2. **Permission-prompt control (gap 2).** Make
   `updaterShouldPromptForPermissionToCheckForUpdates` configurable, ideally
   with a hook that lets the host present its own permission UI and answer
   through `SPUUpdatePermissionResponse`.
3. **Immediate installation block (gap 3).** Expose the
   `willInstallUpdateOnQuit:immediateInstallationBlock:` block as a one-shot,
   main-thread continuation, like `RelaunchContinuation`, so a GPUI
   "Restart to update" button can install a silently downloaded update.
4. **Build version in `UpdateInfo` (gap 5).** Add `SUAppcastItem.versionString`
   next to the display version.
5. **Custom user driver (gap 1).** This is the largest change: a Rust
   `SPUUserDriver` trait with the reply blocks wrapped as one-shot
   continuations, plus a constructor that uses `SPUUpdater` with that driver
   instead of the standard controller. It enables byte-level progress, fully
   custom GPUI presentation, and reuse of a silent check's pending reply.
6. **Per-check callbacks (gap 4).** Turn `mayPerformUpdateCheck` and
   `shouldProceedWithUpdate` into callbacks. This is low priority.

Until each change is released upstream, we accept the gap. We do not fork
the binding. If a gap blocks a ticket and upstream cannot take the change
in reasonable time, the smallest possible objc2 module may be added inside
`gpui-auto-update-macos` (gated by the same feature, with `unsafe` confined
to it). That would require a follow-up ADR that names the upstream issue it
replaces.

## Consequences

- Applications opt in with `gpui-auto-update = { features = ["sparkle"] }`.
  Their macOS builds set `SPARKLE_FRAMEWORK_PATH` (`gpui-auto-update sparkle
  fetch` provides a verified distribution), and their bundles embed and sign
  the framework as `docs/sparkle-packaging.md` describes.
- Tests that need the real framework are opt-in (`--features sparkle`, with
  `SPARKLE_FRAMEWORK_PATH` and `DYLD_FRAMEWORK_PATH`) and marked `#[ignore]`
  or built only with that feature.
- Our `SparkleEvent`/`SparkleEngine` surface is a small duplicate of the
  binding's types. That is the price of portable tests and of being able to
  switch engines. It only translates types and holds no Sparkle logic.
- If the binding is abandoned, the `SparkleEngine` seam keeps the cost of
  replacing it limited to one module.
