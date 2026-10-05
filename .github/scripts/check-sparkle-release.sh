#!/usr/bin/env bash
# Opens an issue when the latest stable Sparkle release is newer than the
# default pin in crates/gpui-auto-update-cli/sparkle-pins.json.
#
# Needs `gh` (authenticated through GH_TOKEN), `jq`, `curl`, and `sha256sum`
# or `shasum`. Set DRY_RUN=1 to print the issue instead of opening it.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pins="$repo_root/crates/gpui-auto-update-cli/sparkle-pins.json"
upstream="sparkle-project/Sparkle"

pinned="$(jq -r .default "$pins")"
# /releases/latest never returns drafts or prereleases.
release="$(gh api "repos/$upstream/releases/latest")"
latest="$(jq -r .tag_name <<<"$release")"
latest="${latest#v}"

newest="$(printf '%s\n%s\n' "$pinned" "$latest" | sort -V | tail -n 1)"
if [[ "$latest" == "$pinned" || "$newest" == "$pinned" ]]; then
  echo "Sparkle $pinned is pinned and is the latest stable release."
  exit 0
fi

title="Sparkle $latest is available (pinned: $pinned)"
security=false
if jq -r '.body // ""' <<<"$release" | grep -Eiq 'security|vulnerab|CVE-[0-9]'; then
  security=true
  title="[security] $title"
fi

existing="$(gh issue list --state all --search "\"Sparkle $latest is available\" in:title" --json number --jq 'length')"
if [[ "$existing" != "0" ]]; then
  echo "An issue for Sparkle $latest already exists."
  exit 0
fi

asset="Sparkle-$latest.tar.xz"
url="https://github.com/$upstream/releases/download/$latest/$asset"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
checksum="(download failed; compute it before pinning)"
size="unknown"
if curl -fsSL --proto '=https' --max-filesize 268435456 -o "$tmp/$asset" "$url"; then
  if command -v sha256sum >/dev/null; then
    checksum="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)"
  else
    checksum="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"
  fi
  size="$(wc -c <"$tmp/$asset" | tr -d ' ')"
fi

priority="Routine maintenance."
if [[ "$security" == true ]]; then
  priority="**The release notes mention security fixes. Treat this as high-priority maintenance.**"
fi

release_url="$(jq -r .html_url <<<"$release")"
IFS= read -r -d '' body <<EOF || true
Sparkle [$latest]($release_url) is the latest stable release; this project pins $pinned.

$priority

| | |
| --- | --- |
| Archive | $url |
| SHA-256 (computed by this workflow) | \`$checksum\` |
| Size | $size bytes |

To update the pin:

- [ ] Download the archive independently and confirm the SHA-256 above.
- [ ] Read the release notes for changes to the minimum macOS version, XPC services, signing, or the appcast format.
- [ ] Add the release to \`crates/gpui-auto-update-cli/sparkle-pins.json\` (version, url, sha256, size, minimum_system_version from the framework's Info.plist) and make it the default.
- [ ] Run \`cargo test -p gpui-auto-update-cli -- --ignored\` on macOS to package, sign, validate, and launch against the new framework.
- [ ] Update the docs and CHANGELOG.

Opened by the scheduled Sparkle release check.
EOF

labels=(--label needs-triage)
if [[ "$security" == true ]]; then
  labels+=(--label security)
fi

if [[ "${DRY_RUN:-0}" == 1 ]]; then
  printf 'Would open: %s\n%s\n' "$title" "$body"
  exit 0
fi

gh label create needs-triage --color ededed --description "Needs maintainer triage" 2>/dev/null || true
gh label create security --color b60205 --description "Security-relevant" 2>/dev/null || true
gh issue create --title "$title" --body "$body" "${labels[@]}"
