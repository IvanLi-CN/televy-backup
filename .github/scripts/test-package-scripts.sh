#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

bash -n \
  "$root_dir/scripts/macos/build-app.sh" \
  "$root_dir/scripts/macos/package-release.sh" \
  "$root_dir/scripts/macos/assemble-universal.sh" \
  "$root_dir/scripts/macos/generate-release-manifest.sh" \
  "$root_dir/scripts/macos/verify-release-assets.sh" \
  "$root_dir/scripts/macos/generate-brand-variants.sh" \
  "$root_dir/scripts/macos/verify-brand-assets.sh" \
  "$root_dir/scripts/macos/generate-app-icon-assets.sh" \
  "$root_dir/scripts/macos/generate-app-icon-previews.sh" \
  "$root_dir/scripts/macos/verify-app-icon-assets.sh"

build_text="$(<"$root_dir/scripts/macos/build-app.sh")"
verify_brand_text="$(<"$root_dir/scripts/macos/verify-brand-assets.sh")"
[[ "$build_text" == *'verify-brand-assets.sh'* && "$verify_brand_text" == *'shared geometry'* ]] || {
  echo "brand asset verification is not wired into the build contract" >&2
  exit 1
}
[[ "$build_text" == *'CFBundleIconFile'* && "$build_text" == *'CFBundleIconName'* && "$build_text" == *'Assets.xcassets'* ]] || {
  echo "AppIcon bundle contract is not wired into build-app.sh" >&2
  exit 1
}
icon_text="$(<"$root_dir/scripts/macos/generate-app-icon-assets.sh")"
[[ "$icon_text" == *'icon_512x512@2x.png:1024'* && "$icon_text" == *'iconutil -c icns'* && "$icon_text" == *'AppIcon-dark-'* ]] || {
  echo "AppIcon generation contract is incomplete" >&2
  exit 1
}
verify_icon_text="$(<"$root_dir/scripts/macos/verify-app-icon-assets.sh")"
[[ "$verify_icon_text" == *'icon_16x16.png:16'* && "$verify_icon_text" == *'iconutil -c iconset'* && "$verify_icon_text" == *'Assets.car'* ]] || {
  echo "AppIcon verification contract is incomplete" >&2
  exit 1
}

package_text="$(<"$root_dir/scripts/macos/package-release.sh")"
[[ "$package_text" == *'--mode release|development'* ]]
[[ "$package_text" == *'product-version.py'* ]]
[[ "$package_text" == *'app_dest="$output_dir/TelevyBackup.app"'* ]]
[[ "$package_text" != *'--version'* ]]

version="$(tr -d '\n' < "$root_dir/VERSION")"
for asset in \
  "TelevyBackup-${version}.dmg" \
  "TelevyBackup-${version}-arm64.dmg" \
  "TelevyBackup-${version}-x86_64.dmg" \
  "televybackup-tools-${version}-arm64.tar.gz" \
  "televybackup-tools-${version}-x86_64.tar.gz"; do
  printf 'fixture %s\n' "$asset" > "$tmp_dir/$asset"
done

mkdir -p "$tmp_dir/scripts"
cp "$root_dir/scripts/product-version.py" "$tmp_dir/scripts/product-version.py"
git -C "$tmp_dir" init -q
git -C "$tmp_dir" config user.name test
git -C "$tmp_dir" config user.email test@example.com
printf '%s\n' "$version" > "$tmp_dir/VERSION"
git -C "$tmp_dir" add VERSION scripts
git -C "$tmp_dir" commit -qm fixture

bash "$root_dir/scripts/macos/generate-release-manifest.sh" \
  --mode release \
  --asset-dir "$tmp_dir" \
  --source-commit "$(git -C "$tmp_dir" rev-parse HEAD)" \
  --packaging-commit "$(git -C "$root_dir" rev-parse HEAD)" \
  --output "$tmp_dir/BUILD-MANIFEST.json"
bash "$root_dir/scripts/macos/verify-release-assets.sh" --mode release --asset-dir "$tmp_dir"

python3 - "$tmp_dir/BUILD-MANIFEST.json" "$version" <<'PY'
import json
import sys

payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload["release_version"] == sys.argv[2]
assert payload["signing"] == "ad-hoc"
assert len(payload["assets"]) == 5
PY

echo "package script contract tests passed"
