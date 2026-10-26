#!/usr/bin/env bash
set -euo pipefail

: "${WORK:?WORK is required}"
: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${NOTARY_KEY_P8_BASE64:?NOTARY_KEY_P8_BASE64 is required}"
: "${NOTARY_KEY_ID:?NOTARY_KEY_ID is required}"
: "${NOTARY_ISSUER_ID:?NOTARY_ISSUER_ID is required}"
umask 077
scratch="$(mktemp -d "$RUNNER_TEMP/notary.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
key="$scratch/AuthKey.p8"
printf '%s' "$NOTARY_KEY_P8_BASE64" | base64 --decode > "$key"
openssl pkey -in "$key" -noout > /dev/null
codesign --verify --deep --strict "$WORK/Reference App.app"
xcrun notarytool submit "$WORK/ReferenceApp.zip" \
  --key "$key" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID" \
  --wait --output-format json > "$WORK/notarization.json"
python3 - "$WORK/notarization.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as result:
    response = json.load(result)
if response.get("status") != "Accepted":
    sys.exit("Notarization was not Accepted; inspect notarization.json before distribution.")
PY
xcrun stapler staple "$WORK/Reference App.app"
xcrun stapler validate "$WORK/Reference App.app"
spctl --assess --type execute --verbose "$WORK/Reference App.app"
# The submitted archive predates stapling. Distribute a new archive containing the ticket.
rm "$WORK/ReferenceApp.zip"
ditto -c -k --sequesterRsrc --keepParent "$WORK/Reference App.app" "$WORK/ReferenceApp.zip"
