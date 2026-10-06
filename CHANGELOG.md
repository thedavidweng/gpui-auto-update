# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Cargo workspace with the core, macOS, Windows, and Linux backend, GPUI
  facade, neutral UI, CLI, and reference application crates (placeholders).
- CI on macOS, Windows, and Linux with rustfmt, clippy, tests, docs, MSRV, and
  cargo-deny checks.
- Dual MIT OR Apache-2.0 licensing, contribution guide, security policy, code
  of conduct, and issue and pull request templates.
- Core: signed native feed for Windows and Linux (Sparkle-compatible fields
  and Ed25519 keys and signatures), fail-closed feed validation, bounded HTTP
  fetching with checked redirects, and order-independent release selection.
  See `docs/feed-format.md`.
- Core: automatic-update preference persisted atomically to a JSON file on
  Windows and Linux (corrupt files fall back to defaults), and a configurable
  automatic-check policy: a launch check that respects a minimum interval,
  optional periodic checks, discovery when automatic checks are enabled, and
  backend-owned preferences for Sparkle.
- Core: verified artifact download and staging (`download` module): streamed
  download with byte progress, declared-length and size-limit enforcement,
  Ed25519 verification before the file gets its final name, and fresh private
  staging directories whose paths never come from the feed. `FeedCheckSource`
  connects the native feed checker to the update coordinator.
- `gpui-auto-update sparkle` commands: fetch a checksum-pinned official
  Sparkle distribution (2.10.0 by default), embed `Sparkle.framework` with its
  license notice and the XPC services for the declared sandbox mode, sign
  nested code in order with the hardened runtime (ad-hoc or Developer ID), and
  validate Info.plist metadata, run paths, sandbox requirements, and
  signatures.
- Scheduled workflow that opens an issue when a newer stable Sparkle release
  is available.
- CLI: `gpui-auto-update keys` generates, imports, inspects, and checks
  Sparkle-compatible Ed25519 signing keys (32-byte seed and legacy 96-byte
  formats), integrates with Sparkle's Keychain workflow on macOS, accepts
  private keys only through standard input, environment variables, or files,
  and fails when a key pair does not match. Core marks a published insecure
  test key that release tooling refuses. See `docs/key-management.md`,
  which includes the key rotation procedure.
- GPUI facade: the `Updater` entity with standard actions (`CheckForUpdates`,
  `InstallUpdate`, `RestartToUpdate`, `DismissUpdate`), background execution
  of all checks, preference saves, and backend calls, an `UpdateBackend`
  install/handoff contract with GPUI restart, helper-owned quit, and
  backend-owned relaunch, asynchronous prepare-to-install hooks, an install
  busy guard, a debug-build install guard, deterministic preview states, and
  per-platform default preference file locations.
- macOS backend on Sparkle 2 (`SPUUpdater` through `sparkle-updater`, behind
  the opt-in `sparkle` feature): manual checks use Sparkle's standard UI,
  background checks use Sparkle's background check, the automatic-update
  preference stays in Sparkle and is mirrored, Sparkle errors become
  structured update errors, and Sparkle-driven downloads, installs, and
  relaunches are reflected in the update state. The facade selects it with
  `UpdaterConfig::sparkle` and hands off with `Handoff::BackendOwned`.
  `UpdateBackend::attach` lets backends report state changes made by their
  native engine. See `docs/adr/0002-sparkle-binding.md`.
- Release automation with release-plz, gated on a fresh-consumer build of the
  packaged crates on macOS, Windows, and Linux. Published crates include the
  license texts and document platform backends on their own targets on
  docs.rs.
- CLI: `gpui-auto-update feed native` signs Windows and Linux artifacts into
  one native feed per OS and architecture, with immutable versioned https
  artifact URLs and validation by the updater's own parser; `feed sparkle`
  runs Sparkle's `generate_appcast` (including deltas) from a pinned
  distribution. Both refuse unsigned entries and keys that do not match the
  app's public key, and support explicit key-rotation bridge releases. See
  `docs/feed-generation.md`.
