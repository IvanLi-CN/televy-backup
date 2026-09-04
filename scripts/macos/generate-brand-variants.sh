#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
source_svg="${1:-$root_dir/assets/brand/televybackup-logo.svg}"
brand_dir="$root_dir/assets/brand"

[[ -f "$source_svg" ]] || {
  echo "missing source SVG: $source_svg" >&2
  exit 1
}

mkdir -p "$brand_dir"

make_variant() {
  local output="$1"
  shift
  local temp
  temp="$(mktemp "${TMPDIR:-/tmp}/televybackup-brand.XXXXXX.svg")"
  trap 'rm -f "$temp"' RETURN
  sed "$@" "$source_svg" > "$temp"
  mv "$temp" "$output"
  trap - RETURN
}

make_variant "$brand_dir/televybackup-logo-ui.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/'

make_variant "$brand_dir/televybackup-logo-template.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/' \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #000000; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #000000; }/'

echo "generated brand variants in $brand_dir"
