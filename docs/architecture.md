# Architecture

This page explains how the pieces fit together. The decisions behind them are
recorded in the [ADRs](adr/), and the vocabulary is defined in
[CONTEXT.md](../CONTEXT.md). Read [CONTRIBUTING.md](../CONTRIBUTING.md) to
build and test.

## Layers

```text
 your GPUI application
        │  uses
        ▼
 gpui-auto-update         facade: observable `Updater` entity, actions, executors,
        │                 backend selection        (depends on GPUI)
        │  selects, per target
        ├────────────────────────┬──────────────────────────┐
        ▼                        ▼                          ▼
 gpui-auto-update-macos   gpui-auto-update-windows   gpui-auto-update-linux
 Sparkle 2 backend        installer / portable       managed install, helper,
 (optional `sparkle`      handoff                    rollback
  feature)
        │                        │                          │
        └────────────────────────┴──────────┬───────────────┘
                                            ▼
                                   gpui-auto-update-core
          update state, errors, capability, feed, verification, policy
                          (no GPUI, no unsafe code)

 gpui-auto-update-ui    optional neutral controls (depends on GPUI and the facade)
 gpui-auto-update-cli   release tooling: init, doctor, keys, sparkle, feed, verify
                        (not needed at runtime; depends on the core only)
```

| Crate | Role | Depends on GPUI |
| --- | --- | --- |
| `gpui-auto-update-core` | The contracts: `UpdateState`, `UpdateError`, `Capability`, the native feed parser, bounded HTTP fetching, Ed25519 verification, artifact staging, the automatic-check policy, preferences. | No |
| `gpui-auto-update-macos` | Sparkle 2 backend. All update mechanics are Sparkle's. | No |
| `gpui-auto-update-windows` | Verified installer and portable handoff. | No |
| `gpui-auto-update-linux` | Managed-install detection, hardened staging, helper, health confirmation, rollback. | No |
| `gpui-auto-update` | The crate applications depend on. Re-exports the core as `gpui_auto_update::core`, the macOS and Windows backends as `macos` and `windows` on their platforms, and provides the Linux adapter as `linux`. | Yes |
| `gpui-auto-update-ui` | Theme-inheriting controls: `UpdateControls`, `UpdateIndicator`, and the plain-data `UpdateSummary`. | Yes |
| `gpui-auto-update-cli` | The `gpui-auto-update` binary. | No |

The rules that keep this shape, and the tests that enforce them
(`tools/workspace-policy`), are in
[ADR 0001](adr/0001-workspace-layout.md): GPUI is confined to the facade, the
UI crate, and the reference app; backends are separate crates that compile on
every target; `unsafe` is denied workspace-wide and allowed only locally in
platform backend modules; the non-GPUI crates support Rust 1.85 while the GPUI
crates follow GPUI's own requirement.

## Threads and the foreground

Rule: the GPUI foreground thread never performs blocking update network or
filesystem work.

- `Updater` is a GPUI entity. Its methods run on the foreground and return
  immediately.
- Checks, downloads, verification, staging, preference I/O, and the backend's
  `install` run on GPUI's background executor.
- Results travel back over a channel and are applied on the foreground in
  order, after which the updater emits an `UpdaterEvent` and notifies
  observers. Update UI therefore follows ordinary GPUI patterns: `cx.observe`
  the entity and read `state()`.
- The prepare-to-install hooks run on the foreground, because they usually
  touch application entities, and may return an asynchronous task.
- On macOS, Sparkle lives on the main thread. Its events are mirrored into the
  state on a dedicated thread so Sparkle's main thread never waits on the
  coordinator or on GPUI observers.

## One state, many backends

The core's `UpdateCoordinator` owns the single `UpdateState`. Backends report
into it through `ProgressSink` events; the facade mirrors it. The macOS backend
translates Sparkle's delegate callbacks into the same events, so an application
renders one state model on every platform. See the
[state machine](state-machine.md).

The facade's `UpdateBackend` trait is the contract between the facade and a
platform backend: report a `Capability`, `stage` a release, `install` it and
return a `Handoff` (`Restart`, `Quit`, or `BackendOwned`), `relaunch`, and
optionally confirm startup and switch channels. `UnsupportedBackend` is the
default, so nothing is installed until a backend is supplied.

## The macOS backend

`gpui-auto-update-macos` delegates to Sparkle 2 through the
[`sparkle-updater`](https://crates.io/crates/sparkle-updater) crate and
talks to it through its own `SparkleEngine` trait, so lifecycle logic is
portable and tested with scripted engines. The real engine is behind the
opt-in `sparkle` feature, because the binding links `Sparkle.framework`. The
gaps between that binding and what the facade would like, and the plan for
them, are in [ADR 0002](adr/0002-sparkle-binding.md).

## The Linux helper

Replacing the install prefix cannot happen while the application runs, so the
update is finished by the application's own executable started in a helper
mode, as decided in [ADR 0003](adr/0003-linux-update-helper.md). The helper
swaps the staged prefix into place, launches the new version, and keeps the
previous version until the new one confirms startup. This is why applications
call `run_update_helper_if_requested()` first in `main` and
`Updater::main_window_opened` once the main window is open.

## Testing

The preferred test seam is a signed update source plus an old installed
application: check, discover, stage, verify, install or hand off, quit,
relaunch, confirm or roll back, observed through public updater state and
externally visible file and process behavior.

- Unit and integration tests for each crate run on every OS in CI. Portable
  logic is kept in modules that can run on any host; platform code is gated by
  `cfg`.
- The [reference application](../apps/reference-app/README.md) performs real
  old-to-new updates: Linux (including rollback of a broken release) and
  Windows (Inno Setup and portable) run in CI through `tools/e2e/`.
- GPUI-facing behavior is tested with GPUI's `TestAppContext`.
- Preview states let UI be tested without any update.

## Release tooling

The CLI shares the core's feed parser and verification code, so `doctor` and
`verify` judge a feed with the same rules the updater applies at runtime. It
wraps Sparkle's own `generate_appcast`, `sign_update`, and `generate_keys`
for macOS rather than reimplementing them. See [doctor](doctor.md),
[feed generation](feed-generation.md), and [verification](verification.md).
