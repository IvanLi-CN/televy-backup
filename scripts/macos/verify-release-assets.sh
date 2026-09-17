#!/usr/bin/env bash
set -euo pipefail

usage() { echo "usage: verify-release-assets.sh --mode release|development --asset-dir DIR --expected-source-commit SHA --expected-packaging-commit SHA [--skip-bundle-checks]" >&2; exit 2; }
mode=""; asset_dir=""; expected_source_commit=""; expected_packaging_commit=""; skip_bundle_checks=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:-}"; shift 2 ;;
    --asset-dir) asset_dir="${2:-}"; shift 2 ;;
    --expected-source-commit) expected_source_commit="${2:-}"; shift 2 ;;
    --expected-packaging-commit) expected_packaging_commit="${2:-}"; shift 2 ;;
    --skip-bundle-checks) skip_bundle_checks=true; shift ;;
    *) usage ;;
  esac
done
[[ -n "$mode" && -d "$asset_dir" && -n "$expected_source_commit" && -n "$expected_packaging_commit" ]] || usage
[[ "$mode" == "release" || "$mode" == "development" ]] || usage
[[ "$expected_source_commit" =~ ^[0-9a-fA-F]{40}$ ]] || {
  echo "expected source commit must be a 40-character SHA" >&2
  exit 2
}
[[ "$expected_packaging_commit" =~ ^[0-9a-fA-F]{40}$ ]] || {
  echo "expected packaging commit must be a 40-character SHA" >&2
  exit 2
}
root_dir="$(git rev-parse --show-toplevel)"
metadata_verifier="$root_dir/scripts/macos/verify-dmg-metadata.py"
requirement_normalizer="$root_dir/scripts/macos/normalize-designated-requirement.py"
read_designated_requirement() {
  codesign -d -r- "$1" 2>&1 | python3 "$requirement_normalizer"
}
verify_nested_helper_path() {
  local app="$1"
  local helper="$2"
  [[ ! -L "$helper" && -d "$helper" ]] || {
    echo "embedded Snapshot Access path must be a real directory: $helper" >&2
    exit 1
  }
  local app_real
  local helper_real
  app_real="$(cd "$app" && pwd -P)"
  helper_real="$(cd "$helper" && pwd -P)"
  [[ "$helper_real" == "$app_real/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app" ]] || {
    echo "embedded Snapshot Access path escapes the main app bundle: $helper" >&2
    exit 1
  }
  local binary="$helper/Contents/MacOS/televybackup-snapshot-access"
  [[ ! -L "$binary" && -f "$binary" ]] || {
    echo "embedded Snapshot Access executable must be a real file: $binary" >&2
    exit 1
  }
}
prepare_evidence_path() {
  local path="$1"
  [[ ! -L "$path" ]] || {
    echo "DMG evidence path must not be a symlink: $path" >&2
    exit 1
  }
  rm -f "$path"
}
if [[ -n "${DMG_EVIDENCE_FILE:-}" ]]; then
  prepare_evidence_path "$DMG_EVIDENCE_FILE"
fi
source_commit="$(git rev-parse HEAD)"
version="$(python3 "$root_dir/scripts/product-version.py" --mode "$mode" --source-sha "$source_commit")"

artifact_sha256() {
  python3 - "$1" <<'PY'
import hashlib, os, stat as stat_module, sys
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
            entry_stat = os.lstat(entry)
            if stat_module.S_ISLNK(entry_stat.st_mode):
                permissions = 0o777
            elif stat_module.S_ISDIR(entry_stat.st_mode) or entry_stat.st_mode & 0o111:
                permissions = 0o755
            else:
                permissions = 0o644
            mode = (entry_stat.st_mode & ~0o777) | permissions
            digest.update(b'entry\0' + relative.encode() + b'\0')
            digest.update(str(mode).encode() + b'\0')
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
emit_dmg_event() {
  local event_json
  event_json="$(python3 - "$@" <<'PY'
import json
import sys

event, dmg, mount_point, device = sys.argv[1:]
print(json.dumps({
    "device": device,
    "dmg": dmg,
    "event": event,
    "mount_point": mount_point,
}, sort_keys=True))
PY
)"
  printf '%s\n' "$event_json"
  if [[ -n "${DMG_EVIDENCE_FILE:-}" ]]; then
    printf '%s\n' "$event_json" >> "$DMG_EVIDENCE_FILE"
  fi
}
resolve_device_for_mount() {
  local device
  device="$(hdiutil info -plist 2>/dev/null | python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.stdin.buffer.read())
entities = list(payload.get("system-entities", []))
for image in payload.get("images", []):
    entities.extend(image.get("system-entities", []))
for entity in entities:
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"])
        raise SystemExit(0)' "$1" 2>/dev/null || true
  )"
  if [[ -n "$device" ]]; then
    printf '%s\n' "$device"
    return 0
  fi
  diskutil info -plist "$1" 2>/dev/null | python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.stdin.buffer.read())
