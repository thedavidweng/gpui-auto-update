# Reference application

A deliberately small GPUI app that exercises `gpui-auto-update` through its
public facade only. It shows manual and automatic checks, the available
update, download progress, the automatic-update preference, a document that
is saved before the app restarts to update, externally managed installs, and
errors. The preview buttons switch the update UI between the facade's
`PreviewState`s without touching the network or the installation.

## Building version N and N+1

Everything that differs between two builds is read at compile time:

```sh
REFERENCE_APP_VERSION=1.0.0 \
REFERENCE_APP_FEED_URL=https://updates.example.com/feed.xml \
REFERENCE_APP_PUBLIC_KEY=<base64 Ed25519 public key> \
cargo build -p gpui-auto-update-reference-app --release
```

| Variable | Meaning | Default |
| --- | --- | --- |
| `REFERENCE_APP_ID` | Reverse-DNS application identifier | `dev.gpui-auto-update.reference-app` |
| `REFERENCE_APP_VERSION` | Version of this build (strict semver) | `1.0.0` |
| `REFERENCE_APP_FEED_URL` | Native update feed | none; checks fail with a configuration error |
| `REFERENCE_APP_PUBLIC_KEY` | Base64 Ed25519 public key, required with a feed | none |
| `REFERENCE_APP_ALLOW_INSECURE_HTTP` | Allow an `http` feed on a loopback host | `false` |
| `REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE` | Let a debug build install updates | `false` |
| `REFERENCE_APP_EXTERNALLY_MANAGED` | Package manager name; marks the install externally managed | none |
| `REFERENCE_APP_CHECK_INTERVAL_SECS` | Periodic automatic check interval | library default |
| `REFERENCE_APP_WINDOWS_INSTALL` | Windows update strategy: `inno-setup` or `portable` | none; Windows builds do not update themselves |
| `REFERENCE_APP_E2E_REPORT` | Absolute path; run unattended and append what happens to it | none |
| `REFERENCE_APP_E2E_FAIL_TO_START` | Exit with an error before the main window opens, as a broken release | `false` |

`reference-app --version` prints the compiled version and exits, so tests can
tell which build is running.

On Windows the build also embeds a version resource whose `ProductVersion`
is `REFERENCE_APP_VERSION`, which the Windows backend checks before it
installs an update.

## Unattended end-to-end runs

A build with `REFERENCE_APP_E2E_REPORT` checks for updates as soon as the
updater is ready, installs whatever the feed offers without waiting for a
click, and appends one line per fact to the report (`started <version>`,
`update-available <version>`, `handoff`, `up-to-date`,
`previous-update-failure <kind>: <message>`,
`error <kind>: <message> [(<diagnostic>)]`, `failed-to-start <version>`). After
a failed update it reports the failure and installs nothing. It keeps
running afterwards; the test ends it. The full list is in
`src/unattended.rs`.

### Windows

`tools/e2e/windows/run-e2e.ps1` (PowerShell 7) runs the Windows end-to-end
flows on a Windows host of the declared architecture:

```powershell
./tools/e2e/windows/run-e2e.ps1 -Arch x86_64 -Flow inno-setup, portable
```

- **inno-setup**: builds 1.0.0 and 1.1.0, packages each with
  [`packaging/windows/reference-app.iss`](packaging/windows/reference-app.iss),
  installs 1.0.0 per user into a non-default directory, and runs it. The
  update must run the 1.1.0 installer only after 1.0.0 quit cleanly on its
  own, replace the files in that same directory, and relaunch 1.1.0.
- **portable**: runs a 1.0.0 executable that must replace itself with the
  1.1.0 executable and restart into it.

Each run signs its feed with a disposable key and serves it from loopback.
The installers append their own lines to the same report. The
`Windows end-to-end` workflow runs both flows on x86_64 and ARM64 runners.

### Linux

`tools/e2e/linux-update.sh` updates a managed install from 1.0.0 to 1.1.0 and
then rolls back a broken 1.2.0 built with `REFERENCE_APP_E2E_FAIL_TO_START`.
CI runs it under `xvfb-run` with Mesa's software Vulkan driver:

```sh
xvfb-run -a tools/e2e/linux-update.sh
```
