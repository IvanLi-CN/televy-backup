#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
iconset_dir="${1:-$root_dir/assets/brand/macos/TelevyBackup.iconset}"
icns_file="${2:-$root_dir/macos/TelevyBackupApp/Resources/TelevyBackup.icns}"
bundle_dir="${3:-}"

command -v sips >/dev/null || { echo "sips is required" >&2; exit 2; }
command -v iconutil >/dev/null || { echo "iconutil is required" >&2; exit 2; }

declare -a expected=(
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

[[ -d "$iconset_dir" ]] || { echo "missing iconset: $iconset_dir" >&2; exit 1; }
[[ -s "$icns_file" ]] || { echo "missing ICNS: $icns_file" >&2; exit 1; }

for entry in "${expected[@]}"; do
  name="${entry%%:*}"
  size="${entry##*:}"
  file="$iconset_dir/$name"
  [[ -s "$file" ]] || { echo "missing iconset member: $name" >&2; exit 1; }
  width="$(sips -g pixelWidth "$file" | awk '/pixelWidth/ { print $2 }')"
  height="$(sips -g pixelHeight "$file" | awk '/pixelHeight/ { print $2 }')"
  alpha="$(sips -g hasAlpha "$file" | awk '/hasAlpha/ { print $2 }')"
  [[ "$width" == "$size" && "$height" == "$size" ]] || {
    echo "wrong dimensions for $name: ${width}x${height}, expected ${size}x${size}" >&2
    exit 1
  }
  [[ "$alpha" == "yes" ]] || { echo "missing alpha channel: $name" >&2; exit 1; }
done

iconutil -c iconset "$icns_file" -o "${TMPDIR:-/tmp}/televybackup-icon-roundtrip.iconset" >/dev/null
rm -rf "${TMPDIR:-/tmp}/televybackup-icon-roundtrip.iconset"

if [[ -n "$bundle_dir" ]]; then
  [[ -s "$bundle_dir/Contents/Resources/TelevyBackup.icns" ]] || {
    echo "bundle is missing Contents/Resources/TelevyBackup.icns" >&2
    exit 1
  }
  for resource in \
    televybackup-logo-ui.svg \
    televybackup-logo-dark.svg \
    televybackup-logo-template.svg; do
    [[ -s "$bundle_dir/Contents/Resources/Brand/$resource" ]] || {
      echo "bundle is missing Brand/$resource" >&2
      exit 1
    }
  done
fi

echo "app icon assets verified"
