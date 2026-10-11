#!/usr/bin/env bash
# End-to-end update of the reference application on macOS.
#
# Builds the reference app twice (version N and N+1) with the `sparkle`
# feature, packages each as a signed .app with Sparkle.framework embedded,
# generates a signed appcast with a delta from N to N+1 using a throwaway
# key, serves it over loopback HTTP, and then lets an installed copy of
# version N update itself, unattended (`REFERENCE_APP_E2E_REPORT`, see
# apps/reference-app/src/unattended.rs). The script asserts on the report
# lines the app writes (`started`, `update-available`, `handoff`, `error`):
#
#   delta: the generated delta is selected and N becomes N+1.
#   full:  the delta is unavailable, Sparkle falls back to the full
#          archive, and N becomes N+1.
#
# Nothing touches the network except `sparkle fetch` (skipped with
# SPARKLE_DIR) and loopback. Signing is ad hoc; no credentials are used.
#
# Environment:
#   SPARKLE_DIR     an extracted Sparkle distribution (`sparkle fetch`);
#                   fetched into the work directory when unset
#   E2E_WORK_DIR    where to build and install (default: a new temp dir)
#   E2E_PROFILE     cargo profile: debug (default) or release
#   E2E_TIMEOUT     seconds each update may take (default 180)
#   E2E_SCENARIOS   which scenarios to run (default "delta full")
#   E2E_CLEAN_SPARKLE_STATE=1  remove the test bundles' Sparkle defaults and
#                   caches afterwards (CI; the bundle identifiers are unique
#                   to this run)
#   CARGO_TARGET_DIR  as usual

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=${E2E_WORK_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/reference-e2e.XXXXXX")}
PROFILE=${E2E_PROFILE:-debug}
TIMEOUT=${E2E_TIMEOUT:-180}
SCENARIOS=${E2E_SCENARIOS:-"delta full"}
TARGET=${CARGO_TARGET_DIR:-$ROOT/target}
RUN_ID="$(date +%s)$$"
# Compiled into the binaries, so every scenario shares it; it is emptied
# before each run.
REPORT="$WORK/report.log"

OLD_VERSION=1.0.0
OLD_BUILD=100
NEW_VERSION=1.0.1
NEW_BUILD=101
APP_NAME="Reference App"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

mkdir -p "$WORK"
log "work directory: $WORK"

profile_flag=()
profile_dir=debug
if [[ $PROFILE == release ]]; then
  profile_flag=(--release)
  profile_dir=release
fi

cargo build --locked -q -p gpui-auto-update-cli --manifest-path "$ROOT/Cargo.toml"
CLI="$TARGET/debug/gpui-auto-update"

if [[ -z ${SPARKLE_DIR:-} ]]; then
  SPARKLE_DIR="$WORK/sparkle"
  [[ -d $SPARKLE_DIR/Sparkle.framework ]] || "$CLI" sparkle fetch --out "$SPARKLE_DIR"
fi
[[ -d $SPARKLE_DIR/Sparkle.framework ]] || die "no Sparkle.framework in $SPARKLE_DIR"
SPARKLE_DIR=$(cd "$SPARKLE_DIR" && pwd)

# A throwaway signing key, owner-only, inside the work directory.
KEY="$WORK/ed25519.key"
[[ -f $KEY ]] || "$CLI" keys generate --output "$KEY" >/dev/null
PUBLIC_KEY=$("$CLI" keys public-key --key-file "$KEY")

PORT=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
SERVER_PID=
cleanup() {
  if [[ -n $SERVER_PID ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [[ ${E2E_CLEAN_SPARKLE_STATE:-} == 1 ]]; then
    for scenario in $SCENARIOS; do
      id=$(bundle_id "$scenario")
      defaults delete "$id" >/dev/null 2>&1 || true
      rm -rf "$HOME/Library/Caches/$id"
    done
  fi
}
trap cleanup EXIT

bundle_id() { echo "dev.gpui-auto-update.reference-e2e.$1.r$RUN_ID"; }

# Builds the reference app binary for one version.
build_binary() {
  local version=$1 out=$2
  log "building reference app $version ($PROFILE)"
  REFERENCE_APP_VERSION=$version \
  REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE=1 \
  REFERENCE_APP_E2E_REPORT=$REPORT \
  SPARKLE_FRAMEWORK_PATH=$SPARKLE_DIR \
    cargo build --locked -q -p gpui-auto-update-reference-app --features sparkle \
      ${profile_flag[@]+"${profile_flag[@]}"} --manifest-path "$ROOT/Cargo.toml"
  cp "$TARGET/$profile_dir/reference-app" "$out"
}

# Packages a binary as an ad-hoc signed .app for one scenario.
package() {
  local binary=$1 version=$2 build=$3 id=$4 feed=$5 app=$6
  "$ROOT/tools/e2e/package-reference-app.sh" --cli "$CLI" --sparkle "$SPARKLE_DIR" \
    --binary "$binary" --app "$app" --version "$version" --build "$build" \
    --bundle-id "$id" --feed "$feed" --public-key "$PUBLIC_KEY" \
    || die "could not package $app"
}

show_report() {
  echo "--- app log ($1)" >&2
  cat "$1" >&2
  echo "--- report ($REPORT)" >&2
  cat "$REPORT" >&2
}

bundle_version() {
  /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$1/Contents/Info.plist" 2>/dev/null || true
}

mkdir -p "$WORK/bin"
build_binary "$OLD_VERSION" "$WORK/bin/reference-app-$OLD_VERSION"
build_binary "$NEW_VERSION" "$WORK/bin/reference-app-$NEW_VERSION"

# The bundle structure, validated without any server: a bundle with an https
# feed passes `gpui-auto-update sparkle validate` (nested Sparkle code, run
# paths, Info.plist, license notice, signatures) besides `codesign --verify
# --deep --strict`.
log "validating the bundle signing structure"
package "$WORK/bin/reference-app-$OLD_VERSION" "$OLD_VERSION" "$OLD_BUILD" \
  "$(bundle_id structure)" "https://example.invalid/appcast.xml" "$WORK/structure/$APP_NAME.app"
"$CLI" sparkle validate --app "$WORK/structure/$APP_NAME.app" --sandbox non-sandboxed >&2 \
  || die "the bundle does not validate"

FEED="http://127.0.0.1:$PORT/appcast.xml"
SERVE="$WORK/serve"
rm -rf "$SERVE"
mkdir -p "$SERVE"
python3 -u -m http.server "$PORT" --bind 127.0.0.1 --directory "$SERVE" >"$WORK/server.log" 2>&1 &
SERVER_PID=$!

run_scenario() {
  local scenario=$1
  local id dir archives install
  id=$(bundle_id "$scenario")
  dir="$WORK/$scenario"
  archives="$dir/archives"
  install="$dir/install/$APP_NAME.app"
  log "[$scenario] packaging $OLD_VERSION and $NEW_VERSION as $id"
  rm -rf "$dir"
  mkdir -p "$archives" "$dir/install"
  for pair in "$OLD_VERSION:$OLD_BUILD" "$NEW_VERSION:$NEW_BUILD"; do
    local version=${pair%%:*} build=${pair##*:}
    local app="$dir/$version/$APP_NAME.app"
    package "$WORK/bin/reference-app-$version" "$version" "$build" "$id" "$FEED" "$app"
    ditto -c -k --sequesterRsrc --keepParent "$app" "$archives/ReferenceApp-$version.zip"
  done

  log "[$scenario] generating the signed appcast"
  "$CLI" feed sparkle --sparkle "$SPARKLE_DIR" --archives "$archives" \
    --output "$archives/appcast.xml" --key-file "$KEY" --public-key "$PUBLIC_KEY" \
    --download-url-prefix "http://127.0.0.1:$PORT/$scenario/" >&2
  grep -q 'sparkle:deltaFrom="'"$OLD_BUILD"'"' "$archives/appcast.xml" \
    || die "[$scenario] the appcast has no delta from $OLD_BUILD"
  local delta
  delta=$(find "$archives" -maxdepth 1 -name '*.delta' -exec basename {} \; | head -n1)
  [[ -n $delta ]] || die "[$scenario] generate_appcast produced no delta"

  rm -rf "${SERVE:?}/$scenario"
  cp -R "$archives" "$SERVE/$scenario"
  cp "$archives/appcast.xml" "$SERVE/appcast.xml"
  if [[ $scenario == full ]]; then
    # The delta is listed but cannot be downloaded.
    rm "$SERVE/$scenario/$delta"
  fi

  ditto "$dir/$OLD_VERSION/$APP_NAME.app" "$install"
  [[ $("$install/Contents/MacOS/reference-app" --version) == "$OLD_VERSION" ]] \
    || die "[$scenario] the installed app is not $OLD_VERSION"

  log "[$scenario] running $OLD_VERSION unattended"
  local log_start requests="$dir/requests.log"
  log_start=$(wc -l <"$WORK/server.log")
  local app_log="$dir/app.log" status=0
  : >"$REPORT"
  "$install/Contents/MacOS/reference-app" >"$app_log" 2>&1 &
  local app_pid=$!
  local deadline=$((SECONDS + TIMEOUT))
  while kill -0 "$app_pid" 2>/dev/null; do
    if ((SECONDS > deadline)); then
      kill "$app_pid" 2>/dev/null || true
      show_report "$app_log"
      die "[$scenario] the app did not quit within ${TIMEOUT}s"
    fi
    sleep 1
  done
  wait "$app_pid" || status=$?
  show_report "$app_log"
  ((status == 0)) || die "[$scenario] the app exited with status $status"
  if grep -q '^error ' "$REPORT"; then
    die "[$scenario] the app reported an error"
  fi
  [[ $(grep -v '^error ' "$REPORT") == "started $OLD_VERSION
update-available $NEW_VERSION
handoff" ]] || die "[$scenario] unexpected report lines (wanted started, update-available, handoff)"

  log "[$scenario] waiting for Sparkle to install $NEW_VERSION"
  while [[ $(bundle_version "$install") != "$NEW_VERSION" ]]; do
    if ((SECONDS > deadline)); then
      tail -n "+$((log_start + 1))" "$WORK/server.log" >&2
      die "[$scenario] Sparkle did not install $NEW_VERSION (still $(bundle_version "$install"))"
    fi
    sleep 1
  done
  # Sparkle swaps the bundle in place; give it a moment to finish.
  sleep 2
  [[ $("$install/Contents/MacOS/reference-app" --version) == "$NEW_VERSION" ]] \
    || die "[$scenario] the installed binary is not $NEW_VERSION"
  codesign --verify --deep --strict "$install" \
    || die "[$scenario] the updated app is not validly signed"

  tail -n "+$((log_start + 1))" "$WORK/server.log" >"$requests"
  cat "$requests" >&2
  # The server logs request paths URL-encoded.
  delta=${delta// /%20}
  local full="ReferenceApp-$NEW_VERSION.zip"
  case $scenario in
    delta)
      grep -q "GET /$scenario/$delta HTTP/1.1\" 200" "$requests" \
        || die "[delta] Sparkle did not download the delta"
      if grep -q "GET /$scenario/$full " "$requests"; then
        die "[delta] Sparkle downloaded the full archive although the delta applies"
      fi
      ;;
    full)
      grep -q "GET /$scenario/$delta HTTP/1.1\" 404" "$requests" \
        || die "[full] Sparkle did not try the delta first"
      grep -q "GET /$scenario/$full HTTP/1.1\" 200" "$requests" \
        || die "[full] Sparkle did not fall back to the full archive"
      ;;
  esac
  # The updated app starts and finds nothing newer.
  : >"$REPORT"
  "$install/Contents/MacOS/reference-app" >>"$app_log" 2>&1 &
  app_pid=$!
  until grep -qx 'up-to-date' "$REPORT"; do
    if ((SECONDS > deadline)) || ! kill -0 "$app_pid" 2>/dev/null; then
      kill "$app_pid" 2>/dev/null || true
      show_report "$app_log"
      die "[$scenario] the updated app did not report up-to-date"
    fi
    sleep 1
  done
  kill "$app_pid" 2>/dev/null || true
  wait "$app_pid" 2>/dev/null || true
  [[ $(head -n1 "$REPORT") == "started $NEW_VERSION" ]] \
    || die "[$scenario] the updated app did not start as $NEW_VERSION"

  log "[$scenario] ok: $OLD_VERSION updated itself to $NEW_VERSION"
}

for scenario in $SCENARIOS; do
  run_scenario "$scenario"
done
log "all scenarios passed: $SCENARIOS"
