#!/usr/bin/env bash
set -euo pipefail

usage() { echo "usage: verify-release-assets.sh --mode release|development --asset-dir DIR [--skip-bundle-checks]" >&2; exit 2; }
mode=""; asset_dir=""; skip_bundle_checks=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:-}"; shift 2 ;;
    --asset-dir) asset_dir="${2:-}"; shift 2 ;;
    --skip-bundle-checks) skip_bundle_checks=true; shift ;;
    *) usage ;;
  esac
done
[[ -n "$mode" && -d "$asset_dir" ]] || usage
[[ "$mode" == "release" || "$mode" == "development" ]] || usage
root_dir="$(git rev-parse --show-toplevel)"
source_commit="$(git rev-parse HEAD)"
version="$(python3 "$root_dir/scripts/product-version.py" --mode "$mode" --source-sha "$source_commit")"

artifact_sha256() {
  python3 - "$1" <<'PY'
import hashlib, os, sys
path = sys.argv[1]
digest = hashlib.sha256()
if os.path.isfile(path):
    with open(path, 'rb') as handle:
        digest.update(handle.read())
else:
    for root, directories, files in os.walk(path, followlinks=False):
        directories.sort()
        files.sort()
        relative_root = os.path.relpath(root, path)
        if relative_root == '.':
            relative_root = ''
        for name in directories + files:
            entry = os.path.join(root, name)
            relative = os.path.join(relative_root, name)
            stat = os.lstat(entry)
            digest.update(b'entry\0' + relative.encode() + b'\0')
            digest.update(str(stat.st_mode).encode() + b'\0')
            if os.path.islink(entry):
                digest.update(b'link\0' + os.readlink(entry).encode() + b'\0')
            elif os.path.isfile(entry):
                with open(entry, 'rb') as handle:
                    digest.update(b'file\0' + handle.read())
            else:
                digest.update(b'other\0')
print(digest.hexdigest())
PY
}
required=("TelevyBackup-${version}.dmg" "TelevyBackup-${version}-arm64.dmg" "TelevyBackup-${version}-x86_64.dmg" "televybackup-tools-${version}-arm64.tar.gz" "televybackup-tools-${version}-x86_64.tar.gz" "SHA256SUMS" "BUILD-MANIFEST.json")
for name in "${required[@]}"; do
  [[ -s "$asset_dir/$name" ]] || { echo "missing or empty asset: $name" >&2; exit 1; }
done
grep -F "TelevyBackup-${version}.dmg" "$asset_dir/SHA256SUMS" >/dev/null
grep -F "televybackup-tools-${version}-arm64.tar.gz" "$asset_dir/SHA256SUMS" >/dev/null
(
  cd "$asset_dir"
  shasum -a 256 -c SHA256SUMS
)
python3 - "$asset_dir/BUILD-MANIFEST.json" "$version" "$root_dir/packaging/macos/snapshot-components.lock.json" "$asset_dir" "$skip_bundle_checks" <<'PY'
import hashlib, json, os, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
lock = json.load(open(sys.argv[3], encoding="utf-8"))
asset_dir = sys.argv[4]
assert manifest["release_version"] == sys.argv[2]
assert manifest["signing"] == "ad-hoc"
assert {"arm64", "x86_64", "universal2"}.issubset(set(manifest["architectures"]))
assert manifest["assets"]
expected_asset_names = {
    f"TelevyBackup-{sys.argv[2]}.dmg",
    f"TelevyBackup-{sys.argv[2]}-arm64.dmg",
    f"TelevyBackup-{sys.argv[2]}-x86_64.dmg",
    f"televybackup-tools-{sys.argv[2]}-arm64.tar.gz",
    f"televybackup-tools-{sys.argv[2]}-x86_64.tar.gz",
}
asset_records = {asset["name"]: asset for asset in manifest["assets"]}
assert set(asset_records) == expected_asset_names
for name in expected_asset_names:
    with open(os.path.join(asset_dir, name), "rb") as handle:
        data = handle.read()
    assert asset_records[name]["sha256"] == hashlib.sha256(data).hexdigest()
    assert asset_records[name]["bytes"] == len(data)
