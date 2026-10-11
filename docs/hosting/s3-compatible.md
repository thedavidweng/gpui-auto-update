# S3-compatible object storage

This is the form the release template uses by default. It works with AWS S3
and any service that speaks the S3 API (MinIO, Backblaze B2, DigitalOcean
Spaces, Wasabi, and [Cloudflare R2](cloudflare-r2.md)).

## Layout

```text
s3://myapp-downloads/
  appcast.xml                          mutable feed (macOS)
  appcast-windows-x86_64.xml           mutable feed
  appcast-linux-x86_64.xml             mutable feed
  releases/1.5.0/MyApp-1.5.0-...       immutable artifacts
```

Serve the bucket over HTTPS (directly, or through a CDN) and set the
repository variable `DOWNLOADS_BASE_URL` to its public base URL.

## Credentials

Create an access key that can only `PutObject` and `GetObject` on this
bucket (no `DeleteObject`, no bucket administration) and store it as the
repository secrets `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`. The
template passes them to the upload steps through `env` only. For a non-AWS
endpoint, also set `AWS_ENDPOINT_URL` in the same `env` block.

On AWS itself, prefer OpenID Connect over long-lived keys: give the job
`permissions: id-token: write` and use `aws-actions/configure-aws-credentials`
with a role restricted to this bucket.

## Upload the immutable artifacts

The template first checks over HTTPS that no artifact URL already serves
content, and refuses to continue otherwise. On S3 you can make that refusal
atomic as well, with a conditional write per object: `If-None-Match: *`
fails with `412 Precondition Failed` when the key already exists, so a
concurrent or repeated upload can never overwrite an existing version.

```sh
for artifact in dist/*; do
  aws s3api put-object \
    --bucket "$ARTIFACT_BUCKET" \
    --key "releases/$VERSION/$(basename "$artifact")" \
    --body "$artifact" \
    --cache-control "public, max-age=31536000, immutable" \
    --if-none-match '*'
done
```

This is the loop the template uses. `--if-none-match` needs AWS CLI 2.22 or
later. If your provider does not support conditional writes, drop the flag,
rely on the template's HTTPS pre-check, and enable bucket versioning or
Object Lock on the `releases/` prefix so that an accidental overwrite stays
recoverable.

`Cache-Control: public, max-age=31536000, immutable` is safe because the
bytes under a versioned URL never change.

## Publish the mutable feeds

Feeds are the only objects that are overwritten, and only after the
`verify-published` job has passed.

```sh
for feed in site/*.xml; do
  aws s3 cp "$feed" "s3://$FEED_BUCKET/$(basename "$feed")" \
    --content-type "application/xml" \
    --cache-control "public, max-age=60"
done
```

Keep `Cache-Control` short (`max-age=60`, or `no-cache` to force
revalidation) so that clients see a new release within minutes, and so that
a feed rollback takes effect quickly. If a CDN sits in front of the bucket,
make sure it honours the origin's `Cache-Control` for `*.xml`, or purge the
feed URLs after publishing.
