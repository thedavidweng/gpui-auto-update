# gpui-auto-update-cli

Release and integration tooling for [gpui-auto-update](https://crates.io/crates/gpui-auto-update): project initialization, doctor checks, signing keys, feed generation, and release verification. Installs the `gpui-auto-update` binary; not needed at application runtime.

> Pre-release: only the `sparkle` commands are implemented so far. See the
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

Licensed under either of MIT or Apache-2.0 at your option.
