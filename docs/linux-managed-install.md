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

Later tickets add further required files (for example the update helper) to
this layout; the marker version will change if the layout changes
incompatibly.

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

## Recommended install location

Install into a dedicated directory under the user's home, for example
`~/.local/opt/<app>`, and optionally link `~/.local/bin/<app>` to
`~/.local/opt/<app>/bin/<app>`. Do not install the prefix directly into a
shared directory such as `~/.local` itself, because the whole prefix is
replaced on update.
