# gpui-auto-update-cli

Release and integration tooling for [gpui-auto-update](https://crates.io/crates/gpui-auto-update): project initialization, doctor checks, signing keys, feed generation, and release verification. Installs the `gpui-auto-update` binary; not needed at application runtime.

> Pre-release: the `sparkle`, `keys`, and `feed` commands are implemented so far. See the
> [project README](https://github.com/thedavidweng/gpui-auto-update) for
> status.

## Sparkle packaging (macOS)

```sh
gpui-auto-update sparkle versions                       # pinned releases and checksums
gpui-auto-update sparkle fetch --out build/sparkle      # download + verify the default pin
gpui-auto-update sparkle embed --app MyApp.app --sparkle build/sparkle --sandbox non-sandboxed
gpui-auto-update sparkle sign --app MyApp.app --identity "Developer ID Application: Example (TEAMID)"
gpui-auto-update sparkle validate --app MyApp.app --require-developer-id
```

See [docs/sparkle-packaging.md](https://github.com/thedavidweng/gpui-auto-update/blob/main/docs/sparkle-packaging.md)
for what each step does and for the Sparkle version policy.

## Signed feeds

```sh
# Windows/Linux: sign an artifact and add it to that platform's native feed
gpui-auto-update feed native --os linux --arch x86_64 --version 1.5.0 \
  --artifact dist/example-1.5.0-linux-x86_64.tar.gz \
  --download-url-prefix https://downloads.example.com/releases/1.5.0/ \
  --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY \
  --feed site/appcast-linux-x86_64.xml --output site/appcast-linux-x86_64.xml

# macOS: run Sparkle's generate_appcast (with deltas) and check every entry is signed
gpui-auto-update feed sparkle --sparkle build/sparkle --archives dist/macos \
  --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY \
  --download-url-prefix https://downloads.example.com/mac/ --output site/appcast.xml
```

See [docs/feed-generation.md](https://github.com/thedavidweng/gpui-auto-update/blob/main/docs/feed-generation.md),
including key-rotation bridge releases.

Licensed under either of MIT or Apache-2.0 at your option.
