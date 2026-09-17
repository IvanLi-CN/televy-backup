#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: verify-dmg-layout.sh --dmg FILE" >&2
  exit 2
}

dmg=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dmg) dmg="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -s "$dmg" ]] || usage

root_dir="$(git rev-parse --show-toplevel)"
layout_path="$root_dir/assets/brand/macos/dmg/layout.json"
mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-dmg-layout.XXXXXX")"
mount_point="$(cd "$mount_point" && pwd -P)"
attached_device=""
mounted=false
attach_attempted=false
attach_completed=false
if [[ -n "${DMG_EVIDENCE_FILE:-}" ]]; then
  : > "$DMG_EVIDENCE_FILE"
fi
emit_dmg_event() {
  local event="$1"
  local dmg_path="$2"
  local mount_path="$3"
  local device="$4"
  local event_json
  event_json="$(python3 - "$event" "$dmg_path" "$mount_path" "$device" <<'PY'
import json
import sys

print(json.dumps({
    "device": sys.argv[4],
    "dmg": sys.argv[2],
    "event": sys.argv[1],
    "mount_point": sys.argv[3],
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
cleanup() {
  original_status=$?
  cleanup_failed=false
  if [[ "$mounted" == true || -n "$attached_device" ]]; then
    cleanup_device="$attached_device"
    [[ -n "$cleanup_device" ]] || cleanup_device="$(resolve_device_for_mount "$mount_point")"
    if [[ -n "$cleanup_device" ]]; then
      if hdiutil detach "$cleanup_device" >/dev/null 2>&1; then
        mounted=false
        attached_device=""
      else
        echo "failed to detach DMG verification device: $cleanup_device" >&2
        cleanup_failed=true
      fi
    else
      echo "failed to resolve DMG verification device for cleanup: $mount_point" >&2
      cleanup_failed=true
    fi
  fi
  if ! rmdir "$mount_point" >/dev/null 2>&1; then
    echo "failed to remove DMG verification mount point: $mount_point" >&2
    cleanup_failed=true
  fi
  if [[ "$cleanup_failed" == true && "$original_status" -eq 0 ]]; then
    exit 1
  fi
}
trap cleanup EXIT

hdiutil verify "$dmg"
emit_dmg_event dmg_verify "$dmg" "" ""
attach_status=0
attach_attempted=true
if attach_plist="$(hdiutil attach -plist -nobrowse -readonly -mountpoint "$mount_point" "$dmg")"; then
  attach_status=0
else
  attach_status=$?
fi
if (( attach_status != 0 )); then
  read -r attached_device attached_mount < <(
    python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.argv[2].encode())
for entity in payload.get("system-entities", []):
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
print("", "")' "$mount_point" "$attach_plist" 2>/dev/null || true
  )
  if [[ -z "$attached_device" ]]; then
    attached_device="$(resolve_device_for_mount "$mount_point")"
  fi
  mounted=true
  echo "hdiutil attach failed for $dmg (status $attach_status); cleanup will detach $attached_device" >&2
  exit "$attach_status"
fi
mounted=true
attach_completed=true
read -r attached_device attached_mount < <(
  python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.argv[2].encode())
for entity in payload.get("system-entities", []):
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
print("", "")' "$mount_point" "$attach_plist"
  )
if [[ -z "$attached_device" ]]; then
  read -r attached_device attached_mount < <(
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
[[ "$attached_mount" == "$mount_point" && -n "$attached_device" ]] || {
  echo "hdiutil attach plist did not resolve an exact device: $dmg" >&2
  exit 1
}
emit_dmg_event dmg_attach "$dmg" "$attached_mount" "$attached_device"
mounted=true
python3 - "$dmg" "$mount_point" "$attached_device" "$layout_path" <<'PY'
import hashlib
import json
import os
import sys

dmg, mount_point, device, layout_path = sys.argv[1:]
layout = json.load(open(layout_path, encoding="utf-8"))
allowlist = sorted(layout["hidden_resource_allowlist"])
entries = sorted(os.listdir(mount_point))
hidden = sorted(name for name in entries if name.startswith("."))
background_suffix = os.path.splitext(layout["composed_background"])[1]
expected_hidden = sorted([".DS_Store", ".background" + background_suffix])
expected = sorted(["TelevyBackup.app", "Applications"] + expected_hidden)
if entries != expected:
    raise SystemExit(f"DMG top-level entries mismatch: {entries!r}")
app_path = os.path.join(mount_point, "TelevyBackup.app")
if os.path.islink(app_path) or not os.path.isdir(app_path):
    raise SystemExit("DMG TelevyBackup.app must be a real directory")
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
if not os.path.isfile(os.path.join(mount_point, ".DS_Store")):
    raise SystemExit("DMG .DS_Store resource is missing")
applications = os.path.join(mount_point, "Applications")
if not os.path.islink(applications) or os.readlink(applications) != "/Applications":
    raise SystemExit("DMG Applications alias does not resolve to /Applications")
if set(logical_hidden) & set(layout["icon_locations"]):
    raise SystemExit("hidden DMG resources have Finder icon locations")
PY
python3 "$root_dir/scripts/macos/read-ds-store-layout.py" \
  --store "$mount_point/.DS_Store" \
  --layout "$layout_path" >/dev/null
diskutil verifyVolume "$attached_device"
emit_dmg_event dmg_filesystem_verify "$dmg" "$mount_point" "$attached_device"
if hdiutil detach "$attached_device"; then
  mounted=false
else
  detach_status=$?
  echo "failed to detach DMG verification device: $attached_device" >&2
  exit "$detach_status"
fi
emit_dmg_event dmg_detach "$dmg" "$mount_point" "$attached_device"
attached_device=""
echo "DMG layout verified: $dmg"
