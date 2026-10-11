# Hosting recipes

`gpui-auto-update` does not require any particular host. Any HTTPS-capable
static origin or CDN works, as long as it serves two kinds of files with
different rules:

| Kind | Examples | URL | `Cache-Control` | Overwrite |
| --- | --- | --- | --- | --- |
| Versioned artifacts | `MyApp-1.5.0-macos-aarch64.zip`, deltas, installers, tarballs | Contains the version, never reused | `public, max-age=31536000, immutable` | Never |
| Mutable feeds | `appcast.xml`, `appcast-<os>-<arch>.xml`, channel feeds | Stable, configured in the application | `public, max-age=60` (at most a few minutes) | Only by the release workflow, last |

Three rules follow from the [publish-order contract](../../release/README.md#the-publish-order-contract):

1. Upload every artifact before any feed that references it.
2. Never overwrite an existing versioned artifact. A broken release is fixed
   by shipping a new version, not by replacing bytes under an old URL:
   clients may already have cached the old bytes, and the signature in the
   feed would no longer match.
3. Run `gpui-auto-update verify` against the staged feeds after the
   artifacts are uploaded and before the feeds are published.

Recipes:

- [GitHub Releases plus a stable feed location](github-releases.md)
- [Cloudflare R2](cloudflare-r2.md)
- [S3-compatible object storage](s3-compatible.md) (AWS S3, MinIO,
  Backblaze B2, DigitalOcean Spaces, and similar)
- [Generic static hosting](static-hosting.md) (any web server, rsync, or
  static-site host)

Each recipe replaces the upload steps of the `publish-artifacts` and
`publish-feeds` jobs in
[`release/github-actions/release.yml`](../../release/github-actions/release.yml).
Keep the rest of the job graph as it is.
