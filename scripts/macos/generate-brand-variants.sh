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

make_variant_from() {
  local input="$1"
  local output="$2"
  shift 2
  local temp
  temp="$(mktemp "${TMPDIR:-/tmp}/televybackup-brand.XXXXXX")"
  trap 'rm -f "$temp"' RETURN
  if [[ $# -gt 0 ]]; then
    sed "$@" "$input" > "$temp"
  else
    sed -e '' "$input" > "$temp"
  fi
  mv "$temp" "$output"
  trap - RETURN
}

make_variant() {
  local output="$1"
  shift
  make_variant_from "$source_svg" "$output" "$@"
}

make_variant "$brand_dir/televybackup-logo-ui.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/'

make_variant "$brand_dir/televybackup-logo-ui-compact.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/' \
  -e 's/width="1254" height="1254" viewBox="0 0 1254 1254"/width="1000" height="1000" viewBox="125 125 1000 1000"/'

make_variant "$brand_dir/televybackup-logo-dark.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/' \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #bbc9d8; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #a2ceff; }/'

make_variant "$brand_dir/televybackup-logo-dark-compact.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/' \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #bbc9d8; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #a2ceff; }/' \
  -e 's/width="1254" height="1254" viewBox="0 0 1254 1254"/width="1000" height="1000" viewBox="125 125 1000 1000"/'

make_variant "$brand_dir/televybackup-logo-template.svg" \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: none; }/' \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #000000; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #000000; }/'

make_variant "$brand_dir/televybackup-logo-compact.svg" \
  -e 's/width="1254" height="1254" viewBox="0 0 1254 1254"/width="1000" height="1000" viewBox="125 125 1000 1000"/'

layers_dir="$brand_dir/macos/layers"
mkdir -p "$layers_dir/default" "$layers_dir/dark" "$layers_dir/mono"
make_variant "$layers_dir/default/televybackup-logo.svg"
make_variant "$layers_dir/default/televybackup-logo-compact.svg" \
  -e 's/width="1254" height="1254" viewBox="0 0 1254 1254"/width="1000" height="1000" viewBox="125 125 1000 1000"/'
make_variant "$layers_dir/dark/televybackup-logo.svg" \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #74869c; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #5aa9ff; }/' \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: #263238; }/'
make_variant "$layers_dir/dark/televybackup-logo-compact.svg" \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #74869c; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #5aa9ff; }/' \
  -e 's/\.canvas { fill: #ffffff; }/.canvas { fill: #263238; }/' \
  -e 's/width="1254" height="1254" viewBox="0 0 1254 1254"/width="1000" height="1000" viewBox="125 125 1000 1000"/'
make_variant "$layers_dir/mono/televybackup-logo.svg" \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #000000; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #000000; }/'
make_variant "$layers_dir/mono/televybackup-logo-compact.svg" \
  -e 's/\.disk { fill: #263238; }/.disk { fill: #000000; }/' \
  -e 's/\.wing { fill: #1677ff; }/.wing { fill: #000000; }/' \
  -e 's/width="1254" height="1254" viewBox="0 0 1254 1254"/width="1000" height="1000" viewBox="125 125 1000 1000"/'

echo "generated brand variants and macOS source groups in $brand_dir"
