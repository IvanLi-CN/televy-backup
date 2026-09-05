#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

bash -n "$root_dir/scripts/macos/package-release.sh" \
  "$root_dir/scripts/macos/assemble-universal.sh" \
  "$root_dir/scripts/macos/generate-brand-variants.sh" \
  "$root_dir/scripts/macos/verify-brand-assets.sh" \
  "$root_dir/scripts/macos/generate-app-icon-assets.sh" \
  "$root_dir/scripts/macos/generate-app-icon-previews.sh" \
  "$root_dir/scripts/macos/verify-app-icon-assets.sh" \
  "$root_dir/scripts/macos/verify-release-assets.sh" \
  "$root_dir/scripts/macos/generate-release-manifest.sh"
build_text="$(<"$root_dir/scripts/macos/build-app.sh")"
verify_brand_text="$(<"$root_dir/scripts/macos/verify-brand-assets.sh")"
[[ "$build_text" == *'verify-brand-assets.sh'* && "$verify_brand_text" == *'shared geometry'* ]] || {
  echo 'build must run the shared-geometry brand asset verifier' >&2
  exit 1
}
[[ "$build_text" == *'CFBundleIconFile'* && "$build_text" == *'CFBundleIconName'* && "$build_text" == *'Assets.xcassets'* ]] || {
  echo 'build script must declare the AppIcon catalog and TelevyBackup.icns fallback' >&2
  exit 1
}
icon_text="$(<"$root_dir/scripts/macos/generate-app-icon-assets.sh")"
[[ "$icon_text" == *'icon_512x512@2x.png:1024'* && "$icon_text" == *'iconutil -c icns'* && "$icon_text" == *'AppIcon-dark-'* ]] || {
  echo 'app icon generator must produce the complete iconset, appearance catalog, and ICNS' >&2
  exit 1
}
verify_icon_text="$(<"$root_dir/scripts/macos/verify-app-icon-assets.sh")"
[[ "$verify_icon_text" == *'icon_16x16.png:16'* && "$verify_icon_text" == *'iconutil -c iconset'* && "$verify_icon_text" == *'Assets.car'* ]] || {
  echo 'app icon verifier must check standard sizes, AppIcon catalog, and ICNS round-trip' >&2
  exit 1
}
assemble_text="$(<"$root_dir/scripts/macos/assemble-universal.sh")"
package_text="$(<"$root_dir/scripts/macos/package-release.sh")"
[[ "$package_text" == *'app_dest="$output_dir/TelevyBackup.app"'* ]] || {
  echo 'architecture package must stage the stable TelevyBackup.app name' >&2
  exit 1
}
[[ "$package_text" != *'TelevyBackup-${version}-${arch}.app'* ]] || {
  echo 'architecture package must not version the DMG app entry name' >&2
  exit 1
}
[[ "$assemble_text" == *'universal_app="$output_dir/TelevyBackup.app"'* ]] || {
  echo 'universal package must stage the stable TelevyBackup.app name' >&2
  exit 1
}
[[ "$assemble_text" != *'TelevyBackup-${version}.app'* ]] || {
  echo 'universal package must not version the DMG app entry name' >&2
  exit 1
}
[[ "$assemble_text" == *'rm -rf "$universal_app/Contents/_CodeSignature"'* ]] || {
  echo 'universal assembly must clear the copied thin-binary signature' >&2
  exit 1
}
[[ "$assemble_text" == *'codesign --force --sign - "$universal_app/Contents/MacOS/$binary"'* ]] || {
  echo 'universal assembly must sign each merged binary before signing the app' >&2
  exit 1
}
ruby -ryaml -e 'ARGV.each { |path| YAML.load_file(path) }' \
  .github/workflows/package-ci.yml \
  .github/workflows/release-backfill.yml
workflow_text="$(<"$root_dir/.github/workflows/package-ci.yml")"
[[ "$workflow_text" == *'dist/arm64/TelevyBackup.app'* && "$workflow_text" == *'dist/x86_64/TelevyBackup.app'* ]] || {
  echo 'package CI must verify the staged architecture app bundles' >&2
  exit 1
}

version="1.2.3-rc.abc1234"
for asset in \
  "TelevyBackup-${version}.dmg" \
  "TelevyBackup-${version}-arm64.dmg" \
  "TelevyBackup-${version}-x86_64.dmg" \
  "televybackup-tools-${version}-arm64.tar.gz" \
  "televybackup-tools-${version}-x86_64.tar.gz"; do
  printf 'fixture %s\n' "$asset" > "$tmp_dir/$asset"
done

bash "$root_dir/scripts/macos/generate-release-manifest.sh" \
  --version "$version" \
  --asset-dir "$tmp_dir" \
  --source-commit "0f283ce8ccbc30c56728c1d6c0366b76d8972772" \
  --packaging-commit "629426aab8f15614c0ac2b41a8ae305cf60f7f5c" \
  --output "$tmp_dir/BUILD-MANIFEST.json"
bash "$root_dir/scripts/macos/verify-release-assets.sh" --version "$version" --asset-dir "$tmp_dir"

python3 - "$tmp_dir/BUILD-MANIFEST.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1], encoding='utf-8'))
assert payload['source_commit'].startswith('0f283ce8')
assert payload['packaging_commit'].startswith('629426a')
assert len(payload['assets']) == 5
PY
echo 'package script contract tests passed'
