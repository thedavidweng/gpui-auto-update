# ADR 0002: The Linux update helper is a mode of the application executable

- Status: accepted
- Date: 2026-10-06

## Context

On Linux, a staged release replaces the whole managed prefix. The running
application cannot do that itself: the prefix must not change until the
application has finished its normal quit and save path, and after the swap
someone must relaunch the new version, wait for it to prove that it started,
and restore the previous version if it did not. The specification calls the
process that does this the *helper*.

ADR 0001 (rule 7) left open where the helper binary lives. The options were:

1. **A separate binary** (for example `bin/<app>-updater`) shipped in every
   release, either built from a binary target of `gpui-auto-update-linux` or
   by the application.
2. **A helper mode of the application's own executable**, entered through a
   private first argument before the GUI starts.

## Decision

The helper is a mode of the application executable (option 2).

- Applications call `gpui_auto_update::run_update_helper_if_requested()` (or
  `gpui_auto_update_linux::run_helper_if_requested()`) first thing in `main`.
  It returns immediately unless the first argument is
  `--gpui-auto-update-helper`; in helper mode it runs the helper and exits
  the process without creating the GUI. It is a no-op on other platforms.
- The backend starts the *running* executable (`std::env::current_exe()`) in
  helper mode. The helper therefore runs the code of the version that is
  installed and verified now, never code from the staged download.

The handoff protocol between the application (the *host*) and the helper:

1. The host starts the helper with the application name, the canonical
   install prefix, the staged prefix, its own pid, the health timeout, and
   the version being installed. The helper gets a piped stdin and stdout,
   runs in its own process group, and inherits stderr for logs.
2. The helper checks that it is not root (on Linux), that its parent is the
   host pid, that the install path is canonical, that the staged prefix sits
   in a `.<prefix>.gpui-auto-update-staged-*` directory next to the install,
   that both layouts pass `validate_layout`, and (on Linux) that the files
   are owned by the current user. It answers one line on stdout: `READY`, or
   `ERROR<TAB><reason>`.
3. Only after `READY` does the backend return `Handoff::Quit`, so the GUI
   begins its normal termination only after both layouts were accepted. The
   host deliberately leaks its end of the helper's stdin, so the pipe closes
   exactly when the host process exits.
4. The helper waits for end-of-file on stdin and then for its parent pid to
   change (the host is gone). If the parent is still the host 30 s after the
   pipe closed, the host gave up on the handoff and the helper exits without
   changing anything.
5. It revalidates both layouts, renames the install to a reserved
   `.<prefix>.gpui-auto-update-backup-*` sibling, and renames the staged
   prefix into place (restoring the backup if that second rename fails).
6. It launches `<prefix>/bin/<app>` with `GPUI_AUTO_UPDATE_HEALTH_FILE` set to
   an unused `.<prefix>.gpui-auto-update-health-*` sibling path. Stdout of the
   relaunched application is `/dev/null`, because the helper's stdout is the
   pipe to the host that has quit.
7. The new version calls `Updater::main_window_opened` once its main window
   opens; the Linux backend creates the health file (exclusively, mode 0600,
   only at a path of that form next to its own prefix). The helper polls
   every 25 ms:
   - health file present: delete the backup; done.
   - the new process exited first: move the new prefix aside, restore the
     backup, record a `rolled-back` diagnostic, relaunch the previous
     version.
   - timeout (60 s by default) while the process still runs: keep it running
     (never pull an install out from under a live process), keep the backup
     for manual recovery, and record an `unconfirmed` diagnostic.
8. Any failure after the host quit is recorded atomically in
   `.<prefix>.gpui-auto-update-diagnostic` (at most 16 KiB) before the
   previous version is relaunched. The next start reads and deletes it, and
   the facade reports it through `Updater::previous_update_failure` and
   `UpdaterEvent::PreviousUpdateFailed`.

The protocol, swap, and rollback code is Unix-portable (`cfg(unix)`); only
the root and ownership checks are Linux-only. Its tests run on macOS and
Linux with real child processes: the integration test binary is its own
helper and, through hard links placed in temporary managed installs, its own
fake application.

## Consequences

- Releases need no extra executable, and the managed-install layout and
  marker version stay as documented by T19 and T20.
- The helper and the application can never disagree about the protocol,
  because they are the same binary.
- Every Linux application must call `run_update_helper_if_requested` before
  creating the GUI. If it does not, the handoff starts a second instance of
  the application instead of a helper; that instance never acknowledges, so
  after the acknowledgement timeout (30 s) the backend stops it and reports a
  `HelperLaunch` error. Nothing is changed, but the mistake is visible.
- Every application must call `Updater::main_window_opened`; otherwise every
  update is reported as unconfirmed and the backup is kept.
- The helper that installs a release is the previous release's code. A
  release whose layout would fail the previous version's `validate_layout`
  cannot be installed by it, so incompatible layout changes need a new
  marker version and a migration plan.
