# Generic static hosting

Any HTTPS web server or static-site host works: nginx, Caddy, Apache, a
file server behind a CDN, Netlify, and so on. The requirements are the same
as for object storage: versioned artifacts are immutable and never
overwritten, feeds are mutable with a short cache lifetime, and feeds are
published last.

## Layout

Use the same layout as the template:

```text
https://downloads.example.com/
  appcast.xml, appcast-<os>-<arch>.xml   mutable feeds
  releases/<version>/...                 immutable artifacts
```

## Upload with rsync over SSH

Store a deploy-only SSH private key as the repository secret
`DEPLOY_SSH_KEY` and the server's host key as the variable
`DEPLOY_KNOWN_HOSTS`. Give the deploy user write access to the download root
only.

```yaml
- name: Configure SSH
  env:
    DEPLOY_SSH_KEY: ${{ secrets.DEPLOY_SSH_KEY }}
    DEPLOY_KNOWN_HOSTS: ${{ vars.DEPLOY_KNOWN_HOSTS }}
  run: |
    set -euo pipefail
    install -m 700 -d ~/.ssh
    printf '%s\n' "$DEPLOY_SSH_KEY" > ~/.ssh/deploy
    chmod 600 ~/.ssh/deploy
    printf '%s\n' "$DEPLOY_KNOWN_HOSTS" > ~/.ssh/known_hosts
```

Upload the immutable artifacts. `--ignore-existing` alone would skip an
existing file without failing, which hides a mistake, so the remote
`mkdir` without `-p` is what refuses an existing version: it fails if
`releases/$VERSION` already exists.

```sh
ssh -i ~/.ssh/deploy deploy@downloads.example.com \
  "mkdir /srv/downloads/releases/$VERSION" \
  || { echo "::error::releases/$VERSION already exists; refusing to overwrite"; exit 1; }
rsync -e "ssh -i ~/.ssh/deploy" --ignore-existing --chmod=F644 \
  dist/ "deploy@downloads.example.com:/srv/downloads/releases/$VERSION/"
```

Publish the mutable feeds last, after `verify-published`. Upload each feed
to a temporary name and rename it, so a client never downloads a partly
written feed:

```sh
for feed in site/*.xml; do
  name=$(basename "$feed")
  rsync -e "ssh -i ~/.ssh/deploy" --chmod=F644 \
    "$feed" "deploy@downloads.example.com:/srv/downloads/.$name.tmp"
  ssh -i ~/.ssh/deploy deploy@downloads.example.com \
    "mv /srv/downloads/.$name.tmp /srv/downloads/$name"
done
```

## Cache headers

The server, not the upload, sets `Cache-Control`. For nginx:

```nginx
location /releases/ {
    add_header Cache-Control "public, max-age=31536000, immutable";
}
location ~ ^/appcast.*\.xml$ {
    add_header Cache-Control "public, max-age=60";
    types { application/xml xml; }
}
```

For a host configured with a `_headers` file (Netlify, Cloudflare Pages):

```text
/releases/*
  Cache-Control: public, max-age=31536000, immutable
/appcast*.xml
  Cache-Control: public, max-age=60
```

Note that hosts that deploy a whole site at once (Netlify, Cloudflare Pages,
GitHub Pages) replace every file on each deploy. Do not keep artifacts in
such a site unless each deploy is guaranteed to carry every previously
published artifact unchanged; host the artifacts elsewhere and keep only the
feeds there, as in the [GitHub Releases recipe](github-releases.md).
