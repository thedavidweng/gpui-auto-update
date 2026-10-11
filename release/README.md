# Release CI for consuming applications

This directory holds reusable CI material for **applications that ship with
`gpui-auto-update`**. It is not how this repository releases itself — the
project's own crates are published by release-plz
(`.github/workflows/release-plz.yml`, `tools/release/`), and nothing here
duplicates that.

What you get:

- `github-actions/release.yml` — a complete, copy-paste-ready release
  workflow template implementing the release phases described below:
  triggered by a version tag or manually (`workflow_dispatch`), with
  publish-order safety built into the job graph.
- `github-actions/verify-feeds.yml` — a reusable (`workflow_call`) workflow
  that audits feeds and artifacts with `gpui-auto-update verify`. It needs no
  secrets (verification uses only the public key) and is also useful on its
  own as a scheduled post-release audit.
- [Hosting recipes](../docs/hosting/README.md) for GitHub Releases + a stable feed
  location, Cloudflare R2, S3-compatible object storage, and generic static
  hosting. The upload steps in the template are the S3-compatible form; each
  recipe shows the equivalent commands and the exact cache headers for its
  host.

## The publish-order contract

The template enforces one order, and the repository's policy tests enforce it
on the template itself:

1. **Build, package, and sign** every platform artifact (`build`).
2. **Generate signed feeds** from those artifacts (`feeds`, `feeds-macos`),
   checking the private key against the public key first. The feeds reference
   the artifacts' final, versioned URLs but are not published yet.
3. **Upload the immutable artifacts** (`publish-artifacts`). Every artifact
   lives under a versioned URL; the upload refuses to continue if any of
   those URLs already serves content, so an existing version is never
   silently overwritten.
4. **Verify every feed against the live artifact URLs**
   (`verify-published`, via `verify-feeds.yml`). `gpui-auto-update verify`
   re-downloads each artifact, checks length and Ed25519 signature, and
   requires the feed's top release to be exactly the version being shipped.
   A failed verification stops the release *after* the immutable artifacts
   are up (harmless, orphaned, never referenced) but *before* any feed moves.
5. **Publish the mutable feeds last** (`publish-feeds`), with short cache
   lifetimes, so no client can be offered an artifact that is not yet
   downloadable.
6. **Create the human-facing GitHub release** (`github-release`) if desired.

Feeds are mutable documents, so they are the only step that may overwrite
existing content — and only after steps 1–4 have succeeded for the new
release.

## Adopting the template

1. Copy both files from `github-actions/` into your application's
   `.github/workflows/`. The template calls the reusable workflow as
   `./.github/workflows/verify-feeds.yml`; keep them together or adjust the
   `uses:` path.
2. Work through the `TODO(app)` markers: application name, bundle/packaging
   commands, code signing and notarization, and the Windows/Linux/macOS
   matrix entries you actually ship.
3. Pick one [hosting recipe](../docs/hosting/README.md) and replace the
   S3-compatible upload steps in `publish-artifacts` and `publish-feeds`
   with that recipe's commands. Keep the recipe's cache headers: they are
   part of the safety contract.
4. Set configuration:
   - **Secret** `SPARKLE_PRIVATE_KEY`: the base64 Ed25519 private key (see
     [key management](../docs/key-management.md#ci-setup)). Only the
     feed-generation jobs read it, only through `env`, never on a command
     line. Pull requests cannot trigger these workflows, so forks never see
     the secret.
   - **Variable** `DOWNLOADS_BASE_URL`: the public HTTPS base URL of your
     download site, for example `https://downloads.example.com/myapp`.
     Artifacts publish to `$DOWNLOADS_BASE_URL/releases/<version>/` and feeds
     to `$DOWNLOADS_BASE_URL/appcast-<os>-<arch>.xml` (`appcast.xml` for
     macOS).
   - **Variable** `APP_PUBLIC_ED_KEY` (or read it from your app's
     configuration in the `feeds` jobs): the public key the released
     application trusts. It is not a secret.
5. Pin `CLI_VERSION` at the top of both workflows. The template installs the
   CLI from crates.io with `--locked`; the comment shows the `--git` pin to
   use before your pinned version is published.

## Triggering a release

- **Tag**: push `v1.5.0`. The tag must be a `v` followed by the semantic
  version; the workflow resolves the version from the tag name.
- **Manual**: *Actions → Release → Run workflow*, entering the version
  explicitly. Both paths converge on one authoritative version in the
  `version` job, and every later job reads it from there.

A `concurrency` group with `cancel-in-progress: false` serializes releases:
a queued release is never cancelled by a newer one, so two runs can never
race an artifact upload past the no-overwrite guard.

## Verifying the templates after editing

The safety invariants of both templates are checked by `cargo test -p
workspace-policy --test release_templates` in this repository. After
adapting them, lint them in your own repository with
[actionlint](https://github.com/rhysd/actionlint):
`actionlint .github/workflows/release.yml .github/workflows/verify-feeds.yml`.
