#!/usr/bin/env bash
set -euo pipefail

inputs=(SIGNING_CERTIFICATE_BASE64 MACOS_CERTIFICATE_PASSWORD MACOS_SIGNING_IDENTITY
  NOTARY_KEY_P8_BASE64 NOTARY_KEY_ID NOTARY_ISSUER_ID)
missing=()
for name in "${inputs[@]}"; do
  if [[ -z "${!name:-}" ]]; then missing+=("$name"); fi
done
if [[ ${#missing[@]} -eq ${#inputs[@]} && "${REQUIRE_NOTARIZATION:-false}" != true ]]; then
  echo "present=false" >> "$GITHUB_OUTPUT"
  echo "::notice::No signing inputs configured. This run does not establish notarized distribution."
elif [[ ${#missing[@]} -gt 0 ]]; then
  echo "::error::Incomplete signing inputs: ${missing[*]}"
  exit 1
elif [[ "$MACOS_SIGNING_IDENTITY" != "Developer ID Application: "* ]]; then
  echo "::error::Signing requires a Developer ID Application identity."
  exit 1
else
  echo "present=true" >> "$GITHUB_OUTPUT"
fi
