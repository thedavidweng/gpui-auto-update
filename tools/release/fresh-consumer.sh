#!/usr/bin/env bash
# Packages every publishable workspace crate with `cargo package` and builds
# the resulting `.crate` contents from a brand-new project outside the
# workspace, the way a crates.io user would see them. Files missing from the
# package, inherited workspace settings that do not survive packaging, and
# docs warnings in the packaged sources all fail here before publication.
#
# Usage: tools/release/fresh-consumer.sh [--no-verify]
#   --no-verify  Skip `cargo package`'s own per-crate verification build
#                (the fresh-consumer build below still compiles everything).
#
# Requires: cargo, jq, tar. Honors CARGO_TARGET_DIR.
set -euo pipefail

verify=()
if [[ "${1:-}" == "--no-verify" ]]; then
  verify=(--no-verify)
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

metadata="$(cargo metadata --format-version 1 --no-deps --locked)"
target_dir="$(jq -r '.target_directory' <<<"$metadata")"
# `publish = false` shows up as an empty registry list; `null` means crates.io.
published="$(jq -r '.packages[] | select(.publish != []) | "\(.name) \(.version)"' <<<"$metadata")"
if [[ -z "$published" ]]; then
  echo "no publishable packages found" >&2
  exit 1
fi

package_args=()
while read -r name _; do
  package_args+=(-p "$name")
done <<<"$published"

# Packaging the crates together lets cargo resolve not-yet-published
# workspace dependencies through a temporary local registry.
cargo package --locked --allow-dirty ${verify[@]+"${verify[@]}"} "${package_args[@]}"

work="$(mktemp -d "${TMPDIR:-/tmp}/fresh-consumer.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/crates" "$work/.cargo" "$work/consumer/src"

# Path patches point crates.io names at the unpacked `.crate` files. The
# config lives above both the consumer and the unpacked crates, so it also
# applies when building the CLI binary package on its own.
patches="$work/.cargo/config.toml"
echo "[patch.crates-io]" >"$patches"
deps=""
while read -r name version; do
  archive="$target_dir/package/$name-$version.crate"
  tar -xzf "$archive" -C "$work/crates"
  dir="$work/crates/$name-$version"
  # Git Bash on Windows needs `pwd -W` for a path cargo understands; TOML
  # literal strings keep any backslashes intact.
  native_dir="$(cd "$dir" && { pwd -W 2>/dev/null || pwd; })"
  echo "$name = { path = '$native_dir' }" >>"$patches"
  if jq -e --arg n "$name" \
    '.packages[] | select(.name == $n) | any(.targets[]; .kind | index("lib"))' \
    <<<"$metadata" >/dev/null; then
    deps+="$name = \"=$version\""$'\n'
  fi
done <<<"$published"

gpui_req="$(jq -r '[.packages[] | .dependencies[] | select(.name == "gpui") | .req] | first' <<<"$metadata")"

cat >"$work/consumer/Cargo.toml" <<EOF
[package]
name = "fresh-consumer"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
${deps}
# Applications, not the libraries, choose GPUI platform features.
gpui = { version = "${gpui_req}", features = ["font-kit", "wayland", "x11", "windows-manifest"] }
EOF

{
  echo "//! Links every published library crate from its packaged sources."
  while read -r name _; do
    if grep -q "^$name = " <<<"$deps"; then
      echo "use ${name//-/_} as _;"
    fi
  done <<<"$published"
  echo "fn main() {}"
} >"$work/consumer/src/main.rs"

echo "--- consumer manifest"
cat "$work/consumer/Cargo.toml"
echo "--- patches"
cat "$patches"

cd "$work/consumer"
cargo generate-lockfile
# Applies the libc pin the README tells applications to use with gpui 0.2.2;
# drop this together with that README note.
cargo update -p libc --precise 0.2.189
cargo build
cargo run --quiet

doc_args=()
while read -r name _; do
  if grep -q "^$name = " <<<"$deps"; then
    doc_args+=(-p "$name")
  fi
done <<<"$published"
RUSTDOCFLAGS="${RUSTDOCFLAGS:-} -D warnings" cargo doc --no-deps "${doc_args[@]}"

# Binary-only packages cannot be dependencies, so build them in place.
while read -r name version; do
  if ! grep -q "^$name = " <<<"$deps"; then
    cargo build --manifest-path "$work/crates/$name-$version/Cargo.toml" --bins
  fi
done <<<"$published"

echo "fresh-consumer: all packaged crates build and document cleanly"
