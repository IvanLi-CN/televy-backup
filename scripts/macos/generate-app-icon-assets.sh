#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
source_svg="${1:-$root_dir/assets/brand/televybackup-logo.svg}"
iconset_dir="${2:-$root_dir/assets/brand/macos/TelevyBackup.iconset}"
icns_output="${3:-$root_dir/macos/TelevyBackupApp/Resources/TelevyBackup.icns}"
asset_catalog_dir="${4:-$root_dir/assets/brand/macos/Assets.xcassets}"
appiconset_dir="$asset_catalog_dir/AppIcon.appiconset"
compact_svg="$root_dir/assets/brand/televybackup-logo-compact.svg"
dark_svg="$root_dir/assets/brand/macos/layers/dark/televybackup-logo.svg"
mono_svg="$root_dir/assets/brand/macos/layers/mono/televybackup-logo.svg"

[[ -f "$source_svg" ]] || {
  echo "missing source SVG: $source_svg" >&2
  exit 1
}
for variant in "$compact_svg" "$dark_svg" "$mono_svg"; do
  [[ -f "$variant" ]] || { echo "missing App Icon source SVG: $variant" >&2; exit 1; }
done
command -v sips >/dev/null || { echo "sips is required" >&2; exit 2; }
command -v iconutil >/dev/null || { echo "iconutil is required" >&2; exit 2; }
command -v xcrun >/dev/null || { echo "xcrun is required" >&2; exit 2; }

case "$iconset_dir" in
  "$root_dir"/*) ;;
  *) echo "iconset output must remain inside the repository" >&2; exit 2 ;;
esac
case "$icns_output" in
  "$root_dir"/*) ;;
  *) echo "ICNS output must remain inside the repository" >&2; exit 2 ;;
esac
case "$asset_catalog_dir" in
  "$root_dir"/*) ;;
  *) echo "asset catalog output must remain inside the repository" >&2; exit 2 ;;
esac

mkdir -p "$(dirname "$iconset_dir")" "$(dirname "$icns_output")" "$appiconset_dir"
rm -rf "$iconset_dir"
mkdir -p "$iconset_dir"
[[ -s "$appiconset_dir/Contents.json" ]] || {
  echo "missing AppIcon catalog Contents.json: $appiconset_dir/Contents.json" >&2
  exit 1
}
rm -f "$appiconset_dir"/AppIcon-default-*.png "$appiconset_dir"/AppIcon-dark-*.png "$appiconset_dir"/AppIcon-mono-*.png

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
  icon_source="$source_svg"
  if (( size <= 64 )); then icon_source="$compact_svg"; fi
  sips -s format png -Z "$size" "$icon_source" --out "$iconset_dir/$name" >/dev/null
done

rm -f "$icns_output"
iconutil -c icns "$iconset_dir" -o "$icns_output"

catalog_sizes=(
  "16:16"
  "16@2x:32"
  "32:32"
  "32@2x:64"
  "128:128"
  "128@2x:256"
  "256:256"
  "256@2x:512"
  "512:512"
  "512@2x:1024"
)
for appearance in default dark mono; do
  case "$appearance" in
    default) appearance_source="$source_svg"; compact_source="$compact_svg" ;;
    dark) appearance_source="$dark_svg"; compact_source="$root_dir/assets/brand/macos/layers/dark/televybackup-logo-compact.svg" ;;
    mono) appearance_source="$mono_svg"; compact_source="$root_dir/assets/brand/macos/layers/mono/televybackup-logo-compact.svg" ;;
  esac
  for entry in "${catalog_sizes[@]}"; do
    name="${entry%%:*}"
    size="${entry##*:}"
    icon_source="$appearance_source"
    if (( size <= 64 )); then icon_source="$compact_source"; fi
    sips -s format png -Z "$size" "$icon_source" --out "$appiconset_dir/AppIcon-$appearance-$name.png" >/dev/null
  done
done

echo "generated $iconset_dir, $icns_output, and $appiconset_dir"