mount = payload.get("MountPoint") or payload.get("mount-point")
device = payload.get("DeviceNode") or payload.get("dev-entry")
if mount == expected_mount and device:
    print(device)' "$1" 2>/dev/null || true
}

attach_dmg_readonly() {
  local dmg="$1"
  local mount_point="$2"
  ATTACHED_DEVICE=""
  ATTACHED_MOUNT=""
  local attached_mount_from_plist
  local attach_plist
  local attach_status=0
  if attach_plist="$(hdiutil attach -plist -nobrowse -readonly -mountpoint "$mount_point" "$dmg")"; then
    attach_status=0
  else
    attach_status=$?
  fi
  ATTACHED_MOUNT="$mount_point"
  if (( attach_status != 0 )); then
    read -r ATTACHED_DEVICE ATTACHED_MOUNT < <(
      python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.argv[2].encode())
for entity in payload.get("system-entities", []):
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
print("", "")' "$mount_point" "$attach_plist" 2>/dev/null || true
    )
    if [[ -z "$ATTACHED_DEVICE" ]]; then
      ATTACHED_DEVICE="$(resolve_device_for_mount "$mount_point")"
    fi
    ATTACHED_MOUNT="$mount_point"
    echo "hdiutil attach failed for $dmg (status $attach_status); cleanup will detach $ATTACHED_DEVICE" >&2
    return "$attach_status"
  fi
  read -r ATTACHED_DEVICE ATTACHED_MOUNT < <(
    python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.argv[2].encode())
for entity in payload.get("system-entities", []):
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
print("", "")' "$mount_point" "$attach_plist"
  )
  if [[ -z "$ATTACHED_DEVICE" ]]; then
    read -r ATTACHED_DEVICE ATTACHED_MOUNT < <(
      hdiutil info -plist | python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.stdin.buffer.read())
entities = list(payload.get("system-entities", []))
for image in payload.get("images", []):
    entities.extend(image.get("system-entities", []))
for entity in entities:
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
print("", "")' "$mount_point"
    )
  fi
  attached_mount_from_plist="$ATTACHED_MOUNT"
  ATTACHED_MOUNT="$mount_point"
  [[ "$attached_mount_from_plist" == "$mount_point" && -n "$ATTACHED_DEVICE" ]] || {
    echo "hdiutil attach plist did not resolve an exact device: $dmg" >&2
    return 1
  }
  emit_dmg_event dmg_attach "$dmg" "$ATTACHED_MOUNT" "$ATTACHED_DEVICE"
}

detach_dmg_exact() {
  local dmg="$1"
  local mount_point="$2"
  local device="$3"
  hdiutil detach "$device"
  emit_dmg_event dmg_detach "$dmg" "$mount_point" "$device"
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
python3 - "$asset_dir/BUILD-MANIFEST.json" "$version" "$root_dir/packaging/macos/snapshot-components.lock.json" "$asset_dir" "$root_dir/assets/brand/macos/dmg/layout.json" "$skip_bundle_checks" "$expected_source_commit" "$expected_packaging_commit" <<'PY'
import hashlib, json, os, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
lock = json.load(open(sys.argv[3], encoding="utf-8"))
asset_dir = sys.argv[4]
layout_path = sys.argv[5]
def require(condition, message):
    if not condition:
        raise SystemExit(message)

layout = json.load(open(layout_path, encoding="utf-8"))
def canonical_json(value):
    return json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(',', ':')).encode('utf-8')

resource_digests = {}
for name, expected in layout["asset_digests"].items():
    resource_path = os.path.join(os.path.dirname(layout_path), name)
    with open(resource_path, "rb") as handle:
        actual = hashlib.sha256(handle.read()).hexdigest()
    require(actual == expected, f"DMG layout resource digest mismatch: {name}")
    resource_digests[name] = actual

