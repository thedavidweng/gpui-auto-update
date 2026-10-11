# Migration and breaking changes

## Where changes are recorded

All notable changes, for every crate in the project, are listed in
[CHANGELOG.md](../CHANGELOG.md), in
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format. release-plz
maintains it from the release process. Breaking changes are listed there under
the release that introduces them and, when they need explanation, below.

## Policy

- The project follows [semantic versioning](https://semver.org/) for its public
  Rust API. release-plz runs `cargo-semver-checks` on release candidates.
- **Before 1.0**, a minor version (`0.x` to `0.(x+1)`) may contain breaking
  changes. Patch versions do not.
- **The crates are versioned together.** The facade, core, backends, UI, and CLI
  share one version and are released as a group, so use matching versions of
  every `gpui-auto-update*` crate you depend on.
- `UpdateState`, `UpdateEvent`, `UpdaterEvent`, `ErrorKind`, `Capability`,
  `Handoff`, and `PreviewState` are `#[non_exhaustive]`: new variants are not
  breaking changes, so match with a wildcard arm.
- **Feed and marker formats are versioned independently of the crates.** The
  native feed follows Sparkle's field conventions; the Linux marker carries a
  contract version (`gpui-auto-update managed-install 1`) and older updaters
  reject a newer one. Installed copies cannot be updated by a
  release they cannot read, so a change to either format that older copies
  cannot parse will be called out here and in the changelog.
- **A GPUI upgrade is a compatibility-matrix change**, recorded in
  [compatibility.md](compatibility.md) and in the changelog. If it changes a
  signature that your code touches, it is a breaking change of this crate too.

## Upgrading

1. Read the changelog entries between your version and the target.
2. Update every `gpui-auto-update*` crate and the CLI to the same version. Pin
   the CLI in CI (`CLI_VERSION` in the [release templates](release-ci.md)).
3. Run `gpui-auto-update doctor`. It reports configuration that the new version
   no longer accepts.
4. Build with your usual warnings and fix what the compiler reports.
5. Before shipping, run an old-to-new update in your own pipeline. The
   [reference application](../apps/reference-app/README.md) shows the
   scenario.

## Notes for specific versions

No released version exists yet, so there is nothing to migrate from. This
section will list, per release, each breaking change with the code before and
after.
