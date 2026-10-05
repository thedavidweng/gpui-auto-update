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
- Release automation with release-plz, gated on a fresh-consumer build of the
  packaged crates on macOS, Windows, and Linux. Published crates include the
  license texts and document platform backends on their own targets on
  docs.rs.
