#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: package-release.sh --mode release|development --arch arm64|x86_64 --output-dir DIR" >&2
  exit 2
}

mode=""
arch=""
output_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:-}"; shift 2 ;;
    --arch) arch="${2:-}"; shift 2 ;;
    --output-dir) output_dir="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$mode" && -n "$arch" && -n "$output_dir" ]] || usage
[[ "$mode" == "release" || "$mode" == "development" ]] || usage
[[ "$arch" == "arm64" || "$arch" == "x86_64" ]] || usage

root_dir="$(git rev-parse --show-toplevel)"
source_commit="$(git rev-parse HEAD)"
version="$(python3 "$root_dir/scripts/product-version.py" --mode "$mode" --source-sha "$source_commit")"
mkdir -p "$output_dir"
export TELEVYBACKUP_APP_VARIANT=prod
export TELEVYBACKUP_BUILD_MODE="$mode"
export TELEVYBACKUP_SOURCE_COMMIT="$source_commit"
if [[ -z "${TELEVYBACKUP_CARGO_TARGET:-}" ]]; then
  if [[ "$arch" == "arm64" ]]; then export TELEVYBACKUP_CARGO_TARGET=aarch64-apple-darwin; else export TELEVYBACKUP_CARGO_TARGET=x86_64-apple-darwin; fi
fi
export TELEVYBACKUP_CODESIGN_IDENTITY="-"

bash "$root_dir/scripts/macos/build-app.sh"
app_source="$root_dir/target/macos-app/TelevyBackup.app"
[[ -d "$app_source" ]] || { echo "missing app bundle: $app_source" >&2; exit 1; }

dmg_name="TelevyBackup-${version}-${arch}.dmg"
tools_name="televybackup-tools-${version}-${arch}.tar.gz"
app_dest="$output_dir/TelevyBackup.app"
rm -rf "$app_dest"
cp -R "$app_source" "$app_dest"

staging="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-package.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/TelevyBackup" "$staging/TelevyBackup Tools/bin" "$staging/TelevyBackup Tools/LaunchAgents"
cp -R "$app_dest" "$staging/TelevyBackup/"
ln -s /Applications "$staging/TelevyBackup/Applications"
hdiutil create -quiet -volname "TelevyBackup $version" -srcfolder "$staging/TelevyBackup" -format UDZO -ov "$output_dir/$dmg_name"

cp "$app_dest/Contents/MacOS/televybackup-cli" "$staging/TelevyBackup Tools/bin/televybackup"
cp "$app_dest/Contents/MacOS/televybackupd" "$staging/TelevyBackup Tools/bin/televybackupd"
cp "$app_dest/Contents/MacOS/televybackup-mtproto-helper" "$staging/TelevyBackup Tools/bin/televybackup-mtproto-helper"
chmod 755 "$staging/TelevyBackup Tools/bin/"*
cat > "$staging/TelevyBackup Tools/LaunchAgents/com.ivan.televybackup.daemon.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.ivan.televybackup.daemon</string>
  <key>ProgramArguments</key><array><string>REPLACE_WITH_INSTALLED_DAEMON</string></array>
  <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>
PLIST
cp "$root_dir/packaging/LICENSE.txt" "$staging/TelevyBackup Tools/"
cp "$root_dir/packaging/INSTALL.md" "$staging/TelevyBackup Tools/"
tar -czf "$output_dir/$tools_name" -C "$staging" "TelevyBackup Tools"

echo "packaged $dmg_name and $tools_name"
