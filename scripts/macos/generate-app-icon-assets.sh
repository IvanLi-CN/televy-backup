#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
source_svg="${1:-$root_dir/assets/brand/televybackup-logo.svg}"
iconset_dir="${2:-$root_dir/assets/brand/macos/TelevyBackup.iconset}"
icns_output="${3:-$root_dir/macos/TelevyBackupApp/Resources/TelevyBackup.icns}"

[[ -f "$source_svg" ]] || {
  echo "missing source SVG: $source_svg" >&2
  exit 1
}
command -v sips >/dev/null || { echo "sips is required" >&2; exit 2; }
command -v iconutil >/dev/null || { echo "iconutil is required" >&2; exit 2; }

case "$iconset_dir" in
  "$root_dir"/*) ;;
  *) echo "iconset output must remain inside the repository" >&2; exit 2 ;;
esac
case "$icns_output" in
  "$root_dir"/*) ;;
  *) echo "ICNS output must remain inside the repository" >&2; exit 2 ;;
esac

mkdir -p "$(dirname "$iconset_dir")" "$(dirname "$icns_output")"
rm -rf "$iconset_dir"
mkdir -p "$iconset_dir"

sizes=(
  "icon_16x16.png:16"
  "icon_16x16@2x.png:32"
  "icon_32x32.png:32"
  "icon_32x32@2x.png:64"
  "icon_128x128.png:128"
  "icon_128x128@2x.png:256"
  "icon_256x256.png:256"
  "icon_256x256@2x.png:512"
  "icon_512x512.png:512"
  "icon_512x512@2x.png:1024"
)

for entry in "${sizes[@]}"; do
  name="${entry%%:*}"
  size="${entry##*:}"
  sips -s format png -Z "$size" "$source_svg" --out "$iconset_dir/$name" >/dev/null
done

rm -f "$icns_output"
iconutil -c icns "$iconset_dir" -o "$icns_output"
echo "generated $iconset_dir and $icns_output"
