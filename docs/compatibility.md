# Compatibility

GPUI is pre-1.0 and may break its API often. This project keeps that churn
inside the GPUI adapter and records what is supported in one place.

## Matrix

| gpui-auto-update | gpui | Rust (core, backends, CLI) | Rust (GPUI crates) | Sparkle (macOS) |
| --- | --- | --- | --- | --- |
| unreleased (0.x) | 0.2.2, the official `gpui` crate on crates.io | 1.85+ | latest stable | 2.10.0 by default; 2.9.6 also pinned |

Notes:

- **GPUI.** The facade and UI crates depend on the official `gpui` crate, not
  on a fork, and declare it with `default-features = false`. Your application
  chooses GPUI's platform features. Unofficial forks such as `gpui-pre` are
  banned in this repository.
- **Rust.** `gpui-auto-update-core`, the three backends, and the CLI declare
  `rust-version = "1.85"` and CI checks it. The facade and UI crates declare no
  `rust-version`, because `gpui` itself supports only the latest stable Rust.
- **Sparkle.** The CLI pins official Sparkle archives by SHA-256 in
  [`sparkle-pins.json`](../crates/gpui-auto-update-cli/sparkle-pins.json).
  2.10.0 requires macOS 12.0 or later (`LSMinimumSystemVersion`); 2.9.6, the
  version the `sparkle-updater` binding was developed against, supports 10.13.
  See the [Sparkle version policy](sparkle-packaging.md#sparkle-version-policy).
- **Architectures.** Windows and Linux feeds exist for `x86_64` and `aarch64`.
- **Versioning.** The project follows semantic versioning for its own public
  API. Until 1.0, breaking changes are possible in any minor release and are
  listed in the [migration notes](migration.md).

Each release of gpui-auto-update adds a row. When it needs a different GPUI
release, the row says so, and the previous row stays as history.

## Known issues

- **Linux: `libc` 0.2.190 or newer.** gpui 0.2.2 does not build on Linux with
  `libc` 0.2.190 or newer, because its `xattr` 0.2 dependency uses a constant
  that was removed. Pin it in your application:
  `cargo update -p libc --precise 0.2.189`.

## Using a different GPUI revision

Applications sometimes need a GPUI Git revision that is not on crates.io. That
can work if the revision is API-compatible with the GPUI release in the matrix.
This project is built and tested only against crates.io `gpui` 0.2.2.

Patch GPUI **at the workspace root of your application**, so that every crate in
the dependency graph, including `gpui-auto-update`, resolves the same GPUI:

```toml
# Cargo.toml of the application's workspace root
[patch.crates-io]
gpui = { git = "https://github.com/zed-industries/zed", rev = "<commit>" }
```

### Do not load two GPUI package identities

Cargo identifies a package by name, version, **and source**. A `gpui` from
crates.io and a `gpui` from a Git URL are two different packages, even at the
same version, and Cargo will happily build both. Types from one are not types
from the other, so the failure looks like this:

- compile errors such as "expected `Entity<Updater>`, found `Entity<Updater>`" or
  "the trait `Render` is not implemented", where both sides print identically;
- or, if the types happen to line up, two copies of GPUI's globals and
  executors in one process, which breaks in ways that do not point at the
  cause.

It happens when:

- your application depends on `gpui = { git = "..." }` directly while
  `gpui-auto-update` (and `gpui-auto-update-ui`) still resolve `gpui` from
  crates.io. A direct Git dependency does not change what other crates use;
  `[patch]` does.
- a `[patch]` applies in one workspace but not another that also builds the
  application, such as a bundler or a test crate with its own `Cargo.lock`.
- the patched revision's own dependencies (for example `gpui_macros` and
  other crates from the same repository) are also depended on from crates.io
  by some other crate, so two copies of those appear.

Check before shipping:

```sh
cargo tree -i gpui            # exactly one gpui, with the source you expect
cargo tree -d | grep -i gpui  # no duplicated gpui* packages
```

The first command should list a single `gpui` entry, and its source should be
the Git URL you patched in. If you see two, make the application depend on
`gpui` the same way everywhere (a plain `gpui = "0.2"` plus the root `[patch]`)
instead of mixing sources.
