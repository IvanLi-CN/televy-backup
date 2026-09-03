#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: assemble-universal.sh --version VERSION --arm64-app APP --x86_64-app APP --output-dir DIR" >&2
  exit 2
}
version=""; arm_app=""; x86_app=""; output_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift 2 ;;
    --arm64-app) arm_app="${2:-}"; shift 2 ;;
    --x86_64-app) x86_app="${2:-}"; shift 2 ;;
    --output-dir) output_dir="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$version" && -d "$arm_app" && -d "$x86_app" && -n "$output_dir" ]] || usage
mkdir -p "$output_dir"
universal_app="$output_dir/TelevyBackup.app"
rm -rf "$universal_app"
cp -R "$arm_app" "$universal_app"
for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper; do
  arm_binary="$arm_app/Contents/MacOS/$binary"
  x86_binary="$x86_app/Contents/MacOS/$binary"
  [[ -f "$arm_binary" && -f "$x86_binary" ]] || { echo "missing binary: $binary" >&2; exit 1; }
  lipo -create "$arm_binary" "$x86_binary" -output "$universal_app/Contents/MacOS/$binary"
done

# The copied arm64 bundle carries a thin-binary CodeResources seal. Remove it
# before signing the lipo outputs so the universal bundle gets a fresh seal.
rm -rf "$universal_app/Contents/_CodeSignature"
for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper; do
  codesign --force --sign - "$universal_app/Contents/MacOS/$binary"
done
codesign --force --sign - "$universal_app"
codesign --verify --deep --strict "$universal_app"

staging="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-universal.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/TelevyBackup"
cp -R "$universal_app" "$staging/TelevyBackup/"
ln -s /Applications "$staging/TelevyBackup/Applications"
hdiutil create -quiet -volname "TelevyBackup $version" -srcfolder "$staging/TelevyBackup" -format UDZO -ov "$output_dir/TelevyBackup-${version}.dmg"
echo "assembled universal app and TelevyBackup-${version}.dmg"
