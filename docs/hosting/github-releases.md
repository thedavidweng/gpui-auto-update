# GitHub Releases plus a stable feed location

GitHub Releases are a convenient home for the versioned artifacts: release
asset URLs contain the tag, and GitHub refuses to upload an asset whose name
already exists unless you pass `--clobber`. They are not a good home for the
feeds, because the application must poll one stable URL, and the
`releases/latest/download/...` redirect changes as soon as a release is
marked latest, which is not necessarily after its feeds were verified.

So this recipe splits the two:

- **artifacts** are attached to the GitHub release `v<version>`, under
  `https://github.com/OWNER/REPO/releases/download/v<version>/<name>`;
- **feeds** live at a stable location you control: a GitHub Pages site, or
  any [static host](static-hosting.md), [R2](cloudflare-r2.md), or
  [S3-compatible](s3-compatible.md) bucket.

## Template changes

1. In the `feeds` and `feeds-macos` jobs, set the download prefix to the
   release asset URL:

   ```sh
   --download-url-prefix "https://github.com/$GITHUB_REPOSITORY/releases/download/v$VERSION/"
   ```

2. Replace the `publish-artifacts` steps. The release is created as a
   published release that is not yet marked latest. It is not a draft, so
   its assets are publicly downloadable for `verify-published`, while GitHub's "latest" marker and
   your feeds still point at the previous version. Give the job
   `permissions: contents: write`.

   ```yaml
   - name: Upload the immutable artifacts to the GitHub release
     env:
       GH_TOKEN: ${{ github.token }}
     run: |
       set -euo pipefail
       if gh release view "v$VERSION" >/dev/null 2>&1; then
         echo "::error::release v$VERSION already published; refusing to overwrite"
         exit 1
       fi
       # Never pass --clobber: an existing asset must make the upload fail.
       gh release create "v$VERSION" dist/* \
         --target "$GITHUB_SHA" --title "v$VERSION" \
         --latest=false --generate-notes
   ```

3. Keep `verify-published` unchanged: it downloads every artifact from the
   release asset URLs referenced by the staged feeds.

4. Replace the `publish-feeds` upload with your feed host's commands (see
   below), then turn the `github-release` job into a promotion step:

   ```sh
   gh release edit "v$VERSION" --latest
   ```

## Stable feed location on GitHub Pages

Publish the feeds to a Pages site, for example
`https://OWNER.github.io/REPO/appcast.xml`, using the official Pages
actions in `publish-feeds`:

```yaml
publish-feeds:
  needs: [version, publish-artifacts, verify-published]
  runs-on: ubuntu-latest
  permissions:
    pages: write
    id-token: write
  environment:
    name: github-pages
  steps:
    - uses: actions/download-artifact@v4
      with:
        pattern: feeds*
        path: site
        merge-multiple: true
    - uses: actions/upload-pages-artifact@v3
      with:
        path: site
    - uses: actions/deploy-pages@v4
```

A Pages deployment replaces the whole site, so `site/` must contain every
feed you publish. The `feeds` jobs already start from the currently
published feeds and only add the new release to them.

## Cache-Control

GitHub sets the headers itself; you cannot configure them:

- Release assets are served from a CDN with long-lived caching. That is
  safe because the URLs contain the tag and are effectively immutable, as
  long as you never delete and re-upload an asset under the same name.
- GitHub Pages serves every file with `Cache-Control: max-age=600`. That is
  an acceptable short lifetime for feeds; clients see a new release within
  about ten minutes. If you need a shorter lifetime, or a custom header, host
  the feeds on [R2](cloudflare-r2.md), [S3](s3-compatible.md), or a
  [static host](static-hosting.md) with `Cache-Control: public, max-age=60`
  instead.

## Rules that keep this safe

- Never use `gh release upload --clobber`, and never delete and recreate a
  published release to "fix" it. Ship a new version instead; an installed
  client may already have cached the old bytes, and the signature in the
  feed would not match the new ones.
- Do not point the application's feed URL at
  `https://github.com/OWNER/REPO/releases/latest/download/appcast.xml`.
  That URL moves when the release is marked latest, not when its feeds are
  verified, and its cache lifetime is not under your control.
