# gpui-auto-update — domain glossary

Shared vocabulary for code, docs, issues, and tests. Use these terms as
defined here; the "avoid" notes list synonyms that should not be used for the
same concept.

## Update lifecycle

**Update state**
: The single, public, observable description of what the updater is doing
  for this installation. It is one of: disabled or externally managed, idle,
  checking, up to date, update available, downloading (known or unknown total
  size), verifying, staged (ready to install), installing, waiting for
  application quit, relaunching, rolled back, completed, or failed. Defined in
  the core crate and mirrored by the facade. Avoid: "status" for this concept.

**Check**
: One attempt to resolve the newest applicable update from the feed. A check is
  either *manual* (user initiated, always produces visible feedback) or
  *background* (automatic, silent unless an update is found). A manual check
  that starts while a background check is running attaches to that check and
  receives its result.

**Available update**
: The metadata of a newer release selected by a check: version, channel,
  release notes or their location, publication date when known, and whether
  the release is critical or major.

**Automatic-update preference**
: Whether background checks run. On macOS it is stored by Sparkle; on Windows
  and Linux it is persisted atomically by this project.

**Channel**
: A named release track (for example stable or beta) that limits which feed
  entries a check may select.

## Trust and distribution

**Feed**
: The signed, architecture-specific, Sparkle-appcast-compatible XML document
  that lists releases. On macOS it is Sparkle's appcast; on Windows and Linux
  it is the project's native feed with the same field conventions.
  Avoid: "manifest" for this concept.

**Artifact**
: The downloadable file a feed entry points to (Sparkle archive or DMG,
  Windows installer or portable archive, Linux tarball). Every artifact has an
  expected length and an Ed25519 signature that must verify before use.

**Verification**
: Checking an artifact's length and Ed25519 signature against the configured
  public key. Production verification is fail-closed: a missing or invalid
  signature is an error, never a warning.

## Installation and ownership

**Capability (ownership)**
: Whether this running installation may update itself: *self-managed and
  updateable*, *externally managed* (for example by Homebrew or a system
  package manager), *unsupported*, or *temporarily unable to update*. The
  updater never modifies an installation it cannot prove it owns.

**Managed install**
: On Linux, a user-local installation prefix that carries this project's
  ownership marker and layout. Only managed installs are self-updated.

**Staging**
: Placing a verified, extracted artifact next to the install so that it can be
  swapped in without further network access.

**Helper**
: The short-lived process that finishes an update after the application has
  quit. On Linux it swaps the staged release into the managed install, waits
  for relaunch health confirmation, and rolls back on failure. On Windows the
  role is played by the installer handoff. On macOS it is Sparkle's own
  installer.

**Health confirmation**
: The signal a relaunched application sends to show that the new version
  started successfully. Without it the helper performs a **rollback** to the
  previous version.

**Prepare-to-install hook**
: The application callback that saves state asynchronously before the updater
  quits or relaunches the application.

## Architecture

**Core**
: The framework-independent crate (`gpui-auto-update-core`) that owns the
  update state, errors, capability, feed, verification, policy, and Windows and
  Linux orchestration. It never depends on GPUI.

**Backend**
: The platform implementation of the core's contract: Sparkle 2 on macOS
  (`gpui-auto-update-macos`), installer and portable handoff on Windows
  (`gpui-auto-update-windows`), and managed-install staging with a helper on
  Linux (`gpui-auto-update-linux`). Unsafe OS interoperability lives only in
  backends.

**Facade**
: The GPUI adapter crate (`gpui-auto-update`) that ordinary applications
  depend on. It owns the observable GPUI entity and actions, schedules
  blocking work off the foreground executor, and selects the backend for the
  target.

**Neutral controls**
: The optional GPUI UI crate (`gpui-auto-update-ui`) with theme-inheriting
  reference controls that applications may use or replace.

**Preview state**
: A deterministic, clearly marked fake update state (for example "update
  available" or "downloading") used for UI development and tests. A preview
  state never installs anything and is visibly distinct from a real updater.

**Reference application**
: The small GPUI app in `apps/reference-app` used to validate real
  old-version to new-version updates on every platform.
