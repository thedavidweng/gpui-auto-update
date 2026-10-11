# Sparkle packaging

On macOS, gpui-auto-update uses [Sparkle 2](https://sparkle-project.org) as the
update engine. The application bundle must therefore ship `Sparkle.framework`,
signed correctly, with the metadata Sparkle reads from `Info.plist`. The
`gpui-auto-update sparkle` commands cover that part of the release. They do not
replace your bundler (cargo-bundle, cargo-packager, Xcode, and so on): run them
on the `.app` your bundler produces.

## Steps

### 1. Fetch a pinned distribution

```sh
gpui-auto-update sparkle fetch --out build/sparkle
```

This downloads the official archive for the default pinned version from the
Sparkle GitHub releases, checks its SHA-256 against the pin, and only then
extracts it. A checksum mismatch fails and writes nothing. Other options:

- `--version 2.9.6` selects another pinned release (`sparkle versions` lists them).
- `--url <mirror>` downloads from a mirror; the pinned checksum still applies.
- `--archive <file>` uses an archive that is already on disk (offline builds).
- `--url`/`--archive` together with `--sha256 <hex>` declare a distribution
  that this tool does not pin. The checksum is still mandatory.

Downloads are https only (plain http is accepted only for loopback test
servers), size-capped, and extracted with path-traversal and symlink checks.

### 2. Embed the framework

```sh
gpui-auto-update sparkle embed --app MyApp.app --sparkle build/sparkle --sandbox <mode>
```

The framework is copied to `Contents/Frameworks/Sparkle.framework` with its
symlinks intact, and Sparkle's `LICENSE` is copied to
`Contents/Resources/ThirdPartyNotices/Sparkle/LICENSE`. Sparkle's license
requires the notice to ship with the framework.

`--sandbox` is required, because the right set of XPC services depends on it:

| Mode | App Sandbox | XPC services kept | Info.plist keys you must set |
| --- | --- | --- | --- |
| `non-sandboxed` | off | none | none |
| `sandboxed` | on, without `com.apple.security.network.client` | `Installer.xpc`, `Downloader.xpc` | `SUEnableInstallerLauncherService`, `SUEnableDownloaderService` |
| `sandboxed-network-client` | on, with `com.apple.security.network.client` | `Installer.xpc` | `SUEnableInstallerLauncherService` |

Sandboxed apps also need the
`com.apple.security.temporary-exception.mach-lookup.global-name` entitlement
with `<bundle id>-spks` and `<bundle id>-spki`. The tool explains these keys
but does not edit your `Info.plist` or entitlements.

The executable must find the framework at run time. Link it with
`-Wl,-rpath,@executable_path/../Frameworks`, for example from `build.rs`:

```rust
println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
```

### 3. Sign

```sh
gpui-auto-update sparkle sign --app MyApp.app --identity "<identity>" [--entitlements app.entitlements]
```

The tool signs in the order Sparkle documents, innermost code first:
`Installer.xpc`, `Downloader.xpc` (its entitlements are preserved),
`Autoupdate`, `Updater.app`, `Sparkle.framework`, and the app last. Everything
gets the hardened runtime and, for real identities, a secure timestamp, which
notarization requires. Without `--entitlements`, the app keeps its existing
entitlements. Other nested code that your app ships (other frameworks or helper
tools) remains your bundler's responsibility. `validate` catches it if it is
unsigned.

Credentials never live in project configuration. Pass the identity with
`--identity` or `GPUI_AUTO_UPDATE_SIGNING_IDENTITY`. In CI, import the
certificate into a temporary keychain and pass `--keychain` or
`GPUI_AUTO_UPDATE_KEYCHAIN`.

`--identity -` signs ad hoc for local testing. With the hardened runtime, the
system enforces library validation, and library validation refuses to load an
ad-hoc signed framework (the framework has no Team ID to match the app's). For
that reason, an ad-hoc signed app itself is signed without the hardened
runtime. Ad-hoc builds cannot be notarized or distributed.

Notarize the signed app with your usual flow, keeping credentials in the
keychain or CI secrets, for example:

```sh
ditto -c -k --keepParent MyApp.app MyApp.zip
xcrun notarytool submit MyApp.zip --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple MyApp.app
```

### 4. Validate

```sh
gpui-auto-update sparkle validate --app MyApp.app [--require-developer-id] [--previous-build-version 41]
```

The command reports errors and warnings and exits with status 1 if there is any
error. It checks the following:

- **Info.plist**: `CFBundleIdentifier` is reverse-DNS; `CFBundleShortVersionString`
  and `CFBundleVersion` are one to three dot-separated integers, so Sparkle can
  order releases (and, with `--previous-build-version`, the build version
  increases); `SUFeedURL` is https; `SUPublicEDKey` is a base64 32-byte Ed25519
  key; `LSMinimumSystemVersion` is at least the embedded Sparkle's minimum (12.0
  for 2.10.0); the update-policy keys (`SUEnableAutomaticChecks`,
  `SUAutomaticallyUpdate`, `SUScheduledCheckInterval`, and others) have the
  right types.
- **Framework**: the framework is at `Contents/Frameworks/Sparkle.framework`,
  `Versions/Current` is valid, there are no dangling symlinks, the helpers are
  present, and the version is pinned.
- **Run path**: every architecture slice of the main executable resolves
  `@rpath/Sparkle.framework/...` to the embedded copy, following dyld's search
  order. The command warns if the executable does not link Sparkle.
- **Sandbox**: the mode comes from `--sandbox` or from the signed entitlements,
  and the two must agree. The command checks the XPC services, the Info.plist
  keys, and the mach-lookup exceptions that the mode needs.
- **License notice**: the Sparkle license is present in the bundle.
- **Signing** (macOS only; skip with `--no-signature-checks`): every component
  is signed with the hardened runtime and by the app's team (library
  validation), `codesign --verify --deep --strict` passes, and there is no
  `get-task-allow`. `--require-developer-id` also requires Developer ID
  Application signatures with secure timestamps.

## Sparkle version policy

- The tool pins official Sparkle release archives by SHA-256 in
  `crates/gpui-auto-update-cli/sparkle-pins.json`. The default is the current
  maintained stable release (2.10.0 since this tooling was written). 2.9.6 is
  kept because the `sparkle-updater` crate was tested against it.
- Pins are only added after the archive has been downloaded and hashed
  independently. Unpinned archives can be used only with an explicit
  `--sha256`.
- The [Sparkle release check](../.github/workflows/sparkle-release-check.yml)
  workflow runs daily. When the latest stable Sparkle release is newer than the
  default pin, it opens an issue with the release's computed checksum and a
  checklist for updating the pin. If the release notes mention security fixes,
  the issue is labeled `security` and should be handled promptly rather than
  left pinned.
- The same workflow re-verifies the pinned archive against GitHub and packages,
  signs, validates, and launches a test app against the real framework
  (`cargo test -p gpui-auto-update-cli -- --ignored`).