component = manifest["components"]["snapshot_access"]
locked = lock["components"]["snapshot_access"]
assert component["bundle_id"] == "com.ivan.televybackup.snapshot-access"
assert component["relative_path"] == "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
assert component["binary"] == locked["binary"]
assert component["component_version"] == locked["component_version"]
assert component["protocol_version"] == 2
assert component["reuse_policy"] == locked["reuse_policy"]
assert locked["identity"]["sha256"] == "BUILD-MANIFEST.json#/components/snapshot_access/sha256"
assert locked["identity"]["artifact_sha256"] == "BUILD-MANIFEST.json#/components/snapshot_access/artifact_sha256"
assert locked["identity"]["cdhash"] == "BUILD-MANIFEST.json#/components/snapshot_access/cdhash"
assert locked["identity"]["designated_requirement"] == "BUILD-MANIFEST.json#/components/snapshot_access/designated_requirement"
mount_component = manifest["components"]["snapshot_mount_helper"]
locked_mount_component = lock["components"]["snapshot_mount_helper"]
assert mount_component["label"] == locked_mount_component["label"]
assert mount_component["install_path"] == locked_mount_component["install_path"]
assert mount_component["component_version"] == locked_mount_component["component_version"]
assert mount_component["compatible_component_versions"] == locked_mount_component["compatible_component_versions"]
assert mount_component["protocol_version"] == locked_mount_component["protocol_version"]
assert mount_component["binary"] == locked_mount_component["binary"]
assert mount_component["source"] == locked_mount_component["source"]
assert mount_component["identity_source"] == locked_mount_component["identity_source"]
assert mount_component["installed_observation"] == locked_mount_component["installed_observation"]
assert mount_component["update_policy"] == locked_mount_component["update_policy"]
assert locked_mount_component["identity"]["sha256"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/sha256"
assert locked_mount_component["identity"]["artifact_sha256"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/artifact_sha256"
assert locked_mount_component["identity"]["cdhash"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/cdhash"
assert locked_mount_component["identity"]["designated_requirement"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/designated_requirement"
if sys.argv[5] != "true":
    assert mount_component["sha256"]
    assert mount_component["cdhash"]
    assert mount_component["designated_requirement"]
if sys.argv[2].endswith("-rc.1"):
    assert component["source"] in {"fresh-rc1-build", "rc1-universal-artifact"}
else:
    assert component["source"] == "rc1-universal-artifact"
PY
if [[ "$skip_bundle_checks" == true ]]; then
  echo "release metadata verified (bundle checks skipped)"
  exit 0
fi
app="$asset_dir/TelevyBackup.app"
[[ -d "$app" ]] || { echo "missing main app bundle: $app" >&2; exit 1; }
[[ ! -d "$asset_dir/TelevyBackup Snapshot Access.app" ]] || {
  echo "Snapshot Access must not be a top-level installable app" >&2
  exit 1
}
if [[ -d "$app" ]]; then
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
  for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
    info="$(lipo -info "$app/Contents/MacOS/$binary")"
    [[ "$info" == *arm64* && "$info" == *x86_64* ]] || { echo "universal binary missing slice: $binary" >&2; exit 1; }
  done
  [[ -x "$app/Contents/MacOS/televybackup-snapshot-mount-helper" ]] || { echo "snapshot mount helper missing" >&2; exit 1; }
  root_helper_binary="$app/Contents/MacOS/televybackup-snapshot-mount-helper"
  root_helper_signature="$(codesign -dvvv "$root_helper_binary" 2>&1 || true)"
  root_helper_sha256="$(shasum -a 256 "$root_helper_binary" | awk '{print $1}')"
  root_helper_cdhash="$(printf '%s\n' "$root_helper_signature" | awk -F= '/^CDHash=/{print $2}')"
  root_helper_requirement="$(codesign -d -r- "$root_helper_binary" 2>&1 | sed -n '/designated =>/p')"
  root_helper_artifact_sha256="$(artifact_sha256 "$root_helper_binary")"
  [[ -n "$root_helper_cdhash" && -n "$root_helper_requirement" ]] || {
    echo "snapshot mount helper signature identity is incomplete" >&2
    exit 1
  }
  python3 - "$asset_dir/BUILD-MANIFEST.json" "$root_helper_sha256" "$root_helper_artifact_sha256" "$root_helper_cdhash" "$root_helper_requirement" <<'PY'
import json, sys
component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_mount_helper"]
assert component["sha256"] == sys.argv[2]
assert component["artifact_sha256"] == sys.argv[3]
assert component["cdhash"] == sys.argv[4]
assert component["designated_requirement"] == sys.argv[5]
PY
  launch_agent="$app/Contents/Library/LaunchAgents/com.ivan.televybackup.snapshot-access.plist"
  [[ -s "$launch_agent" ]] || { echo "embedded Snapshot Access LaunchAgent missing" >&2; exit 1; }
  bundle_program="$(/usr/bin/plutil -extract BundleProgram raw -o - "$launch_agent")"
  [[ "$bundle_program" == "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access" ]] || {
    echo "Snapshot Access LaunchAgent does not use the fixed BundleProgram" >&2
    exit 1
  }
  ! /usr/bin/plutil -extract ProgramArguments xml1 -o - "$launch_agent" >/dev/null 2>&1 || {
    echo "Snapshot Access LaunchAgent must not contain an absolute ProgramArguments path" >&2
    exit 1
  }
  ! /usr/bin/grep -R -a -F "target/macos-app" "$app/Contents" >/dev/null || {
    echo "release bundle contains an old workspace launch path" >&2
    exit 1
  }
fi
access_app="$app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
[[ -d "$access_app" ]] || { echo "missing embedded Snapshot Access app bundle: $access_app" >&2; exit 1; }
if [[ -d "$access_app" ]]; then
  codesign --verify --strict "$access_app"
  bundle_id="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$access_app/Contents/Info.plist")"
  [[ "$bundle_id" == "com.ivan.televybackup.snapshot-access" ]] || { echo "unexpected Snapshot Access bundle id: $bundle_id" >&2; exit 1; }
  [[ -x "$access_app/Contents/MacOS/televybackup-snapshot-access" ]] || { echo "Snapshot Access executable missing" >&2; exit 1; }
  access_mode="$(stat -f '%Lp' "$access_app/Contents/MacOS/televybackup-snapshot-access")"
  [[ "$(( 0$access_mode & 022 ))" -eq 0 ]] || { echo "Snapshot Access executable is writable by group/other" >&2; exit 1; }
  signature="$(codesign -dvvv "$access_app" 2>&1 || true)"
  [[ "$signature" == *"Signature=adhoc"* ]] || { echo "Snapshot Access must use an ad-hoc signature" >&2; exit 1; }
  actual_sha256="$(shasum -a 256 "$access_app/Contents/MacOS/televybackup-snapshot-access" | awk '{print $1}')"
  actual_artifact_sha256="$(artifact_sha256 "$access_app")"
  actual_cdhash="$(printf '%s\n' "$signature" | awk -F= '/^CDHash=/{print $2}')"
  actual_requirement="$(codesign -d -r- "$access_app" 2>&1 | sed -n '/designated =>/p')"
  access_metadata="$("$access_app/Contents/MacOS/televybackup-snapshot-access" --component-metadata)"
  python3 - "$asset_dir/BUILD-MANIFEST.json" "$actual_sha256" "$actual_artifact_sha256" "$actual_cdhash" "$actual_requirement" "$access_metadata" <<'PY'
import json, sys
component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_access"]
assert component["sha256"] == sys.argv[2]
assert component["artifact_sha256"] == sys.argv[3]
assert component["cdhash"] == sys.argv[4]
assert component["designated_requirement"] == sys.argv[5]
metadata = json.loads(sys.argv[6])
assert component["bundle_id"] == metadata["bundleId"]
assert component["relative_path"] == metadata["relativePath"]
assert component["component_version"] == metadata["componentVersion"]
assert component["protocol_version"] == metadata["protocolVersion"]
PY
fi
verify_dmg_helper_identity() (
  set -euo pipefail
  local_dmg="$1"
  require_manifest_identity="$2"
  mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-helper-verify.XXXXXX")"
  mounted=false
  cleanup() {
    if [[ "$mounted" == true ]]; then
      hdiutil detach "$mount_point" >/dev/null 2>&1 || true
    fi
    rmdir "$mount_point" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT
  hdiutil attach -nobrowse -readonly -mountpoint "$mount_point" "$local_dmg" >/dev/null
  mounted=true
  app="$mount_point/TelevyBackup.app"
  [[ -d "$app" ]] || { echo "DMG is missing TelevyBackup.app: $local_dmg" >&2; exit 1; }
  codesign --verify --deep --strict "$app"
  app_arches="$(lipo -info "$app/Contents/MacOS/TelevyBackup")"
  expected_arches=universal
  case "$(basename "$local_dmg")" in
    TelevyBackup-*-arm64.dmg)
      expected_arches=arm64
      [[ "$app_arches" == *arm64* && "$app_arches" != *x86_64* ]] || { echo "arm64 DMG contains a non-arm64 main app: $local_dmg" >&2; exit 1; }
      ;;
    TelevyBackup-*-x86_64.dmg)
      expected_arches=x86_64
      [[ "$app_arches" == *x86_64* && "$app_arches" != *arm64* ]] || { echo "x86_64 DMG contains a non-x86_64 main app: $local_dmg" >&2; exit 1; }
      ;;
    TelevyBackup-*.dmg)
      [[ "$app_arches" == *arm64* && "$app_arches" == *x86_64* ]] || { echo "Universal DMG main app is missing a slice: $local_dmg" >&2; exit 1; }
      ;;
    *) echo "unexpected TelevyBackup DMG name: $local_dmg" >&2; exit 1 ;;
  esac
  for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
    info="$(lipo -info "$app/Contents/MacOS/$binary")"
    case "$expected_arches" in
      universal) [[ "$info" == *arm64* && "$info" == *x86_64* ]] || { echo "Universal DMG binary is missing a slice: $binary" >&2; exit 1; } ;;
      arm64) [[ "$info" == *arm64* && "$info" != *x86_64* ]] || { echo "arm64 DMG contains an unexpected binary architecture: $binary" >&2; exit 1; } ;;
      x86_64) [[ "$info" == *x86_64* && "$info" != *arm64* ]] || { echo "x86_64 DMG contains an unexpected binary architecture: $binary" >&2; exit 1; } ;;
    esac
  done
  helper="$mount_point/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
  [[ -d "$helper" ]] || { echo "DMG is missing embedded Snapshot Access: $local_dmg" >&2; exit 1; }
  codesign --verify --strict "$helper"
  bundle_id="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$helper/Contents/Info.plist")"
  [[ "$bundle_id" == "com.ivan.televybackup.snapshot-access" ]] || {
    echo "DMG contains an unexpected Snapshot Access bundle id: $local_dmg" >&2
    exit 1
  }
  signature="$(codesign -dvvv "$helper" 2>&1 || true)"
  [[ "$signature" == *"Signature=adhoc"* ]] || {
    echo "DMG Snapshot Access is not ad-hoc signed: $local_dmg" >&2
    exit 1
  }
  actual_sha256="$(shasum -a 256 "$helper/Contents/MacOS/televybackup-snapshot-access" | awk '{print $1}')"
  actual_artifact_sha256="$(artifact_sha256 "$helper")"
  actual_cdhash="$(printf '%s\n' "$signature" | awk -F= '/^CDHash=/{print $2}')"
  actual_requirement="$(codesign -d -r- "$helper" 2>&1 | sed -n '/designated =>/p')"
  helper_arches="$(lipo -info "$helper/Contents/MacOS/televybackup-snapshot-access")"
  # Snapshot Access is the identity-stable component. Native DMGs carry the exact same
  # Universal helper as the Universal DMG, even though their outer app is thin.
  [[ "$helper_arches" == *arm64* && "$helper_arches" == *x86_64* ]] || {
    echo "DMG Snapshot Access must remain Universal: $local_dmg" >&2
    exit 1
  }
  access_metadata="$("$helper/Contents/MacOS/televybackup-snapshot-access" --component-metadata)"
  [[ -n "$actual_cdhash" && -n "$actual_requirement" ]] || {
    echo "DMG Snapshot Access signature identity is incomplete: $local_dmg" >&2
    exit 1
  }
  if [[ "$require_manifest_identity" == true ]]; then
    python3 - "$asset_dir/BUILD-MANIFEST.json" "$actual_sha256" "$actual_artifact_sha256" "$actual_cdhash" "$actual_requirement" "$access_metadata" <<'PY'
