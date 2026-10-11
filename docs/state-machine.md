# Update state machine

Every backend reports into one state, `UpdateState`, defined in the core crate
and exposed by the facade through `Updater::state()`. Applications render from
it and never need to know which backend is running. The enum is
`#[non_exhaustive]`, so matches need a wildcard arm.

## States

| State | Meaning |
| --- | --- |
| `Disabled { capability }` | This installation may not update itself (externally managed, unsupported, or temporarily unable). Checks and installs are refused with the capability's error. See [package-manager ownership](package-managers.md). |
| `Idle` | Nothing has happened yet, or an outcome was dismissed. |
| `Checking` | A check is running. |
| `UpToDate` | The last check found no newer release. |
| `Available(update)` | The last check found a newer release. |
| `Downloading { update, progress }` | The artifact is downloading. `progress.total` may be unknown. |
| `Verifying(update)` | Length and Ed25519 signature are being verified. |
| `Staged(update)` | A verified release is ready to install. |
| `Installing(update)` | Installation is running. |
| `WaitingForQuit(update)` | Installation continues once the application quits. |
| `Relaunching(update)` | The application is relaunching into the new version. |
| `RolledBack { update, error }` | The new version failed and the previous one was restored. |
| `Completed(update)` | The update finished installing. |
| `Failed(error)` | The last operation failed. |

`is_busy()` is true for `Checking`, `Downloading`, `Verifying`, `Installing`,
`WaitingForQuit`, and `Relaunching`. A busy state rejects other operations with
`ErrorKind::OperationInProgress`.

## Transitions

```text
Idle ──check──▶ Checking ──▶ UpToDate
                   │
                   ├──▶ Available ──install──▶ Downloading ──▶ Verifying ──▶ Staged
                   └──▶ Failed                                                 │
                                                          restart_to_update    ▼
 Completed ◀── Relaunching ◀── WaitingForQuit ◀──────────────────────── Installing
     ▲              │                 │                                      │
     └──────────────┴─────────────────┴──────────────────────────────────────┘
        (each of the last three can also end in RolledBack or Failed)
```

The exact rules are `UpdateState::apply`:

- `Available` goes to `Downloading` when the download starts.
- `Downloading` stays in `Downloading` as progress arrives, then goes to
  `Verifying`, then `Staged`. Backends that stage without a separate
  verification step go from `Downloading` straight to `Staged`.
- `Staged` goes to `Installing` or directly to `WaitingForQuit`.
- `Installing` goes to `WaitingForQuit` or `Relaunching`.
- `Installing`, `WaitingForQuit`, and `Relaunching` can each end in
  `Completed` or `RolledBack`.
- Any in-flight state, and `Available`, can end in `Failed`.
- `UpToDate`, `Available`, `Failed`, `RolledBack`, and `Completed` return to
  `Idle` when dismissed.
- A check may start from `Idle`, `UpToDate`, `Available`, `Failed`,
  `RolledBack`, or `Completed`. From `Disabled` it is refused with the
  capability's error; from any busy state it is refused with
  `OperationInProgress`.
- Any other event is rejected with `ErrorKind::InvalidState`
  (`OperationInProgress` when the current state is busy).

## Checks: manual and background

A check is **manual** (the user asked, so it always produces visible feedback)
or **background** (automatic, silent unless an update is found). A manual check
that starts while a background check runs attaches to it and receives its
result. Every check produces an `UpdaterEvent::CheckFinished`; a failed
background check is logged and does not move the state to `Failed`.

## Facade operations

| Action / method | Allowed from | Effect |
| --- | --- | --- |
| `CheckForUpdates` / `check_for_updates` | see above | Starts a manual check. |
| `InstallUpdate` / `request_install` | `Available`, `Staged` | From `Available`: download, verify, stage. From `Staged`: same as `restart_to_update`. |
| `RestartToUpdate` / `restart_to_update` | `Staged`, `WaitingForQuit`, `Completed` | Runs the prepare-to-install hooks, then installs and ends the application as the backend requires. |
| `DismissUpdate` / `dismiss` | outcomes listed above | Returns to `Idle`. |

Installs are also rejected while previewing, and in debug builds unless
`UpdaterConfig::allow_debug_self_update(true)` was set.

Staging does not interrupt the user: a release is downloaded and verified in
the background, and nothing quits until the user (or the application) calls
`restart_to_update`.

## Handoff

After the prepare-to-install hooks succeed, the backend decides how the
application ends (`Handoff`, emitted as `UpdaterEvent::Handoff`):

| Handoff | Used by | Meaning |
| --- | --- | --- |
| `Restart { restart_path }` | Windows portable | The facade restarts the application through GPUI, launching `restart_path` instead when set. |
| `Quit` | Windows installers, Linux | An installer or the Linux update helper owns relaunching; the application quits without restarting. |
| `BackendOwned` | macOS (Sparkle) | Sparkle terminates and relaunches the application itself. |

A failing prepare-to-install hook cancels the install: the update stays
`Staged` and `UpdaterEvent::Failed` reports `ErrorKind::QuitCoordination`.

## What each backend reports

- **macOS.** Sparkle drives the lifecycle. The backend mirrors Sparkle's
  downloads, installs, relaunches, and the user's choices in Sparkle's windows
  into this state. Manual checks use Sparkle's standard UI; scheduled
  discoveries are adopted into the state without opening a Sparkle window.
  Byte-level progress is not available from Sparkle's standard user driver
  ([ADR 0002](adr/0002-sparkle-binding.md), gap 1).
- **Windows.** `Available`, `Downloading`, `Verifying`, `Staged`, then
  `Installing` (handoff to the installer, or the portable swap). An installer
  that fails to start within its grace period is reported as
  `ErrorKind::HelperLaunch`, and the update stays staged.
- **Linux.** The same up to `Staged`; installation hands off to the update
  helper. After the next start, `Updater::previous_update_failure()` and
  `UpdaterEvent::PreviousUpdateFailed` report a rollback or failed
  installation once ([details](linux-managed-install.md#diagnostics)).

## Preview states

`PreviewState` (`UpdateAvailable`, `Downloading`, `ReadyToInstall`, `Error`,
`ExternallyManaged`, `RestartRequired`) lets you build and test UI without an
update. A preview update has version `0.0.0-preview`, channel `preview`, and
messages beginning with `Preview:`. While previewing, nothing is checked,
downloaded, or installed. `UpdaterConfig::preview` starts in a preview, and
`Updater::enter_preview` / `exit_preview` switch at runtime.

## Errors

Failures carry an `ErrorKind` that code can match on, a message that is safe
to show to users, and a separate diagnostic (paths, URLs, underlying errors)
that reaches logs through `UpdateError::diagnostic` and `Debug` but never
`Display`. See the `ErrorKind` documentation for the kinds and
[troubleshooting](troubleshooting.md) for what to do about them.
