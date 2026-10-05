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
- CLI: `gpui-auto-update keys` generates, imports, inspects, and checks
  Sparkle-compatible Ed25519 signing keys (32-byte seed and legacy 96-byte
  formats), integrates with Sparkle's Keychain workflow on macOS, accepts
  private keys only through standard input, environment variables, or files,
  and fails when a key pair does not match. Core marks a published insecure
  test key that release tooling refuses. See `docs/key-management.md`,
  which includes the key rotation procedure.
