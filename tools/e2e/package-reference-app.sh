#!/usr/bin/env bash
# Packages a reference-app binary built with the `sparkle` feature as a
# macOS application bundle: writes Info.plist, embeds Sparkle.framework,
# and signs it with `gpui-auto-update sparkle sign`.
#
# Usage:
#   package-reference-app.sh --cli <gpui-auto-update> --sparkle <dir> \
#     --binary <reference-app> --app <out/Name.app> \
#     --version <x.y.z> --build <n> --bundle-id <id> \
#     --feed <SUFeedURL> --public-key <SUPublicEDKey> \
#     [--identity <identity>] [--keychain <keychain>]
#
# The identity defaults to `-` (ad hoc). With a Developer ID identity the
# bundle is validated for notarization (`sparkle validate
# --require-developer-id`); the caller submits it.

set -euo pipefail

identity=-
keychain=
while (($#)); do
  case $1 in
    --cli) cli=$2 ;;
    --sparkle) sparkle=$2 ;;
    --binary) binary=$2 ;;
    --app) app=$2 ;;
    --version) version=$2 ;;
    --build) build=$2 ;;
    --bundle-id) bundle_id=$2 ;;
    --feed) feed=$2 ;;
    --public-key) public_key=$2 ;;
    --identity) identity=$2 ;;
    --keychain) keychain=$2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done
for required in cli sparkle binary app version build bundle_id feed public_key; do
  [[ -n ${!required:-} ]] || { echo "missing --${required//_/-}" >&2; exit 2; }
done

name=$(basename "$app" .app)
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$binary" "$app/Contents/MacOS/reference-app"
# Sparkle downloads updates silently (SUAutomaticallyUpdate) only while
# automatic checks are enabled.
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleName</key><string>$name</string>
  <key>CFBundleExecutable</key><string>reference-app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$build</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>SUFeedURL</key><string>$feed</string>
  <key>SUPublicEDKey</key><string>$public_key</string>
  <key>SUEnableAutomaticChecks</key><true/>
  <key>SUAutomaticallyUpdate</key><true/>
</dict>
</plist>
PLIST

"$cli" sparkle embed --app "$app" --sparkle "$sparkle" --sandbox non-sandboxed >/dev/null
sign=("$cli" sparkle sign --app "$app" --identity "$identity")
[[ -n $keychain ]] && sign+=(--keychain "$keychain")
"${sign[@]}" >/dev/null
codesign --verify --deep --strict "$app"
# Info.plist metadata, framework placement, run paths, license notice, and
# the nested signatures. A Developer ID identity must also meet the
# notarization requirements (secure timestamps).
# `validate` insists on an https feed, so a bundle that points at a loopback
# http server (the update e2e) is only checked with codesign above.
if [[ $feed == https://* ]]; then
  validate=("$cli" sparkle validate --app "$app" --sandbox non-sandboxed)
  [[ $identity != - ]] && validate+=(--require-developer-id)
  "${validate[@]}" >/dev/null
fi
