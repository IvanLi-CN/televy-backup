#!/usr/bin/env bash
set -euo pipefail

usage() { echo "usage: verify-release-assets.sh --version VERSION --asset-dir DIR" >&2; exit 2; }
version=""; asset_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift 2 ;;
    --asset-dir) asset_dir="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$version" && -d "$asset_dir" ]] || usage
required=("TelevyBackup-${version}.dmg" "TelevyBackup-${version}-arm64.dmg" "TelevyBackup-${version}-x86_64.dmg" "televybackup-tools-${version}-arm64.tar.gz" "televybackup-tools-${version}-x86_64.tar.gz" "SHA256SUMS" "BUILD-MANIFEST.json")
for name in "${required[@]}"; do
  [[ -s "$asset_dir/$name" ]] || { echo "missing or empty asset: $name" >&2; exit 1; }
done
grep -F "TelevyBackup-${version}.dmg" "$asset_dir/SHA256SUMS" >/dev/null
grep -F "televybackup-tools-${version}-arm64.tar.gz" "$asset_dir/SHA256SUMS" >/dev/null
python3 - "$asset_dir/BUILD-MANIFEST.json" "$version" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
assert manifest["release_version"] == sys.argv[2]
assert manifest["signing"] == "ad-hoc"
assert {"arm64", "x86_64", "universal2"}.issubset(set(manifest["architectures"]))
assert manifest["assets"]
PY
for app in "$asset_dir"/*.app; do
  [[ -d "$app" ]] || continue
  codesign --verify --deep --strict "$app"
  [[ -s "$app/Contents/Resources/TelevyBackup.icns" ]] || {
    echo "app bundle missing TelevyBackup.icns: $app" >&2
    exit 1
  }
  [[ -s "$app/Contents/Resources/Assets.car" ]] || {
    echo "app bundle missing Assets.car: $app" >&2
    exit 1
  }
  icon_file="$(/usr/bin/plutil -extract CFBundleIconFile raw -o - "$app/Contents/Info.plist")"
  [[ "$icon_file" == "TelevyBackup.icns" ]] || {
    echo "app bundle has unexpected CFBundleIconFile: $icon_file" >&2
    exit 1
  }
  icon_name="$(/usr/bin/plutil -extract CFBundleIconName raw -o - "$app/Contents/Info.plist")"
  [[ "$icon_name" == "AppIcon" ]] || {
    echo "app bundle has unexpected CFBundleIconName: $icon_name" >&2
    exit 1
  }
  for brand_asset in \
    televybackup-logo-ui.svg \
    televybackup-logo-ui-compact.svg \
    televybackup-logo-dark.svg \
    televybackup-logo-dark-compact.svg \
    televybackup-logo-template.svg; do
    [[ -s "$app/Contents/Resources/Brand/$brand_asset" ]] || {
      echo "app bundle missing Brand/$brand_asset: $app" >&2
      exit 1
    }
  done
  for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper; do
    info="$(lipo -info "$app/Contents/MacOS/$binary")"
    [[ "$info" == *arm64* && "$info" == *x86_64* ]] || { echo "universal binary missing slice: $binary" >&2; exit 1; }
  done
done
echo "release assets verified: ${#required[@]} files"
