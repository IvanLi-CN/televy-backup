#!/usr/bin/env bash
set -euo pipefail

# Compute effective semver from src-tauri/Cargo.toml, auto-increment patch if tag exists.

root_dir="$(git rev-parse --show-toplevel)"

git fetch --tags --force >/dev/null 2>&1 || true

cargo_ver="$(
  grep -m1 '^version[[:space:]]*=[[:space:]]*"' "$root_dir/src-tauri/Cargo.toml" \
    | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/'
)"

if [[ -z "${cargo_ver:-}" ]]; then
  echo "Failed to detect version from src-tauri/Cargo.toml" >&2
  exit 1
fi

base_major="$(echo "$cargo_ver" | cut -d. -f1)"
base_minor="$(echo "$cargo_ver" | cut -d. -f2)"
base_patch="$(echo "$cargo_ver" | cut -d. -f3)"

candidate="${base_patch}"
while git rev-parse -q --verify "refs/tags/v${base_major}.${base_minor}.${candidate}" >/dev/null; do
  candidate="$((candidate + 1))"
done

effective="${base_major}.${base_minor}.${candidate}"

echo "APP_EFFECTIVE_VERSION=${effective}" >> "${GITHUB_ENV:-/dev/stdout}"
echo "Computed APP_EFFECTIVE_VERSION=${effective} (base ${cargo_ver})"

