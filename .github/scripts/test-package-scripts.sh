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
  "$root_dir/scripts/macos/verify-release-assets.sh"

package_text="$(<"$root_dir/scripts/macos/package-release.sh")"
[[ "$package_text" == *'--mode release|development'* ]]
[[ "$package_text" == *'product-version.py'* ]]
[[ "$package_text" == *'app_dest="$output_dir/TelevyBackup.app"'* ]]
[[ "$package_text" != *'--version'* ]]

version="0.9.2"
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

python3 - "$tmp_dir/BUILD-MANIFEST.json" <<'PY'
import json
import sys

payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload["release_version"] == "0.9.2"
assert payload["signing"] == "ad-hoc"
assert len(payload["assets"]) == 5
PY

echo "package script contract tests passed"
