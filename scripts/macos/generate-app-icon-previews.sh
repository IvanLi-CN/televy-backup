#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
iconset_dir="${1:-$root_dir/assets/brand/macos/TelevyBackup.iconset}"
preview_dir="${2:-$root_dir/assets/brand/macos/previews}"
asset_catalog_dir="${3:-$root_dir/assets/brand/macos/Assets.xcassets}"

command -v magick >/dev/null || { echo "magick is required" >&2; exit 2; }
[[ -d "$iconset_dir" ]] || { echo "missing iconset: $iconset_dir" >&2; exit 1; }
[[ -s "$asset_catalog_dir/AppIcon.appiconset/Contents.json" ]] || {
  echo "missing AppIcon asset catalog: $asset_catalog_dir" >&2
  exit 1
}
case "$preview_dir" in
  "$root_dir"/*) ;;
  *) echo "preview output must remain inside the repository" >&2; exit 2 ;;
esac

mkdir -p "$preview_dir"
rm -f "$preview_dir"/app-icon-*.png

for appearance in default dark mono; do
  for size in 16 32 48 128 512; do
    case "$size" in
      16) source="$asset_catalog_dir/AppIcon.appiconset/AppIcon-$appearance-16.png" ;;
      32) source="$asset_catalog_dir/AppIcon.appiconset/AppIcon-$appearance-32.png" ;;
      48) source="$asset_catalog_dir/AppIcon.appiconset/AppIcon-$appearance-32@2x.png" ;;
      128) source="$asset_catalog_dir/AppIcon.appiconset/AppIcon-$appearance-128.png" ;;
      512) source="$asset_catalog_dir/AppIcon.appiconset/AppIcon-$appearance-512.png" ;;
    esac
    [[ -s "$source" ]] || { echo "missing preview source: $source" >&2; exit 1; }
    magick "$source" -resize "${size}x${size}" "$preview_dir/app-icon-$appearance-$size.png"
    if [[ "$appearance" == default ]]; then
      cp "$preview_dir/app-icon-default-$size.png" "$preview_dir/app-icon-$size.png"
    fi
  done
done

mask_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-icon-masks.XXXXXX")"
trap 'rm -rf "$mask_dir"' EXIT
for size in 48 128 512; do
  for mask in rounded squircle circle; do
    mask_file="$mask_dir/$size-$mask.png"
    case "$mask" in
      rounded)
        radius=$((size * 20 / 100))
        magick -size "${size}x${size}" xc:none -fill white \
          -draw "roundrectangle 0,0 $((size - 1)),$((size - 1)) $radius,$radius" "$mask_file"
        ;;
      squircle)
        radius=$((size * 30 / 100))
        magick -size "${size}x${size}" xc:none -fill white \
          -draw "roundrectangle 0,0 $((size - 1)),$((size - 1)) $radius,$radius" "$mask_file"
        ;;
      circle)
        magick -size "${size}x${size}" xc:none -fill white \
          -draw "ellipse $((size / 2)),$((size / 2)) $((size / 2)),$((size / 2)) 0,360" "$mask_file"
        ;;
    esac
    for appearance in default dark mono; do
      magick "$preview_dir/app-icon-$appearance-$size.png" "$mask_file" \
        -alpha set -compose DstIn -composite "$preview_dir/app-icon-$appearance-$size-$mask.png"
      if [[ "$appearance" == default ]]; then
        cp "$preview_dir/app-icon-default-$size-$mask.png" "$preview_dir/app-icon-$size-$mask.png"
      fi
    done
  done
done

echo "generated app icon previews in $preview_dir"
