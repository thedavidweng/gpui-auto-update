# CI and release recipes

This page is the map. It ties together the pieces that already exist and says
which one to reach for. Nothing here is specific to GitHub Actions except the
templates, and the commands they run work from any CI system.

## What your pipeline has to do

Releasing an application that uses `gpui-auto-update` is four separate jobs:

| Job | Tooling | Details |
| --- | --- | --- |
| **Package** the app per platform | Your bundler, plus `gpui-auto-update sparkle embed\|sign\|validate` on macOS | [Sparkle packaging](sparkle-packaging.md), [Windows installers](windows-installers.md), [Linux managed install](linux-managed-install.md) |
| **Sign** artifacts and generate feeds | `gpui-auto-update feed native`, `feed sparkle`, `keys check` | [Feed generation](feed-generation.md), [key management](key-management.md) |
| **Publish** artifacts, then feeds | Your host's upload tool | [Hosting recipes](hosting/README.md) |
| **Verify** what is live | `gpui-auto-update verify` | [Verification](verification.md) |

## The publish-order contract

1. Build, package, and sign every platform artifact.
2. Generate signed feeds that point at the artifacts' final, versioned URLs.
   Do not publish them yet.
3. Upload the immutable artifacts. Refuse to overwrite an existing version.
4. Run `gpui-auto-update verify` against the staged feeds and live artifact
   URLs, with `--expect-version` set to the release being shipped.
5. Publish the mutable feeds last, with short cache lifetimes.

Feeds are the only thing that may be overwritten, and only after steps 1 to 4
succeeded. The reasoning and the template's job graph are in
[release/README.md](../release/README.md#the-publish-order-contract).

## Reusable material for applications

| File | Purpose |
| --- | --- |
| [`release/github-actions/release.yml`](../release/github-actions/release.yml) | Complete release workflow template: version resolution, build matrix, feed generation, artifact upload, verification, feed publication, GitHub release. Contains `TODO(app)` markers to fill in. |
| [`release/github-actions/verify-feeds.yml`](../release/github-actions/verify-feeds.yml) | Reusable `workflow_call` workflow that audits feeds and artifacts with `gpui-auto-update verify`. Needs no secrets, and is useful on its own as a scheduled post-release audit. |
| [`release/README.md`](../release/README.md) | Adoption guide for both: secrets, variables, triggers, linting. |
| [`docs/hosting/`](hosting/README.md) | Upload commands and exact `Cache-Control` headers for GitHub Releases plus a stable feed location, Cloudflare R2, S3-compatible storage, and generic static hosting. |

## A pre-release gate on pull requests

`doctor` publishes nothing and installs nothing, so it is safe to run on every
pull request. Pass the private key only if the job can read the secret (never
for forks):

```yaml
- name: Install the CLI
  run: cargo install --locked --git https://github.com/thedavidweng/gpui-auto-update gpui-auto-update-cli
- name: Check the updater integration
  run: gpui-auto-update doctor --offline
```

`--offline` skips fetching published feeds, which do not exist before the first
release. See [`doctor`](doctor.md) for the checks, including the CI-specific
ones that flag secrets on command lines, `--allow-test-key`, `--allow-http`,
and feed publication without `verify`. Pin the CLI to a released version once
one exists.

## Secrets and variables

- `SPARKLE_PRIVATE_KEY` (secret): the base64 Ed25519 private key. It is read
  only by the signing and feed-generation steps, through `env` and
  `--key-env`, never on a command line. Forks and pull requests must not see it.
- `APP_PUBLIC_ED_KEY` (variable): the public key the application trusts. It is
  not a secret.
- `DOWNLOADS_BASE_URL` (variable): the public HTTPS base URL of your download
  site.
- Apple signing and notarization credentials and Windows Authenticode
  credentials belong in your CI's secret store and are outside this project's
  tooling. Never write them to a file in the workspace.

Setup details and the Keychain alternative are in
[key management](key-management.md#ci-setup).

## This repository's own workflows

These exist for the project itself and are not templates:

| Workflow | Purpose |
| --- | --- |
| [`ci.yml`](../.github/workflows/ci.yml) | rustfmt; clippy, tests (including doctests), and `cargo doc` with warnings denied on macOS, Windows, and Linux; the Sparkle backend against the real framework; the Linux update and rollback end-to-end; MSRV; `cargo-deny`. |
| [`windows-e2e.yml`](../.github/workflows/windows-e2e.yml) | Reference-app update from 1.0.0 to 1.1.0 on Windows x86_64 and ARM64, for the Inno Setup and portable flows. |
| [`package.yml`](../.github/workflows/package.yml) | Builds the packaged `.crate` files from a fresh project outside the workspace. |
| [`release-plz.yml`](../.github/workflows/release-plz.yml) | Publishes this project's crates to crates.io with release-plz. |
| [`sparkle-release-check.yml`](../.github/workflows/sparkle-release-check.yml) | Daily check for a newer stable Sparkle release. |

The end-to-end harnesses can also be run locally; see the
[reference application README](../apps/reference-app/README.md).
