# Security and trust model

An updater is a code-execution channel: whoever can make an installed
application accept an update can run code as that user. This page states what
`gpui-auto-update` relies on, what it checks, and what it does not protect
against. To report a vulnerability, see [SECURITY.md](../SECURITY.md).

Verification is fail-closed. There is no best-effort mode that skips a missing
or invalid signature, and shipping an updater without artifact verification is
not a supported production configuration.

## What is trusted

| Trusted | Why |
| --- | --- |
| The Ed25519 **public key** compiled into the application | It is the only thing that authorizes an artifact. On macOS it is `SUPublicEDKey` in `Info.plist`; on Windows and Linux it is the `TrustedKey` the application passes to the updater. |
| The **private key** holder | Whoever holds the private key can ship code to every installation. See [key management](key-management.md). |
| The operating system | Its certificate store validates TLS, and its file permissions protect the install and staging directories. |
| **Sparkle** on macOS | Sparkle performs verification, installation, and relaunch there. Sparkle's own signature checks apply; this project does not reimplement them. |

The feed host, the CDN, and the network path are **not** trusted with the
contents of an update. They can withhold or delay updates, and nothing more
(see [Limits](#limits)).

## What is checked

For Windows and Linux (the native backends, implemented in the core crate):

1. **Transport.** Feeds and artifacts load over `https` only, with
   certificates validated against the operating system trust store. Redirects
   are followed one hop at a time, at most five, and a redirect to a scheme
   other than `https` or from `https` to `http` is refused. Responses are not
   decompressed, so size limits apply to bytes on the wire. Plain `http` can be
   enabled only through an explicit fetch policy meant for loopback test
   servers. Limits and timeouts are listed in
   [the feed format](feed-format.md#transport).
2. **Feed validation.** A feed is validated completely and one invalid entry
   rejects all of it. An entry without a well-formed `sparkle:edSignature`,
   without a strict SemVer `sparkle:version`, or with a missing or zero
   `length` is an error ([details](feed-format.md#validation-is-fail-closed)).
3. **Length and signature.** The artifact is streamed into a fresh private
   staging directory under a random name. Its length must match the feed while
   it downloads, and its Ed25519 signature must verify over the bytes on disk
   before the file receives its final name. Nothing is read from an artifact
   before that.
4. **Re-verification.** The Windows backend verifies the staged file again
   immediately before running it, so a file changed between staging and
   install never runs.
5. **Version confirmation.** The feed is not signed (see
   [Limits](#limits)), so the version inside the verified artifact must equal
   the feed's `sparkle:version`: the PE version resource on Windows
   ([details](windows-installers.md#version-confirmation)), the release root
   name on Linux. This stops a relabeled older release from being offered as
   a newer one.
6. **No feed-controlled paths.** Only a strict SemVer `sparkle:version`
   can become part of a path, and that grammar cannot express `.`, `..`,
   separators, or drive prefixes. URLs, titles, and channel names never
   become paths ([details](feed-format.md#versions-and-file-system-paths)).

For Linux installs there are additional checks:

- **Ownership before mutation.** An installation is updated only if it carries
  the exact marker, lives strictly inside the user's home directory, is owned
  by the current user, is not a package-manager or system location, and its
  parent directory is writable. Root sessions are never updated. See
  [the managed-install contract](linux-managed-install.md#ownership-requirements).
- **Hardened extraction.** Archives are extracted into a private directory
  next to the install with rules that reject traversal, absolute paths,
  links, device files, duplicates, and oversized archives, and that normalize
  permissions so setuid and world-writable bits never reach the install
  ([rules](linux-managed-install.md#archive-rules)).
- **Rollback.** The helper keeps the previous version until the new one
  confirms that it started, and restores it otherwise
  ([sequence](linux-managed-install.md#sequence)).

On Windows, updates never request elevation. An installation the current user
cannot write to reports `Capability::Unsupported` and is left alone.

On macOS, verification (EdDSA signature on the archive, plus Sparkle's check
of the update's Apple code signature) is Sparkle's. The tooling around it
(`gpui-auto-update sparkle embed|sign|validate`) exists to make the packaging
correct: a pinned framework, signed innermost-first, the hardened runtime, the
run path, and the sandbox XPC services. See
[Sparkle packaging](sparkle-packaging.md).

## Supply chain of the tooling

- Sparkle archives are always checksum-pinned. `sparkle fetch` verifies the
  SHA-256 before extracting anything, and a daily workflow opens an issue when
  a newer stable Sparkle release appears, labeled `security` when the release
  notes mention security fixes
  ([policy](sparkle-packaging.md#sparkle-version-policy)).
- The CLI accepts private keys only through standard input, an environment
  variable, a file, or the macOS Keychain, never as an argument, and never
  prints them. `doctor` flags workflows that expand a secret into a command
  line, use `--allow-test-key` or `--allow-http`, or publish feeds without
  running `verify` ([doctor](doctor.md)).
- The published test key is public and insecure. Tooling refuses it unless
  explicitly told otherwise ([key management](key-management.md#the-insecure-test-key)).
- Unsafe code is denied across the workspace and allowed only locally in
  platform backend modules that need operating system interoperability.
  `cargo-deny` checks advisories, licenses, and sources in CI.

## Limits

These are known and deliberate; plan around them.

- **Feed metadata is not signed.** Signatures cover artifacts, not the feed.
  An attacker who controls the feed (but not the private key) cannot make a
  client accept an unsigned or altered artifact. They can withhold updates
  indefinitely, or offer an older release that was genuinely signed with the
  same key and is still newer than what the client runs (for example 1.2
  instead of 1.5). Version confirmation stops an older artifact from being
  relabeled as a newer version, but it cannot stop an honest older release from
  being offered. Serve feeds from an origin only your release process can
  write to.
- **No freshness guarantee.** There is no signed expiry on a feed, so a client
  cannot tell a withheld update from no update.
- **Key compromise.** Anyone holding the private key can sign a release that
  every installation accepts. Rotation needs a bridge release signed with the
  old key ([procedure](key-management.md#rotating-the-signing-key)); until
  users take it, a compromised key remains usable against them.
- **Lost key.** A lost private key cannot be rotated. Users must reinstall
  manually. Keep an offline backup.
- **Ed25519 is not a code-signing identity.** It authorizes updates inside
  this system. SmartScreen, Gatekeeper, notarization, and enterprise policy
  rely on Authenticode and Apple signatures, which you should apply as well
  ([Windows](windows-installers.md#authenticode),
  [macOS](sparkle-packaging.md#3-sign)).
- **Sparkle's guarantees are Sparkle's.** This project configures and
  packages Sparkle; it does not change what Sparkle verifies.
- **No external audit is claimed.** The project is pre-release.

## Checklist for a production integration

1. Generate the signing key before the first public release and keep an
   offline backup ([key management](key-management.md)).
2. Embed the public key, and nothing weaker, in every platform build.
3. Serve feeds and artifacts over HTTPS; never overwrite a versioned
   artifact; upload artifacts before feeds ([CI and release recipes](release-ci.md)).
4. Run `gpui-auto-update doctor` before releasing and
   `gpui-auto-update verify` against the published feeds after
   ([verification](verification.md)).
5. Authenticode-sign Windows binaries and Developer-ID-sign and notarize the
   macOS app.
6. Keep `allow_debug_self_update` off in anything you ship.
