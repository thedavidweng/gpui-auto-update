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
- `gpui-auto-update sparkle` commands: fetch a checksum-pinned official
  Sparkle distribution (2.10.0 by default), embed `Sparkle.framework` with its
  license notice and the XPC services for the declared sandbox mode, sign
  nested code in order with the hardened runtime (ad-hoc or Developer ID), and
  validate Info.plist metadata, run paths, sandbox requirements, and
  signatures.
- Scheduled workflow that opens an issue when a newer stable Sparkle release
  is available.
