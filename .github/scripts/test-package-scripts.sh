#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

bash -n "$root_dir/scripts/macos/package-release.sh" \
  "$root_dir/scripts/macos/assemble-universal.sh" \
  "$root_dir/scripts/macos/verify-release-assets.sh" \
  "$root_dir/scripts/macos/generate-release-manifest.sh"
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
