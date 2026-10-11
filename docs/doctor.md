# Project configuration, `init`, and `doctor`

`gpui-auto-update init` and `gpui-auto-update doctor` read the updater
configuration of an application package from its `Cargo.toml`, in the
`[package.metadata.gpui-auto-update]` table. Cargo ignores this table; it
exists only for this tooling. The runtime configuration is still written in
Rust (`UpdaterConfig`); the table records the same decisions so that they can
be checked before a release.

## `init`

```sh
gpui-auto-update init [--manifest-path Cargo.toml] [--write]
```

`init` reports the package name and version, whether the package depends on
`gpui-auto-update`, and what is already configured. Without a configuration
table it prints a skeleton and explains each value. `--write` appends that
skeleton to the manifest. It never overwrites an existing table.

The application identifier, signing key, Windows installer strategy, and
release hosts are the developer's decisions. `init` never chooses them: every
value in the skeleton is a `<...>` placeholder, and `doctor` reports each
placeholder that is still there.

## Configuration reference

```toml
[package.metadata.gpui-auto-update]
app-id = "com.yourcompany.YourApp"   # reverse DNS; on macOS equals CFBundleIdentifier
public-key = "..."                    # SUPublicEDKey from `gpui-auto-update keys generate`

[package.metadata.gpui-auto-update.macos]
feed-url = "https://updates.yourcompany.com/appcast.xml"   # equals SUFeedURL
sandbox = "non-sandboxed"            # or sandboxed, sandboxed-network-client
sparkle-version = "2.10.0"           # optional; the tool's current pin by default
sparkle = "build/sparkle"            # optional; output of `sparkle fetch`
sparkle-archive = "cache/Sparkle-2.10.0.tar.xz"   # optional; a downloaded official archive
app = "dist/YourApp.app"             # optional; a built (unsigned is fine) bundle

[package.metadata.gpui-auto-update.windows]
strategy = "inno-setup"              # or portable, custom
feeds = { x86_64 = "https://updates.yourcompany.com/appcast-windows-x86_64.xml" }
artifacts = { x86_64 = "dist/yourapp-1.2.3-windows-x86_64-setup.exe" }   # optional

[package.metadata.gpui-auto-update.linux]
app-name = "yourapp"                 # <prefix>/bin/<app-name>
feeds = { x86_64 = "https://updates.yourcompany.com/appcast-linux-x86_64.xml" }
artifacts = { x86_64 = "dist/yourapp-1.2.3-linux-x86_64.tar.gz" }        # optional
```

Keep only the platform tables you ship. Relative paths are resolved against
the directory that contains `Cargo.toml`. Unknown keys are errors, so typos
are caught. Architectures are `x86_64` and `aarch64`.

## `doctor`

```sh
gpui-auto-update doctor [--manifest-path Cargo.toml] [--offline] [--allow-http]
                        [--key-env VAR | --key-file PATH] [--verbose]
```

`doctor` publishes nothing and installs nothing. It prints one line per
problem, as `error[area]: ...` or `warning[area]: ...`, then a summary line,
and exits with status 1 if there is any error. `--verbose` also prints each
check that passed (`ok[area]`) and informational notes (`note[area]`).

| Area | Checks |
| --- | --- |
| `project` | The table exists and has no unknown keys; the package version is strict SemVer (inherited workspace versions are resolved); the package depends on `gpui-auto-update`; at least one platform is configured. |
| `app-id` | Set, not a placeholder, reverse-DNS. |
| `public-key` | Set, a valid Ed25519 public key, not the insecure test key. With `--key-env` or `--key-file`, the private key must belong to it (the private key is never printed). |
| `macos` | `feed-url` is an absolute https URL; `sandbox` is a known mode. With `app`, Info.plist must agree with `app-id`, `public-key`, `feed-url`, and the package version, and the unsigned `sparkle validate` checks run (framework placement, run paths, XPC services, license notice). |
| `sparkle` | `sparkle-version` is pinned; `sparkle` is an extracted, pinned distribution that matches it and has `bin/sign_update` and `bin/generate_appcast`; `sparkle-archive` matches the pinned SHA-256. |
| `windows` | `strategy` is `inno-setup`, `portable`, or `custom`; one distinct https feed per architecture; each artifact exists, names the package version, and is a PE image. Portable executables must be built for their configured architecture. |
| `linux` | `app-name` follows the managed-install name rules. Each artifact exists, names the package version, and follows the [managed-install archive layout](linux-managed-install.md#release-archive): a single `<app>-<version>-linux-<arch>` root for the configured architecture, an executable `bin/<app>` (which doubles as the update helper), and the exact ownership marker. |
| `feed` | Unless `--offline`, every feed URL is fetched. Native feeds must pass the updater's fail-closed parser and list entries only for their own OS and architecture; macOS feeds must be Sparkle appcasts. A feed with no release yet is a warning. |
| `ci` | GitHub Actions workflows of the package and its repository: secrets expanded into a `gpui-auto-update` command line, `--allow-test-key`, `--allow-http`, and publishing feeds without running `verify`. |

`doctor` does not download artifacts or check their signatures. Run
[`gpui-auto-update verify`](verification.md) against each published feed for
that.

Before the first release a feed URL usually answers 404, which `doctor`
reports as an error. Publish an empty feed first, or run with `--offline`.
