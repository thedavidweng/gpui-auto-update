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
| `REFERENCE_APP_E2E_REPORT` | Absolute path; run unattended and append what happens to it | none |
| `REFERENCE_APP_E2E_FAIL_TO_START` | Exit with an error before the main window opens, as a broken release | `false` |

`reference-app --version` prints the compiled version and exits, so tests can
tell which build is running.

## Unattended end-to-end mode

A build with `REFERENCE_APP_E2E_REPORT` checks for updates as soon as the
updater is ready and installs whatever is offered without a click, and
appends one line per fact to the report (`started <version>`,
`update-available <version>`, `handoff`, `up-to-date`,
`previous-update-failure <kind>: <message>`, `error <kind>: <message>`,
`failed-to-start <version>`). After a failed update it reports the failure
and installs nothing. The full list is in `src/unattended.rs`.

On Linux, `tools/e2e/linux-update.sh` uses this mode to update a managed
install from 1.0.0 to 1.1.0 and then to roll back a broken 1.2.0 built with
`REFERENCE_APP_E2E_FAIL_TO_START`. CI runs it under `xvfb-run` with Mesa's
software Vulkan driver:

```sh
xvfb-run -a tools/e2e/linux-update.sh
```
