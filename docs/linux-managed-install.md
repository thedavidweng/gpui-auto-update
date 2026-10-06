# Linux managed-install contract

On Linux, `gpui-auto-update` updates only installations it can prove it owns:
user-local tarball installs that carry an explicit ownership marker. Every
other installation (source builds, distribution packages, Flatpak, Snap, Nix,
Homebrew, system-wide copies, root sessions) reports a capability other than
`Capability::SelfManaged`. The updater stays visible and explains why it is
disabled instead of disappearing.

Detection is implemented by `gpui_auto_update_linux::detect` (portable and
testable with injected inputs) and `gpui_auto_update_linux::detect_current`
(Linux only, gathers inputs from the running process).

## Layout

A managed install is a directory, the *prefix*, laid out as:

```text
<prefix>/
├── bin/
│   └── <app>                              # the application executable
└── share/
    └── <app>/
        └── gpui-auto-update.managed       # the ownership marker
```

- `<app>` is the application name configured in the updater. It must be 1 to
  64 ASCII characters from `A-Z a-z 0-9 . _ -` and must not start with `.` or
  `-`.
- The running executable, after resolving symlinks, must be exactly
  `<prefix>/bin/<app>`. Launching through a symlink such as
  `~/.local/bin/<app>` is fine because detection uses the resolved path.
- `bin/<app>` must be a regular file with an execute bit set.
- The whole prefix is replaced as a unit during an update, so it must contain
  only files that belong to the release.

The update helper is a mode of `bin/<app>` itself (see
`docs/adr/0003-linux-update-helper.md`), so no other file is required. The
marker version will change if the layout changes incompatibly.

## Marker

The marker is `<prefix>/share/<app>/gpui-auto-update.managed`. It must be a
regular file (not a symlink, directory, or special file) of at most 4096 bytes
whose contents are exactly, byte for byte:

```text
gpui-auto-update managed-install 1
app=<app>
```

Each line ends with a single `\n` (no `\r`, no trailing blank line, no extra
keys). `<app>` must equal the configured application name. Any other contents
make the install unsupported. The header line carries the contract version; a
future incompatible contract uses a different version number and older
updaters reject it.

`gpui_auto_update_linux::marker_contents(app)` returns the exact bytes, and
`gpui_auto_update_linux::MARKER_FILE_NAME` the file name, so release tooling
can generate the marker rather than hand-writing it. Release archives must
ship the marker inside the archive so that the staged new version carries it
too.

## Ownership requirements

All of the following must hold, otherwise self-update is disabled:

| Check | Capability when it fails | `DetectionReason` |
| --- | --- | --- |
| Build architecture is `x86_64` or `aarch64` | `Unsupported` | `UnsupportedArchitecture` |
| Application name is valid | `Unsupported` | `InvalidAppName` |
| Effective uid is not 0 | `Unsupported` | `RootSession` |
| Not running inside a Flatpak sandbox (`/.flatpak-info`) | `ExternallyManaged { manager: "Flatpak" }` | `PackageManager` |
| Executable path resolves | `Unsupported` | `ExecutableUnresolvable` |
| Executable is not in a package-manager store (`/nix/store`, `/gnu/store`, `/snap`, Linuxbrew, `…/flatpak/app/…`) | `ExternallyManaged { manager: <name> }` | `PackageManager` |
| Executable is not under a system location (`/usr` including `/usr/local`, `/opt`, `/app`, `/bin`, `/sbin`, `/lib`, `/lib64`) | `ExternallyManaged { manager: None }` | `SystemInstall` |
| Executable is `<prefix>/bin/<app>` | `Unsupported` | `UnknownLayout` |
| `$HOME` is set and resolves | `Unsupported` | `HomeUnresolvable` |
| Prefix is strictly inside the resolved home directory (never the home directory itself) | `Unsupported` | `OutsideHome` |
| Marker exists | `Unsupported` | `MissingMarker` |
| Marker is a small regular file with the exact contents | `Unsupported` | `InvalidMarker` |
| `bin/<app>` is a regular executable file | `Unsupported` | `UnknownLayout` |
| Prefix, `bin/`, `bin/<app>`, and the marker are owned by the effective uid | `Unsupported` | `NotUserOwned` |
| A uniquely named sibling directory can be created and removed next to the prefix | `TemporarilyUnavailable` | `ParentNotWritable` |

