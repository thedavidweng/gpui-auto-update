# Feed generation

`gpui-auto-update feed` publishes signed update feeds from artifacts that
your packaging pipeline has already produced:

- `feed native` signs one Windows or Linux artifact and adds it to that
  platform's [native feed](feed-format.md).
- `feed sparkle` runs Sparkle's own `generate_appcast` to build the macOS
  appcast, including binary deltas, from a pinned Sparkle distribution.

Both commands sign with the same Ed25519 key pair (see
[key-management.md](key-management.md)), take the private key only through
`--key-stdin`, `--key-env <VAR>`, or `--key-file <path>` (or the Keychain for
`feed sparkle`), and refuse to write a feed that contains an unsigned entry.

## Windows and Linux: `feed native`

Publish one feed per operating system and architecture. Each run adds one
release to one feed:

```sh
gpui-auto-update feed native \
  --os linux --arch x86_64 --version 1.5.0 \
  --artifact dist/example-1.5.0-linux-x86_64.tar.gz \
  --download-url-prefix https://downloads.example.com/releases/1.5.0/ \
  --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY \
  --feed site/appcast-linux-x86_64.xml --output site/appcast-linux-x86_64.xml
```

What the command does:

1. Reads the existing feed (`--feed`, optional) with the same parser the
   updater uses. If any entry is invalid, for example unsigned, or if an entry
   is for another OS or architecture, nothing is written.
2. Builds the artifact URL from `--download-url-prefix` and the artifact's
   file name, or takes `--url`. The URL must use `https` (`--allow-http` exists
   for local testing only) and must contain the version, so that every release
   has its own immutable URL. A version or URL that is already in the feed is
   refused; publish a new version instead of replacing an artifact.
3. Checks that the private key belongs to `--public-key`, the key the
   released application trusts. The insecure test key is refused unless
   `--allow-test-key` is given.
4. Signs the artifact bytes (pure Ed25519, Sparkle's `sparkle:edSignature`
   encoding), verifies the new signature, and records the exact length.
5. Writes all entries, newest first, after checking that the updater would
   accept the complete document. The file is replaced atomically.

`--os` and `--arch` are required and are never inferred from file names.
`--arch` accepts only `x86_64` and `aarch64`. Optional release metadata:
`--title`, `--display-version`, `--pub-date` (RFC 822; the current time by
default), `--channel`, `--minimum-system-version`, `--critical` or
`--critical-below <version>`, `--release-notes-url`,
`--full-release-notes-url`, `--description-file`, `--type` (MIME type,
derived from the file extension by default), and `--feed-title`.

Upload the artifact to its versioned URL before you publish the updated feed.

## macOS: `feed sparkle`

macOS appcasts are produced by Sparkle's `generate_appcast`, not by this
tool. `feed sparkle` runs it from a distribution fetched with
`gpui-auto-update sparkle fetch` and checks the result:

```sh
gpui-auto-update sparkle fetch --out build/sparkle
gpui-auto-update feed sparkle \
  --sparkle build/sparkle --archives dist/macos \
  --public-key "$APP_PUBLIC_ED_KEY" --key-env SPARKLE_PRIVATE_KEY \
  --download-url-prefix https://downloads.example.com/mac/ \
  --output site/appcast.xml
```

- The distribution must contain a pinned Sparkle release (see
  `gpui-auto-update sparkle versions`) and its `bin/generate_appcast`.
- The private key is checked against `--public-key` before Sparkle runs, then
  handed to `generate_appcast --ed-key-file -` on standard input. With
  `--keychain [--account <name>]`, `generate_appcast` uses Sparkle's Keychain
  item directly.
- `generate_appcast` creates delta updates between the archives in
  `--archives` and writes them there; `--maximum-deltas` and
  `--delta-compression` are passed through. Other passed-through options are
  listed in `gpui-auto-update feed --help`.
- The appcast is generated next to `--output` (starting from the current
  `--output`, so existing entries are kept) and replaces it only if every
  enclosure, full or delta, carries a well-formed `sparkle:edSignature` and
  every enclosure whose file is in `--archives` (or its `old_updates`
  directory) verifies against the signing key. Otherwise the published
  appcast is left untouched.

Upload the archives and deltas before the appcast.

## Key-rotation bridge releases

A bridge release (see [Rotating the signing key](key-management.md#rotating-the-signing-key))
trusts the new public key but must be signed with the previous one. Ordinary
runs refuse that, because the private key does not match `--public-key`.
Declare the rotation explicitly:

```sh
gpui-auto-update feed native ... \
  --public-key "$NEW_PUBLIC_KEY" \
  --bridge-from-public-key "$OLD_PUBLIC_KEY" --key-env SPARKLE_PRIVATE_KEY_OLD \
  --feed site/v1/appcast-linux-x86_64.xml --output site/v1/appcast-linux-x86_64.xml
```

With `--bridge-from-public-key`, the private key must match the previous key,
and the previous key must differ from `--public-key`. The same option works
for `feed sparkle`. Publish the result only in the old feeds. In the new feeds,
list the bridge release with a signature made by the new key, using an
ordinary run with the new key.

## Interoperability

The same key pair and signature encoding work across all backends. The CLI
tests use one shared vector, the RFC 8032 test key signing a fixed artifact,
whose signature was computed independently with OpenSSL. The native feed
carries exactly that signature, core verification accepts it, and an ignored
test (`cargo test -p gpui-auto-update-cli --test feed -- --ignored`, macOS,
downloads Sparkle) checks that Sparkle's `sign_update` produces and verifies
the same signature and that every enclosure written by the real
`generate_appcast` verifies in core.
