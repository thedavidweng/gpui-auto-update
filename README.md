# gpui-auto-update

**Sparkle-quality automatic updates for GPUI.**

**Sparkle on macOS. Signed native updates on Windows and Linux. One observable GPUI API.**

[![CI](https://github.com/thedavidweng/gpui-auto-update/actions/workflows/ci.yml/badge.svg)](https://github.com/thedavidweng/gpui-auto-update/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/gpui-auto-update.svg)](https://crates.io/crates/gpui-auto-update)
[![docs.rs](https://docs.rs/gpui-auto-update/badge.svg)](https://docs.rs/gpui-auto-update)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

> **Status: pre-release.** The macOS, Windows, and Linux backends, the GPUI
> facade, the neutral controls, and the release tooling are implemented and
> tested, but nothing has been published to crates.io yet and the public API
> can still change. Until the first release, depend on the repository by Git
> (see [Quick start](#quick-start)); the crates.io and docs.rs badges above
> start working with the first release.

> **Security warning:** shipping an updater without artifact signature
> verification is not a supported production configuration. Verification is
> fail-closed by default: a missing or invalid signature is an error, never a
> warning. See the [security model](#security-model).

`gpui-auto-update` gives a GPUI application one `Updater` entity to observe and
a few standard actions (check, install, restart, dismiss). Behind it, Sparkle 2
updates the macOS app, and signed, architecture-specific feeds drive native
updates on Windows and Linux. The project also includes the tooling to package,
sign, publish, and verify releases. You choose the UI: use the optional neutral
controls or render the state yourself.

The runtime crate does not embed Sparkle and does not sign anything. Adding the
dependency lets your app *check and install* updates. Packaging the app,
signing artifacts, and publishing releases are separate steps that this project
documents and automates (see [Release and signing workflow](#release-and-signing-workflow)).

## Supported platforms and features

| | macOS | Windows | Linux |
| --- | --- | --- | --- |
| Update engine | Sparkle 2 | Native backend | Native backend |
| Feed | Sparkle appcast | Signed native feed | Signed native feed |
| Signature | Ed25519 (`sparkle:edSignature`), plus Sparkle's Apple code-signing check | Ed25519, same key and encoding | Ed25519, same key and encoding |
| Architectures | As you build and publish them | `x86_64`, `aarch64` | `x86_64`, `aarch64` |
| Package | `.app` in a Sparkle archive | Per-user Inno Setup installer, portable `.exe`, or your own installer strategy | `.tar.gz` of a managed prefix |
| Install and relaunch | Sparkle | Installer handoff, or in-place executable swap | Helper swap, then relaunch |
| Rollback | Sparkle's installer | Portable swap restores the original on failure | Automatic, until the new version confirms it started |
| Delta updates | Yes, through Sparkle | No | No |
| Channels | Yes | Yes | Yes |
| Package-manager detection | Declared by the app | Declared by the app | Detected (Flatpak, Nix, Guix, Snap, Linuxbrew, system paths) |

Also on every platform: a persisted automatic-update preference, critical-update
and release-note metadata, save-before-restart hooks, preview states for
building UI, and structured errors that do not leak paths into user-facing text.

## Quick start

This path is for an application that ships on macOS only. Other platforms
continue [below](#going-cross-platform) with the same app.

**1. Add the dependency.** The `sparkle` feature links `Sparkle.framework`. It
is opt-in because building with it needs the framework on disk (step 2).

```toml
[dependencies]
gpui = "0.2.2"
gpui-auto-update = { git = "https://github.com/thedavidweng/gpui-auto-update", features = ["sparkle"] }
```

**2. Install the CLI and fetch Sparkle.** The download is checked against a
pinned SHA-256 before anything is extracted.

```sh
cargo install --locked --git https://github.com/thedavidweng/gpui-auto-update gpui-auto-update-cli
gpui-auto-update sparkle fetch --out build/sparkle
export SPARKLE_FRAMEWORK_PATH=build/sparkle   # needed to build with the `sparkle` feature
```

**3. Create the signing key.** It is stored in your login Keychain, and the
command prints the public key. Choose it before your first public release.

```sh
gpui-auto-update keys generate --keychain --sparkle-bin build/sparkle/bin
```

**4. Configure the bundle.** Put two keys in `Info.plist`: `SUFeedURL` (your
appcast URL, https) and `SUPublicEDKey` (the public key from step 3).

**5. Start the updater** and add a menu item:

```rust,ignore
use gpui::{App, Application, Menu, MenuItem};
use gpui_auto_update::{CheckForUpdates, UpdaterConfig};

fn main() {
    Application::new().run(|cx: &mut App| {
        match UpdaterConfig::sparkle("com.example.App") {
            Ok(config) => {
                gpui_auto_update::init(config, cx);
            }
            Err(error) => eprintln!("updates unavailable: {error}"),
        }
        cx.set_menus(vec![Menu {
            name: "Example".into(),
            items: vec![MenuItem::action("Check for Updates…", CheckForUpdates)],
        }]);
        // Open your window here.
    });
}
```

**6. Package and publish.** Embed and sign the framework in your `.app`, then
publish a signed appcast and the archive. The commands are in
[macOS setup](#macos-sparkle-setup) and
[Release and signing workflow](#release-and-signing-workflow). The updater runs
from a bundled `.app`: outside a bundle `UpdaterConfig::sparkle` reports an
unsupported installation, and debug builds never install (see
[defaults](#what-you-get-by-default)).

### Going cross-platform

Keep the same app, key, and `init` call. Add a configuration per target, and
publish one signed feed per OS and architecture.

- The signing key and public key are the same everywhere.
- **Windows:** build `UpdaterConfig::windows` (see [Windows setup](#windows-setup)).
- **Linux:** build `UpdaterConfig::native_feed`, call
  `run_update_helper_if_requested()` first thing in `main`, call
  `main_window_opened` once your window is up, and package a managed prefix
  (see [Linux setup](#linux-managed-install-setup)).
- Choose between configurations with `#[cfg(target_os = "macos")]`,
  `#[cfg(windows)]`, and `#[cfg(target_os = "linux")]`. Each returns an
  `UpdaterConfig` for the same `gpui_auto_update::init`.

The Linux form is below. It compiles on every platform.

```rust,no_run
use gpui::{App, Application};
use gpui_auto_update::core::{trust::TrustedKey, version::ReleaseVersion};
use gpui_auto_update::{NativeFeed, UpdaterConfig};

const PUBLIC_KEY: &str = "<your SUPublicEDKey>";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Finishes an update if this process was started as the update helper;
    // otherwise returns at once. Must run before anything else.
    gpui_auto_update::run_update_helper_if_requested();

    let feed = NativeFeed::new(
        "https://updates.example.com/appcast-linux-x86_64.xml",
        TrustedKey::from_base64(PUBLIC_KEY)?,
        ReleaseVersion::parse(env!("CARGO_PKG_VERSION"))?,
    )?;
    Application::new().run(move |cx: &mut App| {
        let config = UpdaterConfig::native_feed("com.example.App", feed);
        let updater = gpui_auto_update::init(config, cx);
        // ... open the main window, then confirm that this version started:
        updater.update(cx, |updater, cx| updater.main_window_opened(cx));
    });
    Ok(())
}
```

[`apps/reference-app`](apps/reference-app/) is a complete example that builds
for all three platforms and is used to test real updates.

## What you get by default

- **No surprise installs.** Automatic checks only look for updates. On Windows
  and Linux, downloading and installing start when the user (or your code) asks.
- **A conservative automatic-update policy.** One background check near launch,
  at most once an hour, and a check right after the user turns automatic updates
  on. See [policy](#automatic-update-policy).
- **Manual checks always give feedback:** up to date, available, or an error.
- **Fail-closed verification.** Length and Ed25519 signature are verified
  before an artifact is read. See the [security model](#security-model).
- **No blocking on the UI thread.** Network and filesystem work runs on GPUI's
  background executor, and results are applied in order on the foreground.
- **Debug builds never install.** Running from your checkout cannot replace it
  with a production release, unless you call
  `UpdaterConfig::allow_debug_self_update(true)`.
- **Ownership checks.** An installation the updater cannot prove it owns is left
  alone, and the state says why. See
  [package-manager behavior](#package-manager-behavior).
- **Save before restart.** `Updater::on_prepare_to_install` hooks run, and are
  awaited, before the application quits or restarts for an update.

## GPUI state and UI integration

`gpui_auto_update::init` returns an `Entity<Updater>` and registers the actions
`CheckForUpdates`, `InstallUpdate`, `RestartToUpdate`, and `DismissUpdate`.
Observe the entity and render from `state()`:

```rust,no_run
use gpui::{Context, Entity, IntoElement, Render, Subscription, Task, Window, div, prelude::*};
use gpui_auto_update::{Updater, core::UpdateState};

struct UpdateBadge {
    updater: Entity<Updater>,
    _observe: Subscription,
    _save_before_update: Subscription,
}

impl UpdateBadge {
    fn new(updater: Entity<Updater>, cx: &mut Context<Self>) -> Self {
        // Dropping the subscription unregisters the hook, so keep it.
        let save = updater.update(cx, |updater, _| {
            updater.on_prepare_to_install(|_cx| {
                // Save your documents here; the updater waits for this task.
                Task::ready(Ok(()))
            })
        });
        Self {
            _observe: cx.observe(&updater, |_, _, cx| cx.notify()),
            _save_before_update: save,
            updater,
        }
    }
}

impl Render for UpdateBadge {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = match self.updater.read(cx).state() {
            UpdateState::Available(update) => format!("Version {} is available", update.version),
            UpdateState::Staged(update) => format!("Restart to install {}", update.version),
            UpdateState::Failed(error) => error.message().to_owned(),
            _ => String::new(),
        };
        div().child(text)
    }
}
```

`UpdateState` is non-exhaustive, so keep a wildcard arm. The states and their
transitions are in the [state machine](docs/state-machine.md).

- **Neutral controls.** `gpui-auto-update-ui` provides `UpdateControls` (a full
  panel), `UpdateIndicator` (an unobtrusive affordance), and `UpdateSummary`
  (plain data for your own rendering). They inherit your theme and need no
  design system.
- **Preview states.** `UpdaterConfig::preview` and `Updater::enter_preview` show
  clearly marked fake states (`PreviewState`) for building and testing UI
  without a real update.

## macOS (Sparkle) setup

Sparkle does the work on macOS: checking, the UI for manual checks, downloading,
verifying, installing, and relaunching. This project packages and signs the
bundle correctly and mirrors Sparkle into the shared state. On the `.app` your
bundler (cargo-bundle, cargo-packager, Xcode, or another) produced:

```sh
gpui-auto-update sparkle embed --app MyApp.app --sparkle build/sparkle --sandbox non-sandboxed
gpui-auto-update sparkle sign --app MyApp.app --identity "Developer ID Application: Example (TEAMID)"
gpui-auto-update sparkle validate --app MyApp.app
```

Also required: a run path so the executable finds the framework
(`-Wl,-rpath,@executable_path/../Frameworks`), and notarization with your usual
tooling. Sandboxed apps need extra `Info.plist` keys and entitlements.
[Sparkle packaging](docs/sparkle-packaging.md) covers all of it, including the
Sparkle version policy: archives are checksum-pinned, and a daily workflow flags
new stable Sparkle releases.

## Windows setup

The Windows backend updates per-user installations from the feed for the app's
architecture. Declare how updates are applied:

```rust,ignore
use gpui_auto_update::UpdaterConfig;
use gpui_auto_update::core::feed::Arch;
use gpui_auto_update::core::trust::TrustedKey;
use gpui_auto_update::core::version::ReleaseVersion;
use gpui_auto_update::windows::{InnoSetup, UpdateStrategy, WindowsUpdateConfig};

let windows = WindowsUpdateConfig::new(
    ReleaseVersion::parse(env!("CARGO_PKG_VERSION"))?,
    TrustedKey::from_base64(PUBLIC_KEY)?,
    UpdateStrategy::inno_setup(InnoSetup::new()),
)
.with_feed(Arch::X86_64, "https://updates.example.com/appcast-windows-x86_64.xml".parse()?)
.with_feed(Arch::Aarch64, "https://updates.example.com/appcast-windows-aarch64.xml".parse()?);
let config = UpdaterConfig::windows("com.example.App", windows)?;
```

- **Inno Setup:** the verified installer runs silently over the running
  installation as the current user, then relaunches the app.
- **Portable:** the verified executable replaces the running one, and the app
  restarts into it.
- **Other installers** (MSI and so on) plug in through the `InstallerStrategy`
  trait.

Updates never request elevation, and an install directory the user cannot write
to (such as Program Files) is reported as unsupported. The installer or
executable must carry a `ProductVersion` equal to the feed version, which the
updater checks before running anything. Authenticode-sign your binaries as well.
Details, the required Inno Setup script settings, and failure modes:
[Windows installers](docs/windows-installers.md).

## Linux managed-install setup

On Linux the updater only updates installations it can prove it owns: a
user-local directory (the *prefix*) with `bin/<app>` and an ownership marker at
`share/<app>/gpui-auto-update.managed`, inside the user's home directory, owned
by the user, and not under a system location or a package manager's store.
Everything else reports why self-update is off.

- A release is a signed `.tar.gz` with one root directory,
  `<app>-<version>-linux-<arch>/`, holding the whole prefix including the
  marker. Extraction rejects path traversal, links, devices, duplicates, and
  oversized archives.
- An update is finished by your own executable running as a short-lived helper,
  which is why `run_update_helper_if_requested()` must run first in `main`. The
  helper swaps the staged prefix into place and relaunches. If the new version
  exits before `main_window_opened` confirms it started, the helper **rolls
  back** to the previous version.
- After a rollback, `Updater::previous_update_failure()` reports what happened.

The contract is in [Linux managed install](docs/linux-managed-install.md), and
the design in [ADR 0003](docs/adr/0003-linux-update-helper.md).

## Automatic-update policy

`CheckPolicy::recommended()` is the default. Automatic updates are on until the
user turns them off. It checks near launch, at most once per hour, with no
periodic checks, and checks immediately when the user enables the preference.
Manual checks are never restricted. Tune it with `UpdaterConfig::with_policy`:

```rust,no_run
use std::time::Duration;

use gpui_auto_update::UpdaterConfig;
use gpui_auto_update::core::CheckPolicy;

fn every_six_hours(config: UpdaterConfig) -> UpdaterConfig {
    config.with_policy(
        CheckPolicy::recommended()
            .with_minimum_interval(Duration::from_secs(6 * 60 * 60))
            .with_periodic_interval(Some(Duration::from_secs(24 * 60 * 60))),
    )
}
```

The user's choice is `Updater::set_automatic_checks`. On Windows and Linux it is
saved atomically in a per-user file (see `default_preferences_path`), and a
corrupt file falls back to the defaults. On macOS Sparkle owns the preference
and its schedule, and the facade mirrors it, so the facade's own launch and
periodic checks are off there.

## Release and signing workflow

Four separate jobs, none of which the runtime crate does for you:

1. **Package** each platform's artifact with your bundler. On macOS, embed and
   sign Sparkle (above).
2. **Sign** every artifact with one Ed25519 key and write the feeds:

   ```sh
   # Windows and Linux: one signed feed per OS and architecture
   gpui-auto-update feed native --os linux --arch x86_64 --version 1.5.0 \
     --artifact dist/example-1.5.0-linux-x86_64.tar.gz \
     --download-url-prefix https://downloads.example.com/releases/1.5.0/ \
     --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY \
     --feed site/appcast-linux-x86_64.xml --output site/appcast-linux-x86_64.xml

   # macOS: Sparkle's generate_appcast, including deltas
   gpui-auto-update feed sparkle --sparkle build/sparkle --archives dist/macos \
     --download-url-prefix https://downloads.example.com/releases/1.5.0/ \
     --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY \
     --output site/appcast.xml
   ```

3. **Publish** artifacts first and feeds last, and never overwrite a versioned
   artifact.
4. **Verify** what is live: `gpui-auto-update verify --feed <URL> --public-key
   "$APP_PUBLIC_ED_KEY" --expect-version 1.5.0`.

The private key reaches tools only through an environment variable, standard
input, a file, or the macOS Keychain, never as an argument. Rotating a key needs
a bridge release, so read [key management](docs/key-management.md) before your
first public release. Feeds: [format](docs/feed-format.md) and
[generation](docs/feed-generation.md). A complete CI workflow template is in
[`release/`](release/README.md), and [CI and release recipes](docs/release-ci.md)
ties the pieces together.

## Hosting feeds and artifacts

Any HTTPS static host works. Serve two kinds of files differently: versioned
artifacts (never overwritten, cached for a long time) and mutable feeds (short
cache lifetime, written last). Recipes for GitHub Releases plus a stable feed
location, Cloudflare R2, S3-compatible storage, and generic static hosting are
in [docs/hosting](docs/hosting/README.md).

## Package-manager behavior

The updater never modifies an installation it cannot prove it owns. On Linux it
detects Flatpak, Nix, Guix, Snap, Linuxbrew, and system locations. On Windows an
install directory the user cannot write is unsupported. It does **not** detect
Homebrew casks on macOS, or Scoop, winget, and Chocolatey on Windows. If you
distribute through them, mark that build with
`UpdaterConfig::with_capability(Capability::ExternallyManaged { manager: Some("Homebrew".into()) })`.
The updater then stays visible, says who updates the app, and refuses to
install. See [package-manager ownership](docs/package-managers.md).

## Security model

Updates are a code-execution supply chain. In short:

- One Ed25519 public key, compiled into the app, authorizes every artifact.
  Transport security is necessary and not sufficient.
- Feeds and artifacts are size-bounded and fetched over HTTPS only, with checked
  redirects. Staging uses fresh private directories, and the only feed value
  that can become part of a path is a strict SemVer version.
- The version inside a verified artifact must match the feed, so an old release
  cannot be relabeled as a new one.
- Linux archive extraction is hardened, and ownership is validated before
  anything changes.
- Private keys are never accepted as command-line arguments and never printed.

Known limits: the feed itself is not signed, so a feed host can withhold updates
or offer an older genuine release, and a compromised private key can only be
rotated through a bridge release. Read the full
[security and trust model](docs/security.md). To report a vulnerability, see
[SECURITY.md](SECURITY.md).

## CLI tooling

`gpui-auto-update` (from the `gpui-auto-update-cli` crate) is not needed at
runtime.

| Command | Purpose |
| --- | --- |
| `init` | Report what the app needs; `--write` appends a configuration skeleton to `Cargo.toml`. |
| `doctor` | Check the integration before release: `Info.plist`, keys, feeds, artifacts, CI workflows. Publishes nothing. |
| `keys` | `generate`, `import`, `public-key`, and `check` for Sparkle-compatible Ed25519 keys. |
| `sparkle` | `versions`, `fetch`, `embed`, `sign`, and `validate` for the macOS framework and bundle. |
| `feed` | `native` and `sparkle`: signed feed generation. |
| `verify` | Audit a published feed and every artifact without installing. |

Details: [`init` and `doctor`](docs/doctor.md),
[key management](docs/key-management.md),
[feed generation](docs/feed-generation.md), and
[verification](docs/verification.md).

## Compatibility policy

GPUI is pre-1.0 and changes often, so supported combinations are listed
explicitly. The project follows semantic versioning for its own API.

| gpui-auto-update | gpui | Rust (core, backends, CLI) | Rust (GPUI crates) |
| --- | --- | --- | --- |
| unreleased | 0.2.2 (official crate) | 1.85+ | latest stable |

The adapter depends on the official `gpui` crate. If you patch GPUI to a Git
revision, do it with `[patch.crates-io]` at your workspace root and make sure
only one `gpui` package identity is built: a Git `gpui` and a crates.io `gpui`
are different packages with incompatible types. Known issue: on Linux, gpui
0.2.2 does not build with `libc` 0.2.190 or newer; pin it with
`cargo update -p libc --precise 0.2.189`. Details:
[compatibility](docs/compatibility.md). Upgrades: [migration](docs/migration.md).

## Troubleshooting

Check `Updater::capability()`, `state()`, and `last_error()` first. Run
`gpui-auto-update doctor` before a release and `gpui-auto-update verify` against
what you published. On macOS, Sparkle logs to the unified logging system:

```sh
log stream --level debug --predicate 'subsystem == "org.sparkle-project.Sparkle" OR process == "YourApp"'
```

Symptoms, causes, and fixes, including Windows and Linux logs, are in
[troubleshooting](docs/troubleshooting.md).

## Architecture for contributors

A framework-independent core (state, feed, verification, policy), three platform
backends, a GPUI facade that applications depend on, optional neutral controls,
and a CLI. Start with [architecture](docs/architecture.md), then the
[ADRs](docs/adr/), the glossary in [CONTEXT.md](CONTEXT.md), and
[CONTRIBUTING.md](CONTRIBUTING.md). Every page is indexed in
[docs/README.md](docs/README.md).

## Acknowledgements / prior art

This project builds on other people's work. Thanks to:

- **[Sparkle](https://github.com/sparkle-project/Sparkle) and the Sparkle
  Project** for the macOS update framework, the appcast ecosystem, EdDSA update
  signing, delta updates, the native installation and relaunch machinery, and
  the extensible user-driver architecture. On macOS this project configures and
  packages Sparkle, and Sparkle does the updating. The native feed format reuses
  Sparkle's field conventions and key encoding.
- **[Waku](https://github.com/egoist/waku) by egoist** for demonstrating a
  strong production GPUI update experience: GPUI-native background
  presentation, Sparkle 2 on macOS, signed architecture-specific Windows and
  Linux feeds, a silent Windows installer handoff, Linux managed-prefix
  replacement with rollback, and a unified application-facing update state.
  Waku is architectural and behavioral prior art. Waku's source is GPL-licensed,
  and this permissive implementation does not copy it.
- **[hankbao/sparkle-updater](https://github.com/hankbao/sparkle-updater)** for
  the early Rust wrapper around Sparkle and WinSparkle, and especially for the
  philosophy of exposing a very small, framework-independent Rust updater
  boundary. This project targets modern Sparkle 2 and does not inherit that
  older Sparkle 1 implementation.
- **[ahonn/tauri-plugin-sparkle-updater](https://github.com/ahonn/tauri-plugin-sparkle-updater)
  and its standalone [`sparkle-updater`](https://crates.io/crates/sparkle-updater)
  crate** for the modern, runtime-independent Sparkle 2 binding, which made
  Sparkle lifecycle events and relaunch coordination accessible from Rust.
  **This project depends on that crate:** `gpui-auto-update-macos` uses
  `sparkle-updater` 0.1.0 as an optional dependency behind its `sparkle`
  feature. [ADR 0002](docs/adr/0002-sparkle-binding.md) lists the gaps we found
  and the changes we plan to propose upstream. This repository does not fork it.
- **[AprilNEA/gpui-updater](https://github.com/AprilNEA/gpui-updater)** for
  current GPUI ecosystem prior art around observable updater entities, a
  framework-independent core, GitHub and static update sources, cross-platform
  installation, verification, and GPUI restart integration. This project did not
  invent the idea of a GPUI updater, and its crates use the `gpui-auto-update`
  prefix so as not to collide with the existing `gpui-updater` name.
- **[Zed and GPUI](https://github.com/zed-industries/zed)** for GPUI itself, and
  for the production `auto_update` design that shows update state integrated
  into a large GPUI application.
- **[Liora](https://github.com/yhyzgn/liora)** as additional current GPUI
  ecosystem prior art for release discovery and update planning.

These acknowledgements credit ideas and work. They do not imply that any of
these projects endorses, or is affiliated with, this one.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
