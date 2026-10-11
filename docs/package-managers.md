# Package-manager ownership

The updater never modifies an installation it cannot prove it owns. If a
package manager installed the application, the package manager updates it, and
the updater stays out of the way while still telling the user why.

The result of that decision is a `Capability`, reported by
`Updater::capability()`:

| Capability | Meaning |
| --- | --- |
| `SelfManaged` | This updater owns the installation and may update it. |
| `ExternallyManaged { manager }` | Something else updates it. `manager` is a presentable name such as `"Homebrew"`, when known. |
| `Unsupported` | This installation cannot be updated by the library (for example an unmarked Linux install). |
| `TemporarilyUnavailable` | Normally self-managed, but cannot update now (for example a read-only location). |

Anything other than `SelfManaged` puts the state in `UpdateState::Disabled
{ capability }`. Checks and installs are then refused with an `UpdateError`
whose `ErrorKind` is `ExternallyManaged`, `UnsupportedInstallation`, or
`TemporarilyUnavailable`, and whose message can be shown to the user ("This
installation is updated by Homebrew."). The updater does not disappear: a
"Check for Updates" menu item still gives visible feedback, and the neutral
controls in `gpui-auto-update-ui` render the explanation.

## What is detected automatically

| Platform | Detected | Not detected |
| --- | --- | --- |
| Linux | Flatpak (`/.flatpak-info`, `…/flatpak/app/…`), Nix (`/nix/store`), Guix (`/gnu/store`), Snap (`/snap`), Linuxbrew; system locations (`/usr` including `/usr/local`, `/opt`, `/app`, `/bin`, `/sbin`, `/lib`, `/lib64`) as `ExternallyManaged { manager: None }`. Any other installation without the managed-install marker is `Unsupported`. | Other package managers that install under the user's home are not named, but the missing marker still makes them `Unsupported`. |
| Windows | An install directory the current user cannot write to (for example under Program Files) is `Unsupported`, because updates never request elevation. | Scoop, winget, and Chocolatey are **not** recognized. A per-user package-manager install that the user can write to looks self-managed. |
| macOS | Nothing. The Sparkle backend reports `SelfManaged` whenever Sparkle is running. Outside an application bundle the installation is `Unsupported`. | Homebrew casks are **not** recognized. |

The Linux rules, in order, are in the
[managed-install contract](linux-managed-install.md#ownership-requirements).
Package-manager and system-location checks run before the marker is read, so a
marker never turns a package-managed copy into a self-managed one.

## Declaring ownership yourself

Where the library cannot see how the application was installed, the
application can. If you publish through Homebrew, winget, or Scoop, build the
packaged variant with a capability override:

```rust,ignore
use gpui_auto_update::core::Capability;

// `config` is whatever you would normally pass to `gpui_auto_update::init`.
let config = config.with_capability(Capability::ExternallyManaged {
    manager: Some("Homebrew".to_owned()),
});
```

`with_capability` replaces the capability the backend would report. Typical
sources for the decision are a build-time flag in the package manager's build
recipe, or an environment variable the package's launcher sets. The reference
application demonstrates the build-time form
(`REFERENCE_APP_EXTERNALLY_MANAGED`, see its
[README](../apps/reference-app/README.md)).

Overriding is one-way: it can only disable self-update. Do not use it to force
`SelfManaged` on an installation the backend refused.

## Guidance for packagers

- Ship the same binary and mark it externally managed through the application's
  configuration, rather than patching out the updater.
- On Linux, do not add the managed-install marker to a distribution package.
  The marker is the statement "this updater owns this directory".
