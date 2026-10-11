# Troubleshooting

Start with three questions. They locate most problems before you read any log.

1. **What does the updater think the installation is?**
   `updater.read(cx).capability()` and `state()`. A `Disabled` state means the
   updater is deliberately idle; the capability says why (see
   [package-manager ownership](package-managers.md)).
2. **What was the last error?** `updater.read(cx).last_error()`, and
   `previous_update_failure()` after a restart. `UpdateError::kind()` is the
   class, `message()` is safe to show, and `diagnostic()` has the detail.
3. **Is the release itself well formed?** `gpui-auto-update doctor` checks the
   project before release, and `gpui-auto-update verify --feed <URL> --public-key
   <KEY>` audits what is published ([doctor](doctor.md),
   [verification](verification.md)).

## Getting logs

- **Rust side.** The crates log through the [`tracing`](https://crates.io/crates/tracing)
  crate. Nothing is printed unless your application installs a subscriber
  (for example from the `tracing-subscriber` crate). Diagnostics contain paths and URLs, so treat
  them as private.
- **Sparkle on macOS.** Sparkle writes its own log to the unified logging
  system, and Sparkle's installer runs as a separate process, so the story of
  a failed update is often only there. Open Console.app and filter on your
  app's name, or stream from a terminal while you reproduce the problem:

  ```sh
  log stream --level debug --predicate 'subsystem == "org.sparkle-project.Sparkle" OR process == "YourApp" OR process == "Autoupdate" OR process == "Updater"'
  ```

  To read a past run, use `log show --last 30m --info --debug` with the same
  predicate. `org.sparkle-project.Sparkle` is the subsystem string compiled
  into Sparkle 2.10.0; if a later Sparkle changes it, search Console.app for
  "Sparkle" instead. Attach this output to bug reports, with private paths
  removed.
- **Windows installers.** Set a log file with `InnoSetup::with_log_file` and
  read what Inno Setup recorded. The backend adds `/LOG="<file>"` to the
  installer command line ([details](windows-installers.md#inno-setup-handoff)).
- **Linux helper.** The helper inherits the application's standard error, so
  start the application from a terminal. A failure that happens after the
  application quit is persisted for the next start; see
  [Linux](#linux) below.

## By symptom

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| Checks do nothing, state is `Disabled` | The installation is externally managed, unsupported, or temporarily unavailable. | Read `capability()`. For Linux, see [Linux](#linux). |
| `Configuration` error mentioning a feed URL or key | Missing or malformed feed URL or public key. | Check `NativeFeed::new` / `WindowsUpdateConfig` arguments, or `SUFeedURL` and `SUPublicEDKey` on macOS. `doctor` checks them. |
| `FeedRetrieval` | The feed cannot be fetched: 404 before the first release, DNS, TLS, a redirect to `http`, or a feed larger than 1 MiB. | Fetch the URL with `curl -I`. The updater accepts `https` only. |
| `FeedParsing` | The feed is malformed, or one entry is invalid (the whole feed is rejected). | `gpui-auto-update verify --feed <URL> --public-key <KEY>`; it prints the parser's diagnostic. See [feed format](feed-format.md#validation-is-fail-closed). |
| `VersionResolution` | A version is not strict SemVer, or two entries for one platform compare equal. | Fix the entry; see [feed format](feed-format.md). |
| `Signature` | The artifact was signed with a different key than the application trusts, or changed after signing. | `gpui-auto-update keys check --public-key <KEY> --key-env <VAR>` before release; `verify` after. A mismatched key fails every enclosure. |
| `LengthMismatch` / `Download` | The artifact differs from the feed's `length`, was replaced, or exceeds the size limit (512 MiB by default). | Never overwrite a versioned artifact; publish a new version. |
| `ArchiveValidation` | A malformed or unsafe archive, wrong version or architecture inside the artifact, or a Windows artifact without the expected version resource. | See the archive rules for [Linux](linux-managed-install.md#archive-rules) and [version confirmation](windows-installers.md#version-confirmation) for Windows. |
| `HelperLaunch` | The Windows installer or the Linux helper did not start. | Windows: another setup holds the `SetupMutex`, or the installer requires elevation. The update stays staged and can be retried. |
| `QuitCoordination` | A prepare-to-install hook failed. | The update stays staged. Fix the hook; its error is reported with the event. |
| `Replacement` / `Rollback` / `HealthConfirmation` | The swap failed, or the new version did not confirm. | Linux: [diagnostics](linux-managed-install.md#diagnostics). |
| Update found but never installs in a development build | Debug builds never install unless `allow_debug_self_update(true)` is set. | Test installs with a release build, or set the option for a development feed. |
| `UpdateState::Disabled` right after `init` on macOS, outside a `.app` | Sparkle needs an application bundle. | Run the bundled app, not `cargo run`. |

## macOS

- **Build fails with "Sparkle.framework was not found".** The `sparkle` feature
  links the framework at build time. Run `gpui-auto-update sparkle fetch --out
  build/sparkle`, set `SPARKLE_FRAMEWORK_PATH=build/sparkle`, and build again.
  Default builds without the feature never need the framework.
- **The app crashes at launch: "Library not loaded: @rpath/Sparkle.framework".**
  The executable lacks the run path. Add
  `-Wl,-rpath,@executable_path/../Frameworks` (see
  [Sparkle packaging](sparkle-packaging.md#2-embed-the-framework)) and embed
  the framework in `Contents/Frameworks`. `gpui-auto-update sparkle validate
  --app MyApp.app` checks framework placement, run paths, and code signatures.
  A dev run needs `DYLD_FRAMEWORK_PATH` to point at the framework directory.
- **`UpdaterConfig::sparkle` fails.** Sparkle could not start, most often
  because `SUFeedURL` or `SUPublicEDKey` is missing from `Info.plist`.
- **Sparkle says the update is improperly signed.** The EdDSA key in the
  appcast does not match `SUPublicEDKey`, or the update's Apple code-signing
  identity changed together with the EdDSA key. Sparkle accepts a change of
  one, not both at once; see
  [key rotation](key-management.md#rotating-the-signing-key).
- **Sandboxed app cannot update.** The XPC services and entitlements depend on
  the sandbox mode. `sparkle validate` explains what is missing.
- **Sparkle's window appears for a scheduled check.** The default policy keeps
  scheduled discoveries inside your UI; check that you did not start the
  backend with a different presentation policy.

## Windows

- **Capability `Unsupported`.** The install directory is not writable by the
  current user (for example Program Files). Updates never request elevation.
  Install per user.
- **`ArchiveValidation` right after download.** The installer's `ProductVersion`
  string does not equal the feed version. Set `VersionInfoProductTextVersion`
  to the same SemVer string ([details](windows-installers.md#version-confirmation)).
- **Update installs but the old version starts.** The installer's `[Run]`
  entry must relaunch the app without `skipifsilent`
  ([required script settings](windows-installers.md#required-inno-setup-script-settings)).

## Linux

- **Capability `Unsupported` or `ExternallyManaged`.** Ask the detector why:
  `gpui_auto_update_linux::detect_current("myapp").reason()` returns a
  `DetectionReason` such as `MissingMarker`, `OutsideHome`, `NotUserOwned`,
  `PackageManager`, or `ParentNotWritable`. Each reason maps to one row of the
  [ownership table](linux-managed-install.md#ownership-requirements).
- **The update is reported as unconfirmed.** The application never called
  `Updater::main_window_opened`, or the new version needs more than 60 seconds
  to start. The previous version is kept as a backup next to the install.
- **`previous_update_failure()` is set after a restart.** A rollback or a
  failed install was recorded in `.<prefix>.gpui-auto-update-diagnostic`. The
  recorded outcomes and what they mean are in
  [diagnostics](linux-managed-install.md#diagnostics). Leftover
  `.<prefix>.gpui-auto-update-*` directories next to the install are staging or
  backup data; see [files next to the install](linux-managed-install.md#files-next-to-the-install).
- **`cargo build` fails in `xattr` with `libc` 0.2.190 or newer.** gpui 0.2.2
  does not build with it. Pin `libc` with `cargo update -p libc --precise
  0.2.189` ([compatibility](compatibility.md#known-issues)).

## Publishing

- **`doctor` reports a 404 for the feed before the first release.** Expected.
  Publish an empty feed first, or run `doctor --offline`.
- **Clients keep seeing the old release.** The feed is cached longer than
  intended. Feeds should be served with a short `Cache-Control`
  ([hosting recipes](hosting/README.md)).
- **`verify` fails after upload, before publishing feeds.** That is the gate
  working. Fix the artifact or the feed and re-run; nothing has been offered to
  users yet ([CI and release recipes](release-ci.md)).
- **A tool refuses the test key.** The publicly known test key is for local
  development only. Generate a real key ([key management](key-management.md)).
