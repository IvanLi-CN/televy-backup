#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
iconset_dir="${1:-$root_dir/assets/brand/macos/TelevyBackup.iconset}"
icns_file="${2:-$root_dir/macos/TelevyBackupApp/Resources/TelevyBackup.icns}"
bundle_dir="${3:-}"
asset_catalog_dir="${4:-$root_dir/assets/brand/macos/Assets.xcassets}"
preview_dir="${5:-$root_dir/assets/brand/macos/previews}"

command -v sips >/dev/null || { echo "sips is required" >&2; exit 2; }
command -v iconutil >/dev/null || { echo "iconutil is required" >&2; exit 2; }
command -v xcrun >/dev/null || { echo "xcrun is required" >&2; exit 2; }

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
[[ -s "$asset_catalog_dir/AppIcon.appiconset/Contents.json" ]] || {
  echo "missing AppIcon asset catalog: $asset_catalog_dir" >&2
  exit 1
}
[[ -d "$preview_dir" ]] || { echo "missing App Icon preview directory: $preview_dir" >&2; exit 1; }

python3 - "$asset_catalog_dir/AppIcon.appiconset/Contents.json" <<'PY'
import json
import sys

payload = json.load(open(sys.argv[1], encoding="utf-8"))
images = payload.get("images", [])
expected = {}
for appearance in ("default", "dark", "mono"):
    for name in ("16", "16@2x", "32", "32@2x", "128", "128@2x", "256", "256@2x", "512", "512@2x"):
        filename = f"AppIcon-{appearance}-{name}.png"
        expected[filename] = appearance

found = {entry.get("filename"): entry for entry in images}
if set(found) != set(expected):
    missing = sorted(set(expected) - set(found))
    extra = sorted(set(found) - set(expected))
    raise SystemExit(f"unexpected AppIcon catalog members: missing={missing}, extra={extra}")
for filename, appearance in expected.items():
    entry_appearances = found[filename].get("appearances", [])
    values = {item.get("value") for item in entry_appearances}
    if appearance == "default" and values:
        raise SystemExit(f"default AppIcon member has an appearance qualifier: {filename}")
    if appearance == "dark" and values != {"dark"}:
        raise SystemExit(f"dark AppIcon member has wrong appearance qualifier: {filename}")
    if appearance == "mono" and values != {"tinted"}:
        raise SystemExit(f"mono AppIcon member has wrong appearance qualifier: {filename}")
print("AppIcon catalog manifest verified: Default, Dark, and Tinted/Mono entries")
PY

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

roundtrip_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-icon-roundtrip.XXXXXX")"
catalog_compile_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-actool-verify.XXXXXX")"
trap 'rm -rf "$roundtrip_dir" "$catalog_compile_dir"' EXIT
iconutil -c iconset "$icns_file" -o "$roundtrip_dir/TelevyBackup.iconset" >/dev/null

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
  for entry in "${catalog_sizes[@]}"; do
    name="${entry%%:*}"
    size="${entry##*:}"
    file="$asset_catalog_dir/AppIcon.appiconset/AppIcon-$appearance-$name.png"
    [[ -s "$file" ]] || { echo "missing AppIcon catalog member: $file" >&2; exit 1; }
    width="$(sips -g pixelWidth "$file" | awk '/pixelWidth/ { print $2 }')"
    height="$(sips -g pixelHeight "$file" | awk '/pixelHeight/ { print $2 }')"
    [[ "$width" == "$size" && "$height" == "$size" ]] || {
      echo "wrong AppIcon catalog dimensions for $file: ${width}x${height}, expected ${size}x${size}" >&2
      exit 1
    }
  done
done

for appearance in default dark mono; do
  for size in 48 128 512; do
    for mask in rounded squircle circle; do
      preview="$preview_dir/app-icon-$appearance-$size-$mask.png"
      [[ -s "$preview" ]] || { echo "missing App Icon preview: $preview" >&2; exit 1; }
      width="$(sips -g pixelWidth "$preview" | awk '/pixelWidth/ { print $2 }')"
      height="$(sips -g pixelHeight "$preview" | awk '/pixelHeight/ { print $2 }')"
      [[ "$width" == "$size" && "$height" == "$size" ]] || {
        echo "wrong preview dimensions for $preview: ${width}x${height}" >&2
        exit 1
      }
    done
  done
done

xcrun actool \
  --compile "$catalog_compile_dir" \
  --platform macosx \
  --minimum-deployment-target 15.0 \
  --app-icon AppIcon \
  --output-partial-info-plist "$catalog_compile_dir/partial.plist" \
  "$asset_catalog_dir" >/dev/null
[[ -s "$catalog_compile_dir/Assets.car" ]] || { echo "actool did not produce Assets.car" >&2; exit 1; }
catalog_icon_name="$(/usr/bin/plutil -extract CFBundleIconName raw -o - "$catalog_compile_dir/partial.plist")"
[[ "$catalog_icon_name" == "AppIcon" ]] || { echo "actool omitted CFBundleIconName=AppIcon" >&2; exit 1; }

if [[ -n "$bundle_dir" ]]; then
  [[ -s "$bundle_dir/Contents/Resources/TelevyBackup.icns" ]] || {
    echo "bundle is missing Contents/Resources/TelevyBackup.icns" >&2
    exit 1
  }
  [[ -s "$bundle_dir/Contents/Resources/Assets.car" ]] || {
    echo "bundle is missing Contents/Resources/Assets.car" >&2
    exit 1
  }
  bundle_icon_name="$(/usr/bin/plutil -extract CFBundleIconName raw -o - "$bundle_dir/Contents/Info.plist")"
  [[ "$bundle_icon_name" == "AppIcon" ]] || {
    echo "bundle has unexpected CFBundleIconName: $bundle_icon_name" >&2
    exit 1
  }
  for resource in \
    televybackup-logo-ui.svg \
    televybackup-logo-ui-compact.svg \
    televybackup-logo-dark.svg \
    televybackup-logo-dark-compact.svg \
    televybackup-logo-template.svg; do
    [[ -s "$bundle_dir/Contents/Resources/Brand/$resource" ]] || {
      echo "bundle is missing Brand/$resource" >&2
      exit 1
    }
  done
fi

echo "app icon assets verified: iconset, ICNS, AppIcon catalog, and bundle contract"
