#!/usr/bin/env bash
# Conditions passed to wait_until are evaluated later, on purpose.
# shellcheck disable=SC2016

# End-to-end update test of the reference app on Linux.
#
# Builds the reference app three times from the same sources, as version
# 1.0.0, 1.1.0, and a deliberately broken 1.2.0, packages them as managed
# tarball releases, publishes them in a signed native feed served from
# loopback, and checks, through the app's unattended mode
# (apps/reference-app/src/unattended.rs), that:
#
# 1. an installed 1.0.0 updates itself to 1.1.0, the helper swaps the
#    prefix, and 1.1.0 confirms its start;
# 2. when 1.1.0 then installs the broken 1.2.0, which exits before its main
#    window opens, the helper restores and relaunches 1.1.0, and the restored
#    app reports the rollback.
#
# Usage: tools/e2e/linux-update.sh
#   Needs a display (run it under `xvfb-run -a`), a Vulkan driver (Mesa's
#   lavapipe is enough), cargo, python3, and tar. Honors CARGO_TARGET_DIR.
#   Everything else lives in a temporary directory (kept on failure, or set
#   E2E_WORK_DIR to choose it), including HOME for the app, and nothing
#   leaves the machine.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

if [[ "$(uname -s)" != Linux ]]; then
  echo "this end-to-end test runs on Linux only" >&2
  exit 1
fi
arch="$(uname -m)"
case "$arch" in
  x86_64 | aarch64) ;;
  *) echo "unsupported architecture $arch" >&2; exit 1 ;;
esac

work="${E2E_WORK_DIR:-$(mktemp -d)}"
mkdir -p "$work"
work="$(cd "$work" && pwd -P)"
site="$work/site"
releases="$work/releases"
home="$work/home"
opt="$home/.local/opt"
prefix="$opt/reference-app"
report="$work/report.log"
app_log="$work/app.log"
mkdir -p "$site" "$releases" "$opt"
: >"$report"
: >"$app_log"

target_dir="$(cargo metadata --format-version 1 --no-deps --locked |
  python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')"

server_pid=""
cleanup() {
  local status=$?
  pkill -f "$prefix/bin/reference-app" 2>/dev/null || true
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" 2>/dev/null || true
  fi
  if [[ $status -ne 0 ]]; then
    echo "--- report ($report)" >&2
    cat "$report" >&2 || true
    echo "--- app and helper output ($app_log)" >&2
    cat "$app_log" >&2 || true
    echo "--- next to the install ($opt)" >&2
    ls -la "$opt" >&2 || true
    echo "end-to-end test failed; files kept in $work" >&2
  elif [[ -z "${E2E_WORK_DIR:-}" ]]; then
    rm -rf "$work"
  fi
}
trap cleanup EXIT

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

step() {
  echo "==> $*"
}

# Waits up to $2 seconds for the exact line $1 to appear in the report.
wait_for_line() {
  local line="$1" timeout="$2" waited=0
  until grep -Fxq -- "$line" "$report"; do
    if ((waited >= timeout * 2)); then
      fail "no \"$line\" in the report after ${timeout}s"
    fi
    sleep 0.5
    waited=$((waited + 1))
  done
}

# Waits up to $2 seconds until the shell test "$1" holds.
wait_until() {
  local condition="$1" timeout="$2" waited=0
  until eval "$condition"; do
    if ((waited >= timeout * 2)); then
      fail "timed out after ${timeout}s waiting for: $condition"
    fi
    sleep 0.5
    waited=$((waited + 1))
  done
}

no_app_running() { ! pgrep -f "$prefix/bin/reference-app" >/dev/null; }
siblings() { find "$opt" -mindepth 1 -maxdepth 1 -name ".reference-app.gpui-auto-update-$1*" | wc -l; }
installed_version() { "$prefix/bin/reference-app" --version; }

launch() {
  env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME \
    HOME="$home" "$prefix/bin/reference-app" >>"$app_log" 2>&1 &
}

step "Building the release tooling"
cargo build --locked -q -p gpui-auto-update-cli
cli="$target_dir/debug/gpui-auto-update"

step "Generating a disposable signing key"
public_key="$("$cli" keys generate --output "$work/signing-key")"

port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
base_url="http://127.0.0.1:$port"
feed="$site/feed-linux-$arch.xml"