import json, sys
component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_access"]
assert component["sha256"] == sys.argv[2]
assert component["artifact_sha256"] == sys.argv[3]
assert component["cdhash"] == sys.argv[4]
assert component["designated_requirement"] == sys.argv[5]
metadata = json.loads(sys.argv[6])
assert component["bundle_id"] == metadata["bundleId"]
assert component["relative_path"] == metadata["relativePath"]
assert component["component_version"] == metadata["componentVersion"]
assert component["protocol_version"] == metadata["protocolVersion"]
PY
  fi
  mounted=false
  hdiutil detach "$mount_point" >/dev/null
  echo "DMG Snapshot Access verified: $local_dmg"
)
check_dmg_layout() {
  local dmg="$1"
  local mount_point
  mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-verify.XXXXXX")"
  hdiutil attach -nobrowse -readonly -mountpoint "$mount_point" "$dmg" >/dev/null
  local top_level_apps=()
  while IFS= read -r app_path; do
    top_level_apps+=("$app_path")
  done < <(find "$mount_point" -maxdepth 1 -type d -name '*.app' -print)
  if [[ "${#top_level_apps[@]}" -ne 1 || "${top_level_apps[0]}" != "$mount_point/TelevyBackup.app" ]]; then
    hdiutil detach "$mount_point" >/dev/null 2>&1 || true
    rmdir "$mount_point" >/dev/null 2>&1 || true
    echo "DMG must contain exactly one top-level TelevyBackup.app: $dmg" >&2
    return 1
  fi
  if [[ -d "$mount_point/TelevyBackup Snapshot Access.app" ]]; then
    hdiutil detach "$mount_point" >/dev/null 2>&1 || true
    rmdir "$mount_point" >/dev/null 2>&1 || true
    echo "DMG contains a second top-level Snapshot Access app: $dmg" >&2
    return 1
  fi
  hdiutil detach "$mount_point" >/dev/null
  rmdir "$mount_point" >/dev/null 2>&1 || true
}
for dmg in "$asset_dir/TelevyBackup-${version}.dmg" "$asset_dir/TelevyBackup-${version}-arm64.dmg" "$asset_dir/TelevyBackup-${version}-x86_64.dmg"; do
  check_dmg_layout "$dmg"
  verify_dmg_helper_identity "$dmg" true
