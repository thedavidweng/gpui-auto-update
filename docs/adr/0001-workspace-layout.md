# ADR 0001: Workspace layout and crate boundaries

- Status: accepted
- Date: 2026-10-05

## Context

The specification defines three layers: a framework-independent core, platform
backends (Sparkle 2 on macOS, native signed updates on Windows and Linux), and a
GPUI adapter that ordinary applications depend on. It also asks for optional
neutral UI controls, a companion CLI that is not needed at runtime, and a small
reference application. GPUI is pre-1.0 and changes often, so its API churn must
stay inside the adapter. Unsafe OS interoperability must be isolated, and the
core and backends should support an older Rust toolchain than GPUI does.

The plain `gpui-updater` crate names are already used by another active
project, so all crate names here use the `gpui-auto-update` prefix.

## Decision

One Cargo workspace with a committed `Cargo.lock`:

| Path | Package | Role | Depends on GPUI |
| --- | --- | --- | --- |
| `crates/gpui-auto-update-core` | `gpui-auto-update-core` | Update state, errors, capability, feed, verification, policy, Windows/Linux orchestration. `#![forbid(unsafe_code)]`. | No |
| `crates/gpui-auto-update-macos` | `gpui-auto-update-macos` | Sparkle 2 backend. | No |
| `crates/gpui-auto-update-windows` | `gpui-auto-update-windows` | Installer and portable handoff backend. | No |
| `crates/gpui-auto-update-linux` | `gpui-auto-update-linux` | Managed-install staging, helper, health confirmation, rollback. | No |
| `crates/gpui-auto-update` | `gpui-auto-update` | GPUI facade: observable entity, actions, executors, backend selection. | Yes |
| `crates/gpui-auto-update-ui` | `gpui-auto-update-ui` | Optional neutral controls. | Yes |
| `crates/gpui-auto-update-cli` | `gpui-auto-update-cli` (binary `gpui-auto-update`) | Init, doctor, keys, feed generation, verify. | No |
| `apps/reference-app` | `gpui-auto-update-reference-app` (unpublished) | End-to-end validation app. | Yes |
| `tools/workspace-policy` | `workspace-policy` (unpublished) | Tests that enforce this ADR. | No |

Rules:

1. **Backends are separate crates**, not modules behind `cfg` inside the core.
   Each backend crate compiles on every target. Its platform code is gated by
   `cfg`, and portable logic stays in modules that can be unit tested on any
   host. The facade depends on each backend only for its target
   (`[target.'cfg(...)'.dependencies]`).
2. **Only the facade, the UI crate, and the reference app may depend on
   `gpui`.** This is enforced by `tools/workspace-policy` tests and by the
   `gpui` wrapper rule in `deny.toml`. The facade re-exports the core as
   `gpui_auto_update::core` so applications need only one dependency.
3. **GPUI is the official `gpui` crate (0.2.2)**, declared once in
   `[workspace.dependencies]` with `default-features = false`. The application,
   not a library, selects GPUI platform features. Unofficial forks such as
   `gpui-pre` are banned.
4. **Unsafe code** is denied workspace-wide through `[workspace.lints]`. Only
   platform modules inside backend crates may opt in with a local
   `#[allow(unsafe_code)]`. The core, facade, UI, and CLI use
   `#![forbid(unsafe_code)]`.
5. **MSRV**: the non-GPUI crates declare `rust-version = "1.85"` (the edition
   2024 floor), which CI checks. The GPUI crates declare no `rust-version`,
   following `gpui` itself, which only supports the latest stable Rust.
6. **The Sparkle binding (`sparkle-updater`) is not a dependency yet.** Its
   build script panics on macOS when `Sparkle.framework` is missing, so it will
   be added behind an opt-in feature of the macOS backend by the macOS backend
   ticket. Default workspace builds and tests must not require the framework.
7. **Where the Linux helper binary lives is not decided here.** Options
   include a binary target in `gpui-auto-update-linux` or a helper mode of the
   host executable. The Linux helper ticket decides and records it in a
   follow-up ADR. (Decided in ADR 0002: a helper mode of the application
   executable.)

## Consequences

- Downstream users add one crate (`gpui-auto-update`); advanced users can use
  the core or a backend directly without pulling in GPUI.
- Platform-specific crates can be type-checked from any host with
  `cargo check --target ...` when they stay pure Rust. GPUI crates can be
  checked only on native CI runners, because GPUI pulls in `ring`.
- Seven crates must be published in dependency order. Release automation is a
  separate ticket.
- Every published crate must carry crates.io metadata (license, description,
  repository, homepage, documentation, readme, keywords, categories). A policy
  test checks this.
