# Documentation

The [project README](../README.md) is the short version. These pages hold the
detail. API reference is on docs.rs, starting with the
[`gpui-auto-update`](https://docs.rs/gpui-auto-update) crate.

## Using the updater

| Page | Read it to |
| --- | --- |
| [Update state machine](state-machine.md) | Render update UI: every state, transition, and handoff. |
| [Package-manager ownership](package-managers.md) | Understand when the updater steps aside, and how to declare ownership yourself. |
| [Compatibility](compatibility.md) | Check supported GPUI, Rust, and Sparkle versions; use a patched GPUI safely. |
| [Troubleshooting](troubleshooting.md) | Diagnose a failing check or update, including how to get Sparkle logs. |
| [Migration and breaking changes](migration.md) | Upgrade between versions. |

## Packaging per platform

| Page | Read it to |
| --- | --- |
| [Sparkle packaging (macOS)](sparkle-packaging.md) | Embed, sign, and validate `Sparkle.framework` in your app bundle. |
| [Windows installers](windows-installers.md) | Choose and configure Inno Setup, portable, or a custom installer strategy. |
| [Linux managed install](linux-managed-install.md) | Lay out and package a release the updater can own, and understand the helper and rollback. |

## Releasing

| Page | Read it to |
| --- | --- |
| [Key management](key-management.md) | Create, store, check, and rotate the signing key. |
| [Feed format](feed-format.md) | Know exactly what the native feed contains and how releases are selected. |
| [Feed generation](feed-generation.md) | Produce signed feeds with `feed native` and `feed sparkle`. |
| [Verification](verification.md) | Audit published feeds and artifacts. |
| [`init` and `doctor`](doctor.md) | Check an integration before release. |
| [CI and release recipes](release-ci.md) | Wire packaging, signing, publishing, and verification into CI. |
| [Hosting recipes](hosting/README.md) | Publish to GitHub Releases, Cloudflare R2, S3-compatible storage, or static hosting. |

## Design

| Page | Read it to |
| --- | --- |
| [Security and trust model](security.md) | See what is trusted, what is checked, and what is not protected. |
| [Architecture](architecture.md) | See how the crates and backends fit together. |
| [ADR 0001: workspace layout](adr/0001-workspace-layout.md) | Crate boundaries and rules. |
| [ADR 0002: Sparkle binding](adr/0002-sparkle-binding.md) | How the macOS backend uses Sparkle, and the upstream gaps. |
| [ADR 0003: Linux update helper](adr/0003-linux-update-helper.md) | Why the helper is a mode of the application executable. |
| [CONTEXT.md](../CONTEXT.md) | The shared vocabulary. |