done
for tools_archive in "$asset_dir/televybackup-tools-${version}-arm64.tar.gz" "$asset_dir/televybackup-tools-${version}-x86_64.tar.gz"; do
  if tar -tzf "$tools_archive" | /usr/bin/grep -E '(^|/)(TelevyBackup Snapshot Access\.app|com\.ivan\.televybackup\.snapshot-access)' >/dev/null; then
    echo "tools archive contains the private Snapshot Access app or service" >&2
    exit 1
  fi
  expected_arches=arm64
  [[ "$tools_archive" == *-x86_64.tar.gz ]] && expected_arches=x86_64
  tools_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-tools-verify.XXXXXX")"
  tar -xzf "$tools_archive" -C "$tools_dir"
  for binary in televybackup televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
    info="$(lipo -info "$tools_dir/TelevyBackup Tools/bin/$binary")"
    if [[ "$expected_arches" == arm64 ]]; then
      [[ "$info" == *arm64* && "$info" != *x86_64* ]] || { echo "arm64 tools archive contains an unexpected binary architecture: $binary" >&2; exit 1; }
    else
      [[ "$info" == *x86_64* && "$info" != *arm64* ]] || { echo "x86_64 tools archive contains an unexpected binary architecture: $binary" >&2; exit 1; }
    fi
  done
  rm -rf "$tools_dir"
done
echo "release assets verified: ${#required[@]} files"
