#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: assemble-universal.sh --mode release|development --arm64-app APP --x86_64-app APP --output-dir DIR" >&2
  exit 2
}
mode=""; arm_app=""; x86_app=""; output_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:-}"; shift 2 ;;
    --arm64-app) arm_app="${2:-}"; shift 2 ;;
    --x86_64-app) x86_app="${2:-}"; shift 2 ;;
    --output-dir) output_dir="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$mode" && -d "$arm_app" && -d "$x86_app" && -n "$output_dir" ]] || usage
[[ "$mode" == "release" || "$mode" == "development" ]] || usage
root_dir="$(git rev-parse --show-toplevel)"
source_commit="$(git rev-parse HEAD)"
version="$(python3 "$root_dir/scripts/product-version.py" --mode "$mode" --source-sha "$source_commit")"
mkdir -p "$output_dir"
universal_app="$output_dir/TelevyBackup.app"
rm -rf "$universal_app"
cp -R "$arm_app" "$universal_app"
for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
  arm_binary="$arm_app/Contents/MacOS/$binary"
  x86_binary="$x86_app/Contents/MacOS/$binary"
  [[ -f "$arm_binary" && -f "$x86_binary" ]] || { echo "missing binary: $binary" >&2; exit 1; }
  lipo -create "$arm_binary" "$x86_binary" -output "$universal_app/Contents/MacOS/$binary"
done
chmod 755 "$universal_app/Contents/MacOS/"*

# The copied arm64 bundle carries a thin-binary CodeResources seal. Remove it
# before signing the lipo outputs so the universal bundle gets a fresh seal.
rm -rf "$universal_app/Contents/_CodeSignature"
for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
  codesign --force --sign - "$universal_app/Contents/MacOS/$binary"
done

access_relative_path="Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
arm_access_app="$arm_app/$access_relative_path"
x86_access_app="$x86_app/$access_relative_path"
[[ -d "$arm_access_app" && -d "$x86_access_app" ]] || {
  echo "missing embedded Snapshot Access app in one native build" >&2
  exit 1
}
universal_access_app="$universal_app/$access_relative_path"
arm_access_binary="$arm_access_app/Contents/MacOS/televybackup-snapshot-access"
x86_access_binary="$x86_access_app/Contents/MacOS/televybackup-snapshot-access"
universal_access_binary="$universal_access_app/Contents/MacOS/televybackup-snapshot-access"
arm_arches="$(lipo -info "$arm_access_binary")"
x86_arches="$(lipo -info "$x86_access_binary")"
if [[ "$arm_arches" == *"arm64"* && "$arm_arches" == *"x86_64"* && "$x86_arches" == *"arm64"* && "$x86_arches" == *"x86_64"* ]]; then
  cmp -s "$arm_access_app/Contents/Info.plist" "$x86_access_app/Contents/Info.plist" || {
    echo "reused Snapshot Access bundle metadata differs between native packages" >&2
    exit 1
  }
  cmp -s "$arm_access_binary" "$x86_access_binary" || {
    echo "reused Snapshot Access helper bytes differ between native packages" >&2
    exit 1
  }
  diff -qr "$arm_access_app" "$x86_access_app" >/dev/null || {
    echo "reused Snapshot Access bundle contents differ between native packages" >&2
    exit 1
  }
else
  lipo -create "$arm_access_binary" "$x86_access_binary" -output "$universal_access_binary"
  chmod 755 "$universal_access_binary"
  rm -rf "$universal_access_app/Contents/_CodeSignature"
  codesign --force --sign - "$universal_access_app"
  codesign --verify --strict "$universal_access_app"
fi

codesign --force --sign - "$universal_app"
codesign --verify --deep --strict "$universal_app"

staging="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-universal.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/TelevyBackup"
cp -R "$universal_app" "$staging/TelevyBackup/"
ln -s /Applications "$staging/TelevyBackup/Applications"
hdiutil create -quiet -volname "TelevyBackup $version" -srcfolder "$staging/TelevyBackup" -format UDZO -ov "$output_dir/TelevyBackup-${version}.dmg"
echo "assembled universal app and TelevyBackup-${version}.dmg"