expected_dmg_layout = {
    "schema_version": layout["schema_version"],
    "builder": layout["builder"],
    "format": layout["format"],
    "filesystem": layout["filesystem"],
    "window": layout["window"],
    "icon_size": layout["icon_size"],
    "icon_locations": layout["icon_locations"],
    "overlay": layout["overlay"],
    "resources": {
        "background": layout["background"],
        "overlay": layout["overlay_asset"],
        "composed_background": layout["composed_background"],
        "digests": resource_digests,
    },
    "hidden_resource_allowlist": sorted(layout["hidden_resource_allowlist"]),
    "symlinks": layout["symlinks"],
}
expected_dmg_layout["semantic_layout_digest"] = hashlib.sha256(canonical_json(expected_dmg_layout)).hexdigest()
require(manifest["dmg_layout"] == expected_dmg_layout, "manifest DMG layout contract mismatch")
require(expected_dmg_layout["builder"] == {"name": "dmgbuild", "version": "1.6.7"}, "DMG builder is not pinned")
require(expected_dmg_layout["format"] == "UDZO", "DMG format is not UDZO")

require(manifest["release_version"] == sys.argv[2], "manifest release version mismatch")
require(manifest.get("source_commit") == sys.argv[7], "manifest source_commit does not match expected source commit")
require(manifest.get("packaging_commit") == sys.argv[8], "manifest packaging_commit does not match expected packaging commit")
require(manifest["signing"] == "ad-hoc", "manifest signing mode mismatch")
require({"arm64", "x86_64", "universal2"}.issubset(set(manifest["architectures"])), "manifest architectures are incomplete")
require(manifest["assets"], "manifest assets are missing")
expected_asset_names = {
    f"TelevyBackup-{sys.argv[2]}.dmg",
    f"TelevyBackup-{sys.argv[2]}-arm64.dmg",
    f"TelevyBackup-{sys.argv[2]}-x86_64.dmg",
    f"televybackup-tools-{sys.argv[2]}-arm64.tar.gz",
    f"televybackup-tools-{sys.argv[2]}-x86_64.tar.gz",
}
manifest_asset_names = [asset.get("name") for asset in manifest["assets"]]
require(len(manifest_asset_names) == len(set(manifest_asset_names)), "manifest contains duplicate asset names")
asset_records = {asset["name"]: asset for asset in manifest["assets"]}
require(set(asset_records) == expected_asset_names, "manifest asset names mismatch")
checksum_records = {}
checksum_names = []
for line in open(os.path.join(asset_dir, "SHA256SUMS"), encoding="utf-8"):
    fields = line.strip().split(maxsplit=1)
    if not fields:
        continue
    require(len(fields) == 2, "malformed SHA256SUMS entry")
    name = fields[1].lstrip("*")
    checksum_names.append(name)
    checksum_records[name] = fields[0]
require(len(checksum_names) == len(set(checksum_names)), "SHA256SUMS contains duplicate asset names")
require(set(checksum_records) == expected_asset_names, "SHA256SUMS asset names mismatch")
for name in expected_asset_names:
    with open(os.path.join(asset_dir, name), "rb") as handle:
        data = handle.read()
    digest = hashlib.sha256(data).hexdigest()
    require(asset_records[name]["sha256"] == digest, f"asset digest mismatch: {name}")
    require(checksum_records[name] == digest, f"SHA256SUMS digest mismatch: {name}")
    require(asset_records[name]["bytes"] == len(data), f"asset size mismatch: {name}")
for name, record in asset_records.items():
    if name.endswith(".dmg"):
        require(record.get("dmg_layout_digest") == expected_dmg_layout["semantic_layout_digest"], f"DMG layout digest mismatch: {name}")