Checks run in this order and the first failure wins. Package-manager and
system-location checks run before the marker is read, so a marker never turns
a package-managed or system-wide copy into a self-managed one.

The writability probe creates and immediately removes
`<parent>/.<prefix-name>.gpui-auto-update-probe-<pid>-<nonce>-<n>`. It is the
same operation class as the rename-based swap, so it also catches read-only
mounts.

## Release archive

A Linux release artifact is a gzip-compressed tar archive (`.tar.gz`)
listed in the architecture's native feed (see `docs/feed-format.md`). It is
signed like every other artifact; nothing in it is read until the core's
downloader has verified the length and Ed25519 signature and returned a
`StagedArtifact`.

`gpui_auto_update_linux::ReleaseStager::stage` extracts it as follows:

1. The verified file is reopened and must still be a regular file of the
   verified length, otherwise staging fails with `ArtifactChanged`.
2. A private (`0700`) directory
   `<parent>/.<prefix-name>.gpui-auto-update-staged-<version>-<random>` is
   created next to the prefix, on the same filesystem, so the helper can
   swap with a rename. The running prefix is never written.
3. The archive is streamed into that directory with the rules below. Any
   violation stops extraction and removes the staging directory.
4. The extracted prefix must pass `validate_layout` (the layout and marker
   described above, with no symbolic links on those paths). Only then is a
   `StagedRelease` returned, so a broken release is reported before the
   application is asked to quit.

### Archive rules

The archive contains exactly one top-level directory, the *release root*:

```text
<app>-<version>-linux-<arch>/
├── bin/<app>
└── share/<app>/gpui-auto-update.managed
```

- `<version>` is the feed entry's `sparkle:version`, byte for byte (build
  metadata included). Feed metadata is not signed, but the archive is, so
  this name is what proves the archive is the release the feed offered. A
  different valid version fails with `VersionMismatch` (for example an older
  signed release relabeled as newer), and a version part that is not strict
  SemVer fails with `InvalidVersionPath`.
- `<arch>` is `x86_64` or `aarch64` and must match the installation;
  otherwise staging fails with `ArchMismatch`.
- Any other top-level name, or a top-level entry that is not a directory,
  fails with `UnexpectedRoot`.

Every entry must also satisfy:

