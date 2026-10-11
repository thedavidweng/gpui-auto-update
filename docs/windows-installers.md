# Windows installers and portable updates

On Windows, `gpui-auto-update` updates per-user installations from a signed,
architecture-specific feed. The application declares how updates are applied.
Two strategies are built in:

- **Inno Setup** (`UpdateStrategy::inno_setup`): the verified installer runs
  silently over the running installation and relaunches the application.
- **Portable** (`UpdateStrategy::portable`): the verified executable replaces
  the running one in place, and the application restarts into it.

Other installer technologies, such as MSI, plug in through the
`InstallerStrategy` trait (see [MSI and other installers](#msi-and-other-installers)).
Updates never request elevation. An installation the current user cannot
write to (for example a machine-wide install under Program Files) reports
`Capability::Unsupported`.

The backend is `gpui_auto_update_windows::WindowsBackend`. Applications using
the GPUI facade build it with `UpdaterConfig::windows`:

```rust,ignore
use gpui_auto_update::UpdaterConfig;
use gpui_auto_update::core::feed::Arch;
use gpui_auto_update::core::trust::TrustedKey;
use gpui_auto_update::core::version::ReleaseVersion;
use gpui_auto_update::windows::{InnoSetup, UpdateStrategy, WindowsUpdateConfig};

let windows = WindowsUpdateConfig::new(
    ReleaseVersion::parse(env!("CARGO_PKG_VERSION"))?,
    TrustedKey::from_base64(PUBLIC_ED_KEY)?,
    UpdateStrategy::inno_setup(InnoSetup::new()),
)
.with_feed(Arch::X86_64, "https://updates.example.com/appcast-windows-x86_64.xml".parse()?)
.with_feed(Arch::Aarch64, "https://updates.example.com/appcast-windows-aarch64.xml".parse()?);
let config = UpdaterConfig::windows("com.example.App", windows)?;
```

## Architecture and feed selection

Each architecture has its own feed, declared with `with_feed(arch, url)`. The
backend uses the feed for the architecture the running binary was compiled
for, unless `with_arch` names another one (for example to move an emulated
x86_64 build on ARM64 hardware to the native ARM64 release). A missing feed
for the selected architecture is a configuration error when the backend is
created.

Only feed entries whose `sparkle:os` is `windows` and whose
`gpui-auto-update:arch` matches are considered (see
[feed-format.md](feed-format.md)). File names, URLs, and MIME types never
decide what an artifact is or which architecture it is for, and the update
strategy is the declared one, whatever the artifact is called.

## What happens during an update

1. **Check.** The feed is fetched within the configured timeouts and size
   limits, and the newest applicable signed entry is selected.
2. **Stage.** The artifact is streamed into a fresh, private staging
   directory under a random, extensionless name, its length is checked
   against the feed while it downloads, and its Ed25519 signature is verified
   over the bytes on disk. Only then is it given its final name (`setup.exe`
   for Inno Setup). Nothing runs.
3. **Confirm the version.** The release version is read from inside the
   verified bytes (see [Version confirmation](#version-confirmation)) and must
   equal the feed entry's `sparkle:version`. Portable executables must also be
   built for the configured architecture. The update is reported as staged
   only after this succeeds.
4. **Prepare.** The application's prepare-to-install hooks run, so it can
   save its state.
5. **Install.** The staged file's length and signature are verified again and
   its version confirmed again, so a file changed after staging never runs.
   Then the strategy hands off (below).
6. **Quit and relaunch.** The application quits through its normal quit path.

### Inno Setup handoff

The installer is started directly with `CreateProcess`, as the current user,
without the `runas` verb. An installer whose manifest requires administrator
rights fails to start (`ERROR_ELEVATION_REQUIRED`) and is reported as an
error; it never shows an elevation prompt. The installer is detached from the
application's console and, where Windows allows it, from its job object, and
its working directory is the staging directory so the install directory is
not held open.

The command line is the configured switch set, then `/LOG="<file>"` when a
log file is configured, then `/DIR="<install dir>"`, where the install
directory is the directory of the running executable. That keeps a relocated
or portable-style copy updating where it runs instead of creating a second
installation at the script's default location.

The default switch set is:

| Switch | Purpose |
| --- | --- |
| `/VERYSILENT` | No wizard and no progress window. `InnoSetup::passive()` uses `/SILENT` instead, which shows only a progress window. |
| `/SUPPRESSMSGBOXES` | Message boxes take their default answer instead of waiting for the user. |
| `/NORESTART` | Never reboot Windows. |
| `/SP-` | No "This will install..." prompt. |
| `/CURRENTUSER` | Non-administrative install mode, when the script allows command-line overrides. Ignored otherwise. |
| `/CLOSEAPPLICATIONS` | Restart Manager closes any process that still holds files being replaced. |
| `/NORESTARTAPPLICATIONS` | Restart Manager does not restart what it closed. The script's `[Run]` entry relaunches the application instead, so it is not started twice. |

Replace the set with `InnoSetup::with_switches` or extend it with
`InnoSetup::with_extra_switch` (for example `/MERGETASKS=!desktopicon` or
`/NOCANCEL`). Each switch must be a single token starting with `/`. The backend
refuses `/DIR=` (it always sets the install directory itself), `/ALLUSERS`
(updates never elevate), and `/LOG` (use `InnoSetup::with_log_file`).

After starting the installer, the backend watches it for a short grace period
(`DEFAULT_LAUNCH_GRACE`, 750 ms, configurable with `with_launch_grace`). An
installer that cannot be started, or that exits with a non-zero code during
that time (for example because another setup holds the `SetupMutex`), is
reported as an `ErrorKind::HelperLaunch` error while the application is still
running, and the update stays staged so it can be retried. Otherwise the
application quits and the installer owns the rest.

### Ordering and locked files

Windows does not allow a running executable, or a DLL it has loaded, to be
overwritten or deleted. The handoff therefore happens in this order:

1. The application saves its state (prepare-to-install hooks).
2. The verified installer is started.
3. The application quits promptly through its normal quit path. With the
   GPUI facade this is `Handoff::Quit`, and GPUI does not restart the old
   executable.
4. The installer's file replacement begins only after Restart Manager has
   found no process using the files, closing any instance that is still
   running (`/CLOSEAPPLICATIONS` and `CloseApplications=force`).
5. The installer's `[Run]` entry starts the new version.

Starting the installer before the application exits is deliberate: a failure
to start it can still be shown to the user, and nothing needs to stay behind
to start it later. The installer, not a timing assumption, prevents the
locked-file race: Inno Setup's Restart Manager integration waits for or
closes the application before it touches its files, and `SetupMutex` stops
two installers from running at once.

### Required Inno Setup script settings

```ini
[Setup]
; Never change AppId after the first release; it identifies the installation.
AppId={{01234567-89AB-CDEF-0123-456789ABCDEF}
AppName=Example
AppVersion={#AppVersion}
; Per-user install without elevation. {autopf} resolves to
; %LOCALAPPDATA%\Programs for non-administrative installs.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\Example
UsePreviousAppDir=yes
; The updater confirms the release from these version resources.
VersionInfoVersion={#AppVersionNumeric}
VersionInfoProductTextVersion={#AppVersion}
; One installer at a time, and close the running app before replacing files.
SetupMutex=ExampleSetupMutex
CloseApplications=force
RestartApplications=no
; Architectures the installer supports (one installer per feed).
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Run]
; No skipifsilent, so silent updates relaunch the application.
Filename: "{app}\Example.exe"; Flags: nowait postinstall
```

For the ARM64 installer use `arm64` for both architecture directives. Pass the
version when compiling, for example
`iscc /DAppVersion=1.5.0 /DAppVersionNumeric=1.5.0.0 example.iss`.

## Version confirmation

The Ed25519 signature proves that the bytes come from the release key, but the
feed entry that labels them with a version is not signed. Without a check, an
attacker who controls the feed host could relabel an older, correctly signed
release as a newer one. The backend therefore reads the version from inside
the verified bytes and requires it to equal the feed's `sparkle:version`
exactly.

Built-in strategies read the PE version resource of the artifact
(`VS_VERSIONINFO`, `StringFileInfo`) and compare the `ProductVersion` string:

- Inno Setup writes `VersionInfoProductTextVersion` to the installer's
  `ProductVersion` string. Set it explicitly to `{#AppVersion}`, the same
  semver string the feed uses (for example `1.5.0` or `2.0.0-beta.1`).
  `VersionInfoVersion` must be four numbers and cannot represent pre-releases,
  so it is not used for confirmation.
- Portable executables need the same string in their own version resource,
  for example with the `winres` or `embed-resource` crates
  (`ProductVersion = env!("CARGO_PKG_VERSION")`). The executable's machine
  type must also match the configured architecture (`IMAGE_FILE_MACHINE_AMD64`
  or `IMAGE_FILE_MACHINE_ARM64`). Inno Setup installers are always 32-bit x86
  programs, so their machine type is not checked.

Every string table that contains the key must hold the expected text;
surrounding whitespace, such as the trailing spaces Inno Setup pads its version
strings with, is ignored. A missing version resource, a missing key, or a
different value fails staging with `ErrorKind::ArchiveValidation`, and
nothing is installed. Use `with_version_key` to compare a different string,
such as `FileVersion`. The reader parses only headers and the resource tree, bounds-checks every offset,
and caps every read, so it is safe to run on any verified artifact.

## Portable installations

A portable application is a single executable, possibly with data files the
update does not replace, whose feed artifact is the new executable itself.

- The staging root defaults to a `.gpui-auto-update` directory inside the
  install directory, so the final move never crosses volumes and is an atomic
  rename.
- At install time the running `<name>.exe` is renamed to
  `<name>.exe.previous` (Windows allows renaming a running executable but not
  overwriting it), and the verified executable is renamed into its place. If
  that second rename fails, the original is renamed back.
- The application then restarts into the new executable. With the GPUI facade
  this is `Handoff::Restart` with the executable as the restart path, and GPUI
  starts it after the old process has exited.
- On the next launch, `capability()` removes `<name>.exe.previous`. A backup
  that an old instance still uses is left for a later launch.

Portable applications made of several executables or DLLs that must change
together need an installer strategy instead.

## MSI and other installers

`InstallerStrategy` is the extension point. An implementation describes only
the handoff; the backend still verifies the signature first, calls
`confirm_version`, starts the command as the current user without elevation,
watches it during the grace period, and quits the application.

```rust,ignore
use std::path::Path;

use gpui_auto_update::core::version::ReleaseVersion;
use gpui_auto_update::core::{ErrorKind, UpdateError};
use gpui_auto_update::windows::{InstallTarget, InstallerCommand, InstallerStrategy, UpdateStrategy};

#[derive(Debug)]
struct PerUserMsi;

impl InstallerStrategy for PerUserMsi {
    fn name(&self) -> &str {
        "MSI"
    }

    fn staged_file_name(&self) -> &str {
        "update.msi"
    }

    fn confirm_version(&self, msi: &Path, expected: &ReleaseVersion) -> Result<(), UpdateError> {
        // Read the ProductVersion property from the package's Property
        // table (for example with the `msi` crate) and compare it with
        // `expected`. Never return Ok without checking.
        let found = read_msi_product_version(msi)?;
        if found == expected.as_str() {
            Ok(())
        } else {
            Err(UpdateError::new(ErrorKind::ArchiveValidation)
                .with_diagnostic(format!("package is {found}, feed says {expected}")))
        }
    }

    fn command(&self, msi: &Path, target: &InstallTarget) -> Result<InstallerCommand, UpdateError> {
        Ok(InstallerCommand::new(r"C:\Windows\System32\msiexec.exe")
            .raw_arg("/i")
            .raw_arg(format!("\"{}\"", msi.display()))
            .raw_arg("/passive")
            .raw_arg("/norestart")
            .raw_arg("ALLUSERS=2")
            .raw_arg("MSIINSTALLPERUSER=1")
            .raw_arg(format!("INSTALLDIR=\"{}\"", target.install_dir().display())))
    }
}

let strategy = UpdateStrategy::installer(PerUserMsi);
```

Guidelines for any strategy:

- Confirm the version from metadata inside the signed artifact. MSI packages
  carry `ProductVersion` in their Property table. Formats without embedded
  metadata need a signed version marker inside the artifact.
- Install per user and without prompts, and never request elevation.
- Install into `target.install_dir()`, the directory of the running
  executable.
- Make the installer relaunch the application, because the application has
  quit by the time files are replaced. For MSI, use a custom action or the
  Restart Manager options. A strategy that cannot relaunch leaves the user to
  start the application again.
- Arguments are passed verbatim (`raw_arg`), because installers parse their
  own command lines. Quote paths yourself.

## Authenticode

Ed25519 verification is what authorizes an update. Authenticode-sign
installers and executables anyway: SmartScreen, antivirus products, and
enterprise policies rely on it, and signing does not interfere with update
verification. Sign before computing the Ed25519 signature and feed entry,
because the Ed25519 signature covers the final bytes.

## Limits and failures

| Situation | Result |
| --- | --- |
| No feed for the selected architecture | Configuration error when the backend is created |
| Install directory not writable by the user | `Capability::Unsupported` |
| Artifact larger than the size limit, or length differs from the feed | `ErrorKind::Download` or `ErrorKind::LengthMismatch`; nothing staged |
| Signature does not verify | `ErrorKind::Signature`; nothing staged |
| Embedded version or architecture differs | `ErrorKind::ArchiveValidation`; nothing staged |
| Staged file changed before install | `ErrorKind::Signature`; nothing runs |
| Installer cannot start or exits with an error at once | `ErrorKind::HelperLaunch`; update stays staged |
| Portable executable cannot be swapped | `ErrorKind::Replacement` (or `ErrorKind::Rollback` if the original could not be restored) |
