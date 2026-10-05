# gpui-auto-update

**Sparkle-quality automatic updates for GPUI.**

**Sparkle on macOS. Signed native updates on Windows and Linux. One observable GPUI API.**

[![CI](https://github.com/thedavidweng/gpui-auto-update/actions/workflows/ci.yml/badge.svg)](https://github.com/thedavidweng/gpui-auto-update/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

> **Status: pre-release, under active development.** The crates in this
> repository are placeholders and do not perform updates yet. The sections
> below describe the intended product and will be completed as each part lands.

`gpui-auto-update` aims to give GPUI applications the complete native updater
lifecycle, GPUI integration, release tooling, and Waku-class cross-platform
behavior: Sparkle 2 handles updates on macOS, signed native update flows handle
Windows and Linux, and your application sees one observable GPUI entity with
standard actions.

> **Security warning:** shipping an updater without artifact signature
> verification is not a supported production configuration. Verification is
> fail-closed by default.

## Supported platforms and features

_To be completed._

## Quick start

_To be completed._

## What you get by default

_To be completed._

## GPUI state and UI integration

_To be completed._

## macOS (Sparkle) setup

_To be completed._

## Windows setup

_To be completed._

## Linux managed-install setup

_To be completed._

## Automatic-update policy

_To be completed._

## Release and signing workflow

_To be completed._

## Hosting feeds and artifacts

_To be completed._

## Package-manager behavior

_To be completed._

## Security model

_To be completed._ See [SECURITY.md](SECURITY.md) for how to report a
vulnerability.

## CLI tooling

_To be completed._

## Compatibility policy

| gpui-auto-update | gpui | Rust (core, backends, CLI) | Rust (GPUI crates) |
| --- | --- | --- | --- |
| unreleased | 0.2.2 (official crate) | 1.85+ | latest stable |

Known issue: on Linux, gpui 0.2.2 does not build with `libc` 0.2.190 or newer
(its `xattr` 0.2 dependency uses a removed constant). Pin it in your
application with `cargo update -p libc --precise 0.2.189`.

## Troubleshooting

_To be completed._

## Architecture for contributors

The workspace layout is described in
[ADR 0001](docs/adr/0001-workspace-layout.md) and the domain vocabulary in
[CONTEXT.md](CONTEXT.md). See [CONTRIBUTING.md](CONTRIBUTING.md) to build and
test locally.

## Acknowledgements / prior art

_To be expanded._ This project builds on ideas and work from
[Sparkle](https://github.com/sparkle-project/Sparkle),
[Waku](https://github.com/egoist/waku) (behavioral and architectural prior
art only; no GPL-licensed Waku source is copied),
[hankbao/sparkle-updater](https://github.com/hankbao/sparkle-updater),
[ahonn/tauri-plugin-sparkle-updater](https://github.com/ahonn/tauri-plugin-sparkle-updater),
[AprilNEA/gpui-updater](https://github.com/AprilNEA/gpui-updater),
[Zed / GPUI](https://github.com/zed-industries/zed), and
[Liora](https://github.com/yhyzgn/liora). These acknowledgements do not imply
endorsement by any upstream project.

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