| Rule | Error |
| --- | --- |
| The path is non-empty UTF-8 without control characters and does not start with `./` | `UnsafePath` |
| No `..` component | `PathTraversal` |
| Not absolute | `AbsolutePath` |
| The entry is a regular file or a directory (no symbolic or hard links) | `Link` |
| No devices, FIFOs, sparse, contiguous, or unknown entry types | `SpecialFile` |
| No two entries name the same path after removing empty and `.` components (`a//b`, `a/./b`, and `a/b/` all equal `a/b`) | `DuplicatePath` |
| At most `max_entries` entries (default 100 000) | `TooManyEntries` |
| The compressed archive is at most `max_compressed_bytes` (default 512 MiB, the core's artifact limit) | `ArchiveTooLarge` |
| The declared entry sizes, and the whole decompressed tar stream including headers and long-name records, are at most `max_expanded_bytes` (default 1 GiB) | `ExpandedTooLarge` |
| The gzip and tar data are well formed | `Malformed` |

Parent directories need not be listed; they are created as needed. Ownership,
timestamps, and extended attributes are not restored. Permissions are
normalized: directories are `0755`, files with any execute bit are `0755`,
and all other files are `0644`, so setuid, setgid, sticky, and group- or
world-writable bits never reach the staged installation.

All `StageError` values map to `ErrorKind::ArchiveValidation`, except
`ArtifactChanged` and filesystem failures (`Io`), which map to
`ErrorKind::Staging`.

A release can be packaged with, for example:

```sh
tar -czf demo-1.5.0-linux-x86_64.tar.gz demo-1.5.0-linux-x86_64
```

Do not create the archive from inside the directory (`tar -czf … .`), which
produces `./` entries and no release root.

## Installing a staged release

The staged release is installed by the *update helper*, a mode of the
application's own executable. The decision and the full protocol are in
`docs/adr/0003-linux-update-helper.md`; this section is the integration
contract.

### What the application must do

```rust
fn main() {
    // Before anything else, in particular before creating the GPUI app.
    gpui_auto_update::run_update_helper_if_requested();
    // ...create the application and the updater, for example with
    // UpdaterConfig::native_feed, then, once the main window is open:
    // updater.update(cx, |updater, cx| updater.main_window_opened(cx));
}
```

- `run_update_helper_if_requested` returns at once unless the process was
  started as the helper.
- `Updater::main_window_opened` is the *health signal*. Call it on every
  start, after the main window has opened. When the process was not started
  by the helper it does nothing.
- On start, show `Updater::previous_update_failure` (also emitted as
  `UpdaterEvent::PreviousUpdateFailed`) if it is set: it reports a rollback
  or a failed installation once.

Without the facade, the same pieces are `gpui_auto_update_linux::
run_helper_if_requested`, `confirm_startup`, and `take_diagnostic`, and
`LinuxUpdater` (stage, hand off) or the lower-level `HelperCommand`.

### Sequence

1. The release is staged and validated next to the install (above).
2. Installing runs the prepare-to-install hooks, then starts the helper. The
   helper checks both the current and the staged layout and acknowledges
   before the application is asked to quit. A refusal is reported as a
   `HelperLaunch` error and nothing changes.
3. The application quits through its normal quit path. The helper waits
   until the process has exited.
4. The helper renames the prefix to a backup sibling, renames the staged
   prefix into its place, and launches `<prefix>/bin/<app>` with
   `GPUI_AUTO_UPDATE_HEALTH_FILE` set.
5. When the new version confirms its start, the backup is deleted. If it
   exits before confirming, the previous version is restored and relaunched.
   If it neither confirms nor exits within 60 seconds, it keeps running, the
   backup is kept for manual recovery, and the update is reported as
   unconfirmed.

### Files next to the install

All of these live in the prefix's parent directory, outside the prefix, so
they survive a swap:

| Name | Purpose |
| --- | --- |
| `.<prefix>.gpui-auto-update-staged-<version>-<random>/` | The staged release (removed after the swap) |
| `.<prefix>.gpui-auto-update-downloads/` | The verified download while it is extracted |
| `.<prefix>.gpui-auto-update-backup-*` | The previous version until the new one confirms its start |
| `.<prefix>.gpui-auto-update-failed-*` | A failed new version while it is rolled back |
| `.<prefix>.gpui-auto-update-health-*` | The health file the new version creates |
| `.<prefix>.gpui-auto-update-diagnostic` | The last helper failure, read and removed by the next start |

### Diagnostics

The diagnostic is a small text file (at most 16 KiB) written atomically. It
records one of these outcomes, each surfaced as an `UpdateError`:

| Outcome | Error kind | Meaning |
| --- | --- | --- |
| `rolled-back` | `HealthConfirmation` (or `Relaunch`) | The new version did not start; the previous one was restored and relaunched |
| `not-installed` | `Replacement` | The new version could not be put in place; the current one was kept and relaunched |
| `unconfirmed` | `HealthConfirmation` | The new version runs but never confirmed its start; the backup was kept |
| `rollback-failed` | `Rollback` | Restoring the previous version failed; the detail names where it is kept |

## Recommended install location

Install into a dedicated directory under the user's home, for example
`~/.local/opt/<app>`, and optionally link `~/.local/bin/<app>` to
`~/.local/opt/<app>/bin/<app>`. Do not install the prefix directly into a
shared directory such as `~/.local` itself, because the whole prefix is
replaced on update.
