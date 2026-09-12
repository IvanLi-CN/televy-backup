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
  "$root_dir/scripts/macos/verify-component-identity.sh" \
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
[[ "$build_text" == *'LSUIElement'* && "$build_text" == *'<true/>'* ]] || {
  echo "LSUIElement menu-bar agent contract is missing from build-app.sh" >&2
  exit 1
}
[[ "$build_text" == *'TelevyBackupReleaseVersion'* && "$build_text" == *'TelevyBackupSourceCommit'* ]] || {
  echo "Snapshot Access bundle is missing the full product identity metadata" >&2
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
verify_release_text="$(<"$root_dir/scripts/macos/verify-release-assets.sh")"
[[ "$verify_release_text" == *'0$access_mode & 022'* ]] || {
  echo "Snapshot Access mode check must parse stat output as octal" >&2
  exit 1
}

package_text="$(<"$root_dir/scripts/macos/package-release.sh")"
[[ "$package_text" == *'--mode release|development'* ]]
[[ "$package_text" == *'product-version.py'* ]]
[[ "$package_text" == *'app_dest="$output_dir/TelevyBackup.app"'* ]]
[[ "$package_text" != *'access_dest="$output_dir/TelevyBackup Snapshot Access.app"'* ]]
[[ "$package_text" != *'REPLACE_WITH_SNAPSHOT_ACCESS_APP'* ]]
[[ "$package_text" != *'--version'* ]]
assemble_text="$(<"$root_dir/scripts/macos/assemble-universal.sh")"
grep -F 'chmod 755 "$universal_app/Contents/MacOS/"*' <<<"$assemble_text" >/dev/null || {
  echo "Universal main binaries must remain executable after lipo" >&2
  exit 1
}
grep -F 'chmod 755 "$universal_access_binary"' <<<"$assemble_text" >/dev/null || {
  echo "Universal Snapshot Access binary must remain executable after lipo" >&2
  exit 1
}
grep -F 'access_relative_path="Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"' <<<"$assemble_text" >/dev/null || {
  echo "Universal assembly must keep Snapshot Access nested in the main app" >&2
  exit 1
}
grep -F 'repackage_native_app "$arm_app" arm64' <<<"$assemble_text" >/dev/null || {
  echo "Native arm64 DMG must reuse the Universal Snapshot Access identity" >&2
  exit 1
}
grep -F 'repackage_native_app "$x86_app" x86_64' <<<"$assemble_text" >/dev/null || {
  echo "Native x86_64 DMG must reuse the Universal Snapshot Access identity" >&2
  exit 1
}

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
# This contract fixture runs on Linux and deliberately verifies metadata only;
# macOS package/release jobs run the default bundle checks.
bash "$root_dir/scripts/macos/verify-release-assets.sh" --mode release --asset-dir "$tmp_dir" --skip-bundle-checks

python3 - "$tmp_dir/BUILD-MANIFEST.json" "$version" <<'PY'
import json
import sys

payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload["release_version"] == sys.argv[2]
assert payload["signing"] == "ad-hoc"
assert len(payload["assets"]) == 5
assert payload["components"]["snapshot_mount_helper"]["compatible_component_versions"] == ["0.1.0", "0.9.8"]
PY

python3 - "$root_dir/packaging/macos/snapshot-components.lock.json" <<'PY'
import json
import sys

lock = json.load(open(sys.argv[1], encoding="utf-8"))
access = lock["components"]["snapshot_access"]
assert lock["signing"] == "ad-hoc"
assert lock["bootstrap_release_tag"] == "v0.9.8-rc.1"
assert access["bundle_id"] == "com.ivan.televybackup.snapshot-access"
assert access["component_version"] == "0.2.0"
assert access["protocol_version"] == 2
assert access["reuse_policy"] == "byte-identical-no-rebuild-no-lipo-no-resign"
identity = access["identity"]
assert identity["sha256"].startswith("BUILD-MANIFEST.json#/")
assert identity["artifact_sha256"].startswith("BUILD-MANIFEST.json#/")
assert identity["cdhash"].startswith("BUILD-MANIFEST.json#/")
assert identity["designated_requirement"].startswith("BUILD-MANIFEST.json#/")
mount_identity = lock["components"]["snapshot_mount_helper"]["identity"]
assert mount_identity["sha256"].startswith("BUILD-MANIFEST.json#/")
assert mount_identity["artifact_sha256"].startswith("BUILD-MANIFEST.json#/")
assert mount_identity["cdhash"].startswith("BUILD-MANIFEST.json#/")
assert mount_identity["designated_requirement"].startswith("BUILD-MANIFEST.json#/")
mount = lock["components"]["snapshot_mount_helper"]
assert mount["identity_source"] == "bundled-release-artifact"
assert mount["installed_observation"] == "manual-rc-acceptance-required"
assert mount["update_policy"] == "compatibility-check-only"
PY

echo "package script contract tests passed"
