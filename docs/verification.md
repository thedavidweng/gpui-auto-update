# Verifying a published feed

`gpui-auto-update verify` audits a feed and its artifacts the way a release
pipeline should before publishing, or the way CI should after: it downloads
nothing into an installation, changes nothing, and reports every problem it
finds instead of stopping at the first.

```sh
gpui-auto-update verify \
  --feed https://downloads.example.com/appcast-linux-x86_64.xml \
  --public-key "$APP_PUBLIC_ED_KEY" \
  --os linux --arch x86_64 --expect-version 1.5.0
```

Both feed formats are supported and auto-detected (`--format` overrides):

- **native feeds** (Windows/Linux, [feed-format.md](feed-format.md)) are
  checked with the same fail-closed parser the updater uses, so an unsigned,
  malformed, or duplicate entry is reported as "the updater rejects this
  feed" with core's diagnostic, in addition to the per-entry checks below;
- **Sparkle appcasts** (macOS) are checked structurally: every full and
  delta enclosure must carry a well-formed `sparkle:edSignature`, a positive
  `length`, and (for deltas) a `sparkle:deltaFrom`.

For every enclosure, verify then:

1. downloads the artifact with the updater's transport rules (https only,
   bounded redirects and size; `--allow-http` exists for local testing),
   refusing anything larger than the declared `length`;
2. checks the downloaded bytes against the declared `length` and the
   `sparkle:edSignature` against `--public-key`;
3. checks that the artifact URL contains the item's version
   (`sparkle:version` or `sparkle:shortVersionString`, and `deltaFrom` for
   deltas), because a published artifact behind a mutable URL can be
   silently replaced after signing.

With `--os` and `--arch`, a native feed is additionally checked for entries
covering that platform, and `--expect-version <semver>` requires the highest
applicable release to be exactly that version, so CI can assert that the
feed it just published offers the release it just built.

The exit code is 0 only when there are no problems; diagnostics list every
problem found, one per line, with the offending item and enclosure. `-v`
also lists each artifact as it is fetched. `--max-artifact-bytes` replaces
the 1 GiB default bound, and the insecure, publicly known test key is
refused unless `--allow-test-key` is given.

A feed URL that cannot be fetched, or a local feed file that cannot be read,
fails the command before any per-entry check. The public key is the one the
released application ships; verifying against a different key tells you the
feed was signed with the wrong key, which verify reports as failing
signature checks on every enclosure.