# Builds and packages version $1; $2 is 1 for the broken build.
release() {
  local version="$1" broken="$2"
  local name="reference-app-$version-linux-$arch"
  step "Building reference app $version (broken: $broken)"
  REFERENCE_APP_ID=dev.gpui-auto-update.e2e \
    REFERENCE_APP_VERSION="$version" \
    REFERENCE_APP_FEED_URL="$base_url/feed-linux-$arch.xml" \
    REFERENCE_APP_PUBLIC_KEY="$public_key" \
    REFERENCE_APP_ALLOW_INSECURE_HTTP=1 \
    REFERENCE_APP_ALLOW_DEBUG_SELF_UPDATE=1 \
    REFERENCE_APP_E2E_REPORT="$report" \
    REFERENCE_APP_E2E_FAIL_TO_START="$broken" \
    cargo build --locked -q -p gpui-auto-update-reference-app
  mkdir -p "$releases/$name/bin" "$releases/$name/share/reference-app" "$site/$version"
  cp "$target_dir/debug/reference-app" "$releases/$name/bin/reference-app"
  strip "$releases/$name/bin/reference-app" 2>/dev/null || true
  printf 'gpui-auto-update managed-install 1\napp=reference-app\n' \
    >"$releases/$name/share/reference-app/gpui-auto-update.managed"
  tar -C "$releases" -czf "$site/$version/$name.tar.gz" "$name"
  [[ "$("$releases/$name/bin/reference-app" --version)" == "$version" ]] ||
    fail "the $version build reports another version"
}

# Adds the packaged version $1 to the feed.
publish() {
  local version="$1" existing=()
  if [[ -f "$feed" ]]; then
    existing=(--feed "$feed")
  fi
  step "Publishing $version"
  "$cli" feed native --os linux --arch "$arch" --version "$version" \
    --artifact "$site/$version/reference-app-$version-linux-$arch.tar.gz" \
    --download-url-prefix "$base_url/$version/" --allow-http \
    --public-key "$public_key" --key-file "$work/signing-key" \
    "${existing[@]}" --output "$feed"
}

release 1.0.0 0
release 1.1.0 0
release 1.2.0 1

step "Serving the feed on $base_url"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$site" \
  >"$work/server.log" 2>&1 &
server_pid=$!
publish 1.1.0
wait_until "python3 -c 'import urllib.request; urllib.request.urlopen(\"$base_url/feed-linux-$arch.xml\")' 2>/dev/null" 30

step "Installing 1.0.0 as a managed install in $prefix"
cp -R "$releases/reference-app-1.0.0-linux-$arch" "$prefix"

# --- Scenario 1: 1.0.0 -> 1.1.0 -----------------------------------------
step "Starting 1.0.0; it should update itself to 1.1.0"
launch
wait_for_line "started 1.0.0" 120
wait_for_line "update-available 1.1.0" 60
wait_for_line "handoff" 120
wait_for_line "started 1.1.0" 120
# The backup is deleted, and the helper exits, once 1.1.0 confirms its start.
wait_until '[[ "$(siblings backup)" -eq 0 ]]' 90
wait_for_line "up-to-date" 60
[[ "$(installed_version)" == 1.1.0 ]] || fail "1.1.0 is not installed after the update"
[[ "$(siblings diagnostic)" -eq 0 ]] || fail "the successful update left a diagnostic"
[[ "$(siblings staged)" -eq 0 ]] || fail "the staged release was not removed"
if grep -q '^previous-update-failure' "$report"; then
  fail "a successful update reported a failure"
fi
echo "PASS: 1.0.0 updated to 1.1.0"

step "Quitting 1.1.0"
pkill -f "$prefix/bin/reference-app" || true
wait_until no_app_running 30
: >"$report"

# --- Scenario 2: 1.1.0 -> broken 1.2.0 -> rolled back to 1.1.0 ----------
publish 1.2.0
step "Starting 1.1.0; the broken 1.2.0 should be rolled back"
launch
wait_for_line "started 1.1.0" 120
wait_for_line "update-available 1.2.0" 60
wait_for_line "handoff" 120
wait_for_line "failed-to-start 1.2.0" 120
wait_until 'grep -q "^previous-update-failure " "$report"' 120
wait_until '[[ "$(siblings failed)" -eq 0 && "$(siblings backup)" -eq 0 ]]' 60
[[ "$(installed_version)" == 1.1.0 ]] || fail "1.1.0 was not restored"
# The restored app read and removed the diagnostic when it started.
[[ "$(siblings diagnostic)" -eq 0 ]] || fail "the diagnostic was not consumed"

expected="started 1.1.0
update-available 1.2.0
handoff
failed-to-start 1.2.0
started 1.1.0"
actual="$(grep -v '^previous-update-failure ' "$report" | grep -v '^error ' || true)"
[[ "$actual" == "$expected" ]] || fail "unexpected sequence:
$actual"
failure="$(grep '^previous-update-failure ' "$report")"
[[ "$failure" == "previous-update-failure HealthConfirmation: "* ]] ||
  fail "the rollback was reported as: $failure"
[[ "$(tail -n 1 "$report")" == "$failure" ]] ||
  fail "the rollback was not reported by the restored app"
echo "PASS: the broken 1.2.0 was rolled back to 1.1.0, which reported: ${failure#previous-update-failure }"
