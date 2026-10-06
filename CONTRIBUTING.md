# Contributing to gpui-auto-update

Thanks for your interest in contributing. This project ships code that
installs other code, so correctness and security matter more than speed.

## Before you start

- Read [CONTEXT.md](CONTEXT.md) for the project vocabulary and
  [docs/adr/](docs/adr/) for recorded design decisions, starting with
  [ADR 0001: workspace layout](docs/adr/0001-workspace-layout.md).
- For anything larger than a small fix, open or comment on an issue first so
  the approach can be agreed before you write code.
- Report security problems privately as described in [SECURITY.md](SECURITY.md).

## Workspace layout

| Crate | Purpose |
| --- | --- |
| `crates/gpui-auto-update-core` | Framework-independent contracts. Must never depend on GPUI. |
| `crates/gpui-auto-update-{macos,windows,linux}` | Platform backends. The only place `unsafe` OS interoperability may live. |
| `crates/gpui-auto-update` | GPUI facade that applications depend on. |
| `crates/gpui-auto-update-ui` | Optional neutral GPUI controls. |
| `crates/gpui-auto-update-cli` | Release and integration CLI (`gpui-auto-update` binary). |
| `apps/reference-app` | Minimal app for end-to-end update validation. |
| `tools/workspace-policy` | Tests that enforce the layering rules. |

## Prerequisites

- The latest stable Rust toolchain. The non-GPUI crates also support Rust 1.85.
- **macOS:** full Xcode (not only the Command Line Tools). GPUI compiles Metal
  shaders at build time. If `xcrun metal --version` fails, run
  `xcodebuild -downloadComponent MetalToolchain`.
- **Linux:** GPUI system libraries, for example on Debian or Ubuntu:

  ```sh
  sudo apt-get install pkg-config libxkbcommon-dev libxkbcommon-x11-dev \
    libwayland-dev libx11-xcb-dev libxcb1-dev libvulkan-dev \
    libfontconfig1-dev libfreetype-dev
  ```

- **Windows:** the MSVC toolchain and Windows SDK.
- Optional: [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) for
  dependency checks.

## Checks to run before opening a pull request

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo deny --locked check
```

CI runs the same checks on macOS, Windows, and Linux. The `Cargo.lock` file is
committed. Use `--locked` and include lockfile changes in your pull request
when you change dependencies.

Platform code you cannot run locally should be `cfg`-gated, with its logic kept
in portable modules that can be unit tested on any host. Pure-Rust crates can be
type-checked for other platforms with
`cargo check --target x86_64-pc-windows-msvc` or
`--target x86_64-unknown-linux-gnu`.

## Releasing

Releases are automated with [release-plz](https://release-plz.dev/), configured
in [release-plz.toml](release-plz.toml):

1. Every push to `main` opens or updates a release pull request that bumps the
   versions of all published crates together and updates the changelogs from
   [Conventional Commits](https://www.conventionalcommits.org/) messages.
2. Review and merge that pull request. Release-plz then publishes the crates
   to crates.io, tags them, and creates the GitHub release.

Publishing waits for the `Package` workflow, which runs
`tools/release/fresh-consumer.sh` on macOS, Windows, and Linux. The script
packages every published crate with `cargo package`, then builds and documents
the packaged sources (with warnings denied) from a new project outside the
workspace, as a crates.io user would. Run it locally before changing crate
manifests or packaging rules:

```sh
tools/release/fresh-consumer.sh            # full `cargo package` verification
tools/release/fresh-consumer.sh --no-verify  # faster; still builds the consumer
```

Each published crate ships copies of `LICENSE-MIT` and `LICENSE-APACHE`. If
you change the root license files, update the copies too; the
`workspace-policy` tests fail if they differ.

## Engineering rules

- **No GPL code.** Waku and Zed are studied only for observable behavior and
  architecture. Do not copy their source into this permissively licensed
  project.
- **Fail closed.** No code path may silently skip signature or length
  verification. Never add a "best effort" verification mode.
- **Never trust feed metadata for paths.** Validate anything from a feed
  before it touches the filesystem.
- **Keep secrets out of logs.** Private keys, tokens, and sensitive paths must
  not appear in logs or user-visible messages.
- **Isolate `unsafe`.** Unsafe code is denied workspace-wide. Platform modules
  in backend crates may opt in locally, with a `// SAFETY:` comment on every
  unsafe block.
- **Keep GPUI in the facade.** Only `gpui-auto-update`, `gpui-auto-update-ui`,
  and the reference app may depend on `gpui`.
- **Test behavior through public APIs.** Tests must not use the production
  internet. Use local HTTP servers, temporary directories, and disposable
  signing keys.
- **Document public APIs.** Missing documentation fails CI.
- Record user-visible changes in [CHANGELOG.md](CHANGELOG.md).

## License

By contributing, you agree that your contributions are dual licensed under
MIT OR Apache-2.0, as described in the [README](README.md#license), without any
additional terms or conditions.
