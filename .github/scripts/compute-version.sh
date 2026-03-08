#!/usr/bin/env bash
set -euo pipefail

# Compute effective semver by:
# - base version: max stable semver tag (vX.Y.Z or X.Y.Z), fallback daemon Cargo.toml version
# - bump: major/minor/patch, controlled by $BUMP_LEVEL
# - uniqueness: keep incrementing patch until an unused stable tag is found
# Output:
# - APP_EFFECTIVE_VERSION=<semver> (no leading v)

root_dir="$(git rev-parse --show-toplevel)"

git fetch --tags --force >/dev/null 2>&1 || true

cargo_ver="$({
  grep -m1 '^version[[:space:]]*=[[:space:]]*"' "$root_dir/crates/daemon/Cargo.toml" || true
} | sed -E 's/.*"([0-9]+\.[0-9]+\.[0-9]+)".*/\1/')"

if [[ -z "${cargo_ver:-}" ]]; then
  echo "Failed to detect version from crates/daemon/Cargo.toml" >&2
  exit 1
fi

if [[ -z "${BUMP_LEVEL:-}" ]]; then
  echo "Missing BUMP_LEVEL (expected: major|minor|patch)" >&2
  exit 1
fi

case "${BUMP_LEVEL}" in
  major|minor|patch)
    ;;
  *)
    echo "Invalid BUMP_LEVEL=${BUMP_LEVEL} (expected: major|minor|patch)" >&2
    exit 1
    ;;
esac

max_tag="$(
  git tag -l \
    | grep -E '^v?[0-9]+\.[0-9]+\.[0-9]+$' \
    | sed -E 's/^v//' \
    | sort -Vu \
    | tail -n 1 \
    || true
)"

base_ver="${max_tag:-$cargo_ver}"
base_major="$(echo "$base_ver" | cut -d. -f1)"
base_minor="$(echo "$base_ver" | cut -d. -f2)"
base_patch="$(echo "$base_ver" | cut -d. -f3)"

case "${BUMP_LEVEL}" in
  major)
    next_major="$((base_major + 1))"
    next_minor="0"
    next_patch="0"
    ;;
  minor)
    next_major="${base_major}"
    next_minor="$((base_minor + 1))"
    next_patch="0"
    ;;
  patch)
    next_major="${base_major}"
    next_minor="${base_minor}"
    next_patch="$((base_patch + 1))"
    ;;
esac

candidate="${next_patch}"
while git rev-parse -q --verify "refs/tags/v${next_major}.${next_minor}.${candidate}" >/dev/null \
  || git rev-parse -q --verify "refs/tags/${next_major}.${next_minor}.${candidate}" >/dev/null; do
  candidate="$((candidate + 1))"
done

effective="${next_major}.${next_minor}.${candidate}"

echo "APP_EFFECTIVE_VERSION=${effective}" >> "${GITHUB_ENV:-/dev/stdout}"
echo "Computed APP_EFFECTIVE_VERSION=${effective}"
echo "  base_version=${base_ver} (max_tag=${max_tag:-<none>}, cargo=${cargo_ver})"
echo "  bump_level=${BUMP_LEVEL}"