component = manifest["components"]["snapshot_access"]
locked = lock["components"]["snapshot_access"]
require(component["bundle_id"] == "com.ivan.televybackup.snapshot-access", "Snapshot Access bundle id mismatch")
require(component["relative_path"] == "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app", "Snapshot Access path mismatch")
require(component["binary"] == locked["binary"], "Snapshot Access binary mismatch")
require(component["component_version"] == locked["component_version"], "Snapshot Access component version mismatch")
require(component["protocol_version"] == 2, "Snapshot Access protocol version mismatch")
require(component["reuse_policy"] == locked["reuse_policy"], "Snapshot Access reuse policy mismatch")
require(locked["identity"]["sha256"] == "BUILD-MANIFEST.json#/components/snapshot_access/sha256", "Snapshot Access SHA-256 lock reference mismatch")
require(locked["identity"]["artifact_sha256"] == "BUILD-MANIFEST.json#/components/snapshot_access/artifact_sha256", "Snapshot Access artifact lock reference mismatch")
require(locked["identity"]["cdhash"] == "BUILD-MANIFEST.json#/components/snapshot_access/cdhash", "Snapshot Access CDHash lock reference mismatch")
require(locked["identity"]["designated_requirement"] == "BUILD-MANIFEST.json#/components/snapshot_access/designated_requirement", "Snapshot Access requirement lock reference mismatch")
mount_component = manifest["components"]["snapshot_mount_helper"]
locked_mount_component = lock["components"]["snapshot_mount_helper"]
require(mount_component["label"] == locked_mount_component["label"], "mount helper label mismatch")
require(mount_component["install_path"] == locked_mount_component["install_path"], "mount helper install path mismatch")
require(mount_component["component_version"] == locked_mount_component["component_version"], "mount helper component version mismatch")
require(mount_component["compatible_component_versions"] == locked_mount_component["compatible_component_versions"], "mount helper compatible versions mismatch")
require(mount_component["protocol_version"] == locked_mount_component["protocol_version"], "mount helper protocol version mismatch")
require(mount_component["binary"] == locked_mount_component["binary"], "mount helper binary mismatch")
require(mount_component["source"] == locked_mount_component["source"], "mount helper source mismatch")
require(mount_component["identity_source"] == locked_mount_component["identity_source"], "mount helper identity source mismatch")
require(mount_component["installed_observation"] == locked_mount_component["installed_observation"], "mount helper observation mismatch")
require(mount_component["update_policy"] == locked_mount_component["update_policy"], "mount helper update policy mismatch")
require(locked_mount_component["identity"]["sha256"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/sha256", "mount helper SHA-256 lock reference mismatch")
require(locked_mount_component["identity"]["artifact_sha256"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/artifact_sha256", "mount helper artifact lock reference mismatch")
require(locked_mount_component["identity"]["cdhash"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/cdhash", "mount helper CDHash lock reference mismatch")
require(locked_mount_component["identity"]["designated_requirement"] == "BUILD-MANIFEST.json#/components/snapshot_mount_helper/designated_requirement", "mount helper requirement lock reference mismatch")
if sys.argv[6] != "true":
    require(mount_component["sha256"], "mount helper SHA-256 is missing")
    require(mount_component["cdhash"], "mount helper CDHash is missing")
    require(mount_component["designated_requirement"], "mount helper designated requirement is missing")
if component["source"] == "one-time-bootstrap-universal-build":
    require(component["reuse_policy"] == "byte-identical-no-rebuild-no-lipo-no-resign", "bootstrap reuse policy mismatch")
elif sys.argv[2].endswith("-rc.1"):
    require(component["source"] in {"fresh-rc1-build", "rc1-universal-artifact"}, "RC1 helper source is invalid")
else:
    require(component["source"] == "rc1-universal-artifact", "reused helper source is invalid")
PY
if [[ "$skip_bundle_checks" == true ]]; then
  echo "release metadata verified (bundle checks skipped)"
  exit 0
fi
app="$asset_dir/TelevyBackup.app"
[[ ! -L "$app" && -d "$app" ]] || { echo "missing main app bundle or symlinked app: $app" >&2; exit 1; }
[[ ! -d "$asset_dir/TelevyBackup Snapshot Access.app" ]] || {
  echo "Snapshot Access must not be a top-level installable app" >&2
  exit 1
}
bundle_id="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")"
[[ "$bundle_id" == "com.ivan.televybackup" ]] || {
  echo "release asset must use the prod app bundle id: $bundle_id" >&2
  exit 1
}
if [[ -d "$app" ]]; then
  codesign --verify --deep --strict "$app"
  app_signature="$(codesign -dvvv "$app" 2>&1 || true)"
  [[ "$app_signature" == *"Signature=adhoc"* ]] || { echo "main app must use an ad-hoc signature" >&2; exit 1; }
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
    [[ -x "$app/Contents/MacOS/$binary" ]] || { echo "main app binary is not executable: $binary" >&2; exit 1; }
    info="$(lipo -info "$app/Contents/MacOS/$binary")"
    [[ "$info" == *arm64* && "$info" == *x86_64* ]] || { echo "universal binary missing slice: $binary" >&2; exit 1; }
  done
  root_helper_binary="$app/Contents/MacOS/televybackup-snapshot-mount-helper"
  root_helper_signature="$(codesign -dvvv "$root_helper_binary" 2>&1 || true)"
  [[ "$root_helper_signature" == *"Signature=adhoc"* ]] || { echo "snapshot mount helper must use an ad-hoc signature" >&2; exit 1; }
  root_helper_sha256="$(shasum -a 256 "$root_helper_binary" | awk '{print $1}')"
  root_helper_cdhash="$(printf '%s\n' "$root_helper_signature" | awk -F= '/^CDHash=/{print $2}')"
  root_helper_requirement="$(read_designated_requirement "$root_helper_binary")"
  root_helper_artifact_sha256="$(artifact_sha256 "$root_helper_binary")"
  [[ -n "$root_helper_cdhash" && -n "$root_helper_requirement" ]] || {
    echo "snapshot mount helper signature identity is incomplete" >&2
    exit 1
  }
  python3 - "$asset_dir/BUILD-MANIFEST.json" "$root_helper_sha256" "$root_helper_artifact_sha256" "$root_helper_cdhash" "$root_helper_requirement" <<'PY'
import json, re, sys
def require(condition, message):
    if not condition:
        raise SystemExit(message)

component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_mount_helper"]
require(component["sha256"] == sys.argv[2], "mount helper SHA-256 mismatch")
require(component["artifact_sha256"] == sys.argv[3], "mount helper artifact digest mismatch")
requirement_cdhashes = {
    value.lower()
    for value in re.findall(r'\bcdhash\s+H"([0-9A-Fa-f]+)"', sys.argv[5])
}
require(requirement_cdhashes, "mount helper designated requirement has no CDHash identities")
require({component["cdhash"].lower(), sys.argv[4].lower()} <= requirement_cdhashes, "mount helper CDHash mismatch")
require(component["designated_requirement"] == sys.argv[5], "mount helper designated requirement mismatch")
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
verify_nested_helper_path "$app" "$access_app"
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
  actual_requirement="$(read_designated_requirement "$access_app")"
  access_metadata="$("$access_app/Contents/MacOS/televybackup-snapshot-access" --component-metadata)"
  python3 - "$asset_dir/BUILD-MANIFEST.json" "$actual_sha256" "$actual_artifact_sha256" "$actual_cdhash" "$actual_requirement" "$access_metadata" <<'PY'
import json, re, sys
def require(condition, message):
    if not condition:
        raise SystemExit(message)

component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_access"]
require(component["sha256"] == sys.argv[2], "Snapshot Access SHA-256 mismatch")
require(component["artifact_sha256"] == sys.argv[3], "Snapshot Access artifact digest mismatch")
requirement_cdhashes = {
    value.lower()
    for value in re.findall(r'\bcdhash\s+H"([0-9A-Fa-f]+)"', sys.argv[5])
}
require(requirement_cdhashes, "Snapshot Access designated requirement has no CDHash identities")
require({component["cdhash"].lower(), sys.argv[4].lower()} <= requirement_cdhashes, "Snapshot Access CDHash mismatch")
require(component["designated_requirement"] == sys.argv[5], "Snapshot Access designated requirement mismatch")
metadata = json.loads(sys.argv[6])
require(component["bundle_id"] == metadata["bundleId"], "Snapshot Access bundle id mismatch")
require(component["relative_path"] == metadata["relativePath"], "Snapshot Access relative path mismatch")
require(component["component_version"] == metadata["componentVersion"], "Snapshot Access component version mismatch")
require(component["protocol_version"] == metadata["protocolVersion"], "Snapshot Access protocol version mismatch")
PY
fi
verify_dmg_helper_identity() (
  set -euo pipefail
  local_dmg="$1"
  require_manifest_identity="$2"
  mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-helper-verify.XXXXXX")"
  mount_point="$(cd "$mount_point" && pwd -P)"
  attached_device=""
  mounted=false
  attach_completed=false
  cleanup() {
    original_status=$?
    cleanup_failed=false
    if [[ "$mounted" == true || -n "$attached_device" || -n "${ATTACHED_DEVICE:-}" || "${ATTACHED_MOUNT:-}" == "$mount_point" ]]; then
      cleanup_device="$attached_device"
      [[ -n "$cleanup_device" ]] || cleanup_device="${ATTACHED_DEVICE:-}"
      [[ -n "$cleanup_device" ]] || cleanup_device="$(resolve_device_for_mount "$mount_point")"
      if [[ -n "$cleanup_device" ]]; then
        if hdiutil detach "$cleanup_device" >/dev/null 2>&1; then
          mounted=false
          attached_device=""
        else
          echo "failed to detach Snapshot Access verification device: $cleanup_device" >&2
          cleanup_failed=true
        fi
      else
        echo "failed to resolve Snapshot Access verification device for cleanup: $mount_point" >&2
        cleanup_failed=true
      fi
    fi
    if ! rmdir "$mount_point" >/dev/null 2>&1; then
      echo "failed to remove Snapshot Access verification mount point: $mount_point" >&2
      cleanup_failed=true
    fi
    if [[ "$cleanup_failed" == true && "$original_status" -eq 0 ]]; then
      exit 1
    fi
  }
  trap cleanup EXIT
  hdiutil verify "$local_dmg"
  emit_dmg_event dmg_verify "$local_dmg" "$mount_point" ""
  attach_dmg_readonly "$local_dmg" "$mount_point"
  attached_device="$ATTACHED_DEVICE"
  mounted=true
  attach_completed=true
  app="$mount_point/TelevyBackup.app"
  [[ ! -L "$app" && -d "$app" ]] || { echo "DMG is missing a real TelevyBackup.app: $local_dmg" >&2; exit 1; }
  bundle_id="$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")"
  [[ "$bundle_id" == "com.ivan.televybackup" ]] || {
    echo "DMG must use the prod app bundle id: $local_dmg" >&2
    exit 1
  }
  codesign --verify --deep --strict "$app"
  app_signature="$(codesign -dvvv "$app" 2>&1 || true)"
  [[ "$app_signature" == *"Signature=adhoc"* ]] || { echo "DMG main app must use an ad-hoc signature: $local_dmg" >&2; exit 1; }
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
    [[ -x "$app/Contents/MacOS/$binary" ]] || { echo "DMG main binary is not executable: $binary" >&2; exit 1; }
    info="$(lipo -info "$app/Contents/MacOS/$binary")"
    case "$expected_arches" in
      universal) [[ "$info" == *arm64* && "$info" == *x86_64* ]] || { echo "Universal DMG binary is missing a slice: $binary" >&2; exit 1; } ;;
      arm64) [[ "$info" == *arm64* && "$info" != *x86_64* ]] || { echo "arm64 DMG contains an unexpected binary architecture: $binary" >&2; exit 1; } ;;
      x86_64) [[ "$info" == *x86_64* && "$info" != *arm64* ]] || { echo "x86_64 DMG contains an unexpected binary architecture: $binary" >&2; exit 1; } ;;
    esac
    binary_signature="$(codesign -dvvv "$app/Contents/MacOS/$binary" 2>&1 || true)"
    [[ "$binary_signature" == *"Signature=adhoc"* ]] || { echo "DMG binary is not ad-hoc signed: $binary" >&2; exit 1; }
  done
  helper="$mount_point/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
  verify_nested_helper_path "$mount_point/TelevyBackup.app" "$helper"
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
  actual_requirement="$(read_designated_requirement "$helper")"
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
import json, re, sys
def require(condition, message):
    if not condition:
        raise SystemExit(message)

component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_access"]
require(component["sha256"] == sys.argv[2], "Snapshot Access SHA-256 mismatch")
require(component["artifact_sha256"] == sys.argv[3], "Snapshot Access artifact digest mismatch")
requirement_cdhashes = {
    value.lower()
    for value in re.findall(r'\bcdhash\s+H"([0-9A-Fa-f]+)"', sys.argv[5])
}
require(requirement_cdhashes, "Snapshot Access designated requirement has no CDHash identities")
require({component["cdhash"].lower(), sys.argv[4].lower()} <= requirement_cdhashes, "Snapshot Access CDHash mismatch")
require(component["designated_requirement"] == sys.argv[5], "Snapshot Access designated requirement mismatch")
metadata = json.loads(sys.argv[6])
require(component["bundle_id"] == metadata["bundleId"], "Snapshot Access bundle id mismatch")
require(component["relative_path"] == metadata["relativePath"], "Snapshot Access relative path mismatch")
require(component["component_version"] == metadata["componentVersion"], "Snapshot Access component version mismatch")
require(component["protocol_version"] == metadata["protocolVersion"], "Snapshot Access protocol version mismatch")
PY
  fi
  diskutil verifyVolume "$attached_device"
  emit_dmg_event dmg_filesystem_verify "$local_dmg" "$mount_point" "$attached_device"
  detach_dmg_exact "$local_dmg" "$mount_point" "$attached_device"
  mounted=false
  attached_device=""
  ATTACHED_DEVICE=""
  ATTACHED_MOUNT=""
  echo "DMG Snapshot Access verified: $local_dmg"
)
check_dmg_layout() {
  local dmg="$1"
  local image_info_path
  local filesystem_info_path
  image_info_path="$(mktemp "${TMPDIR:-/tmp}/televybackup-dmg-image-info.XXXXXX")"
  filesystem_info_path="$(mktemp "${TMPDIR:-/tmp}/televybackup-dmg-filesystem-info.XXXXXX")"
  local mount_point
  mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-verify.XXXXXX")"
  mount_point="$(cd "$mount_point" && pwd -P)"
  local attached_device=""
  local mounted=false
  local attach_completed=false
  cleanup() {
    original_status=$?
    cleanup_failed=false
    if [[ "$mounted" == true || -n "$attached_device" || -n "${ATTACHED_DEVICE:-}" || "${ATTACHED_MOUNT:-}" == "$mount_point" ]]; then
      cleanup_device="$attached_device"
      [[ -n "$cleanup_device" ]] || cleanup_device="${ATTACHED_DEVICE:-}"
      [[ -n "$cleanup_device" ]] || cleanup_device="$(resolve_device_for_mount "$mount_point")"
      if [[ -n "$cleanup_device" ]]; then
        if hdiutil detach "$cleanup_device" >/dev/null 2>&1; then
          mounted=false
          attached_device=""
        else
          echo "failed to detach DMG layout verification device: $cleanup_device" >&2
          cleanup_failed=true
        fi
      else
        echo "failed to resolve DMG layout verification device for cleanup: $mount_point" >&2
        cleanup_failed=true
      fi
    fi
    if ! rmdir "$mount_point" >/dev/null 2>&1; then
      echo "failed to remove DMG layout verification mount point: $mount_point" >&2
      cleanup_failed=true
    fi
    rm -f "$image_info_path" "$filesystem_info_path"
    if [[ "$cleanup_failed" == true && "$original_status" -eq 0 ]]; then
      exit 1
    fi
  }
  trap cleanup RETURN
  hdiutil imageinfo -plist "$dmg" > "$image_info_path"
  hdiutil verify "$dmg"
  emit_dmg_event dmg_verify "$dmg" "$mount_point" ""
  attach_dmg_readonly "$dmg" "$mount_point"
  attached_device="$ATTACHED_DEVICE"
  mounted=true
  attach_completed=true
  diskutil info -plist "$attached_device" > "$filesystem_info_path"
  python3 "$metadata_verifier" \
    --image-info "$image_info_path" \
    --filesystem-info "$filesystem_info_path" \
    --expected-format UDZO \
    --expected-filesystem HFS+
  diskutil verifyVolume "$attached_device"
  local top_level_apps=()
  while IFS= read -r app_path; do
    top_level_apps+=("$app_path")
  done < <(find "$mount_point" -maxdepth 1 -type d -name '*.app' -print)
  if [[ "${#top_level_apps[@]}" -ne 1 || "${top_level_apps[0]}" != "$mount_point/TelevyBackup.app" ]]; then
    echo "DMG must contain exactly one top-level TelevyBackup.app: $dmg" >&2
    return 1
  fi
  if [[ -d "$mount_point/TelevyBackup Snapshot Access.app" ]]; then
    echo "DMG contains a second top-level Snapshot Access app: $dmg" >&2
    return 1
  fi
  python3 - "$mount_point" "$root_dir/assets/brand/macos/dmg/layout.json" <<'PY'
import hashlib
import json
import os
import sys

mount_point, layout_path = sys.argv[1:]
layout = json.load(open(layout_path, encoding="utf-8"))
allowlist = sorted(layout["hidden_resource_allowlist"])
entries = sorted(os.listdir(mount_point))
hidden = sorted(name for name in entries if name.startswith("."))
background_suffix = os.path.splitext(layout["composed_background"])[1]
expected_hidden = sorted([".DS_Store", ".background" + background_suffix])
expected = sorted(["TelevyBackup.app", "Applications"] + expected_hidden)
if entries != expected:
    raise SystemExit(f"DMG top-level entries mismatch: {entries!r}")
logical_hidden = sorted(".background" if name.startswith(".background.") else name for name in hidden)
if logical_hidden != allowlist:
    raise SystemExit(f"DMG hidden-resource allowlist mismatch: {hidden!r}")
background_path = os.path.join(mount_point, ".background" + background_suffix)
if os.path.islink(background_path) or not os.path.isfile(background_path):
    raise SystemExit("DMG .background resource is missing")
expected_background = layout["asset_digests"][layout["composed_background"]]
actual_background = hashlib.sha256(open(background_path, "rb").read()).hexdigest()
if actual_background != expected_background:
    raise SystemExit(f"DMG background digest mismatch: {actual_background} != {expected_background}")
store_path = os.path.join(mount_point, ".DS_Store")
if os.path.islink(store_path) or not os.path.isfile(store_path):
    raise SystemExit("DMG .DS_Store resource is missing")
applications = os.path.join(mount_point, "Applications")
if not os.path.islink(applications) or os.readlink(applications) != "/Applications":
    raise SystemExit("DMG Applications alias does not resolve to /Applications")
if set(logical_hidden) & set(layout["icon_locations"]):
    raise SystemExit("hidden DMG resources have Finder icon locations")
PY
  python3 "$root_dir/scripts/macos/read-ds-store-layout.py" \
    --store "$mount_point/.DS_Store" \
    --layout "$root_dir/assets/brand/macos/dmg/layout.json" >/dev/null
  emit_dmg_event dmg_filesystem_verify "$dmg" "$mount_point" "$attached_device"
  detach_dmg_exact "$dmg" "$mount_point" "$attached_device"
  mounted=false
  attached_device=""
  ATTACHED_DEVICE=""
  ATTACHED_MOUNT=""
}
for dmg in "$asset_dir/TelevyBackup-${version}.dmg" "$asset_dir/TelevyBackup-${version}-arm64.dmg" "$asset_dir/TelevyBackup-${version}-x86_64.dmg"; do
  check_dmg_layout "$dmg"
  verify_dmg_helper_identity "$dmg" true
done
for tools_archive in "$asset_dir/televybackup-tools-${version}-arm64.tar.gz" "$asset_dir/televybackup-tools-${version}-x86_64.tar.gz"; do
  (
    if tar -tzf "$tools_archive" | /usr/bin/grep -E '(^|/)(TelevyBackup Snapshot Access\.app|com\.ivan\.televybackup\.snapshot-access)' >/dev/null; then
      echo "tools archive contains the private Snapshot Access app or service" >&2
      exit 1
    fi
    expected_arches=arm64
    [[ "$tools_archive" == *-x86_64.tar.gz ]] && expected_arches=x86_64
    tools_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-tools-verify.XXXXXX")"
    trap 'rm -rf "$tools_dir"' EXIT
    python3 - "$tools_archive" <<'PY'
import posixpath
import sys
import tarfile

archive = sys.argv[1]
with tarfile.open(archive, "r:gz") as handle:
    for member in handle.getmembers():
        name = member.name
        normalized = posixpath.normpath(name)
        if name.startswith("/") or normalized == ".." or normalized.startswith("../"):
            raise SystemExit(f"unsafe tools archive member path: {name}")
        if member.issym() or member.islnk() or member.isdev() or member.isfifo():
            raise SystemExit(f"unsupported tools archive member type: {name}")
PY
    tar -xzf "$tools_archive" -C "$tools_dir"
    for binary in televybackup televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
      [[ -x "$tools_dir/TelevyBackup Tools/bin/$binary" ]] || { echo "tools binary is not executable: $binary" >&2; exit 1; }
      info="$(lipo -info "$tools_dir/TelevyBackup Tools/bin/$binary")"
      if [[ "$expected_arches" == arm64 ]]; then
        [[ "$info" == *arm64* && "$info" != *x86_64* ]] || { echo "arm64 tools archive contains an unexpected binary architecture: $binary" >&2; exit 1; }
      else
        [[ "$info" == *x86_64* && "$info" != *arm64* ]] || { echo "x86_64 tools archive contains an unexpected binary architecture: $binary" >&2; exit 1; }
      fi
      binary_signature="$(codesign -dvvv "$tools_dir/TelevyBackup Tools/bin/$binary" 2>&1 || true)"
      [[ "$binary_signature" == *"Signature=adhoc"* ]] || { echo "tools binary is not ad-hoc signed: $binary" >&2; exit 1; }
    done
  )
done
echo "release assets verified: ${#required[@]} files"
