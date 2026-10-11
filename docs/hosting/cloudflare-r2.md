# Cloudflare R2

R2 speaks the S3 API, so the [S3-compatible recipe](s3-compatible.md) applies
with an R2 endpoint. This page lists only what differs.

## Bucket and domain

1. Create a bucket, for example `myapp-downloads`.
2. Connect a custom domain to it (*R2 → bucket → Settings → Custom
   Domains*), for example `downloads.example.com`. Do not rely on the
   `r2.dev` development URL in production: it is rate limited and not
   cached.
3. Set the repository variable `DOWNLOADS_BASE_URL` to
   `https://downloads.example.com`.

## Credentials

Create an R2 API token with **Object Read & Write** permission scoped to
this bucket only. Store its access key ID and secret access key as the
repository secrets `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, and add
the account endpoint to the upload steps' `env`:

```yaml
env:
  AWS_ACCESS_KEY_ID: ${{ secrets.AWS_ACCESS_KEY_ID }}
  AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS_SECRET_ACCESS_KEY }}
  AWS_DEFAULT_REGION: auto
  AWS_ENDPOINT_URL: https://<account-id>.r2.cloudflarestorage.com
```

The account ID is not a secret, but keep it in a repository variable if you
prefer.

## Upload the immutable artifacts

R2 supports conditional writes, so use the `If-None-Match: *` loop from the
S3 recipe: an existing key fails with `412 Precondition Failed`, and the
release stops instead of silently overwriting a published version. Never
overwrite an object under `releases/`; ship a new version instead.

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

The template's HTTPS pre-check (refuse if the URL already serves content)
still runs first and catches the common case early.

## Publish the mutable feeds

```sh
for feed in site/*.xml; do
  aws s3 cp "$feed" "s3://$FEED_BUCKET/$(basename "$feed")" \
    --content-type "application/xml" \
    --cache-control "public, max-age=60"
done
```

## Cloudflare cache

Objects served through a custom domain pass through Cloudflare's cache,
which respects the stored `Cache-Control` header. Two settings matter:

- Do not add a Cache Rule that raises the edge TTL for `*.xml`; that would
  override the short feed lifetime. If you do cache feeds at the edge,
  purge the feed URLs (`/appcast*.xml`) after `publish-feeds`.
- Artifacts under `/releases/` can be cached at the edge indefinitely; they
  are `immutable` and never overwritten.
