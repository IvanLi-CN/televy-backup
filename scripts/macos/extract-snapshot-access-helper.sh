#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: extract-snapshot-access-helper.sh --dmg FILE --output-dir DIR" >&2
  exit 2
}

dmg=""
output_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dmg) dmg="${2:-}"; shift 2 ;;
    --output-dir) output_dir="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -s "$dmg" && -n "$output_dir" ]] || usage
root_dir="$(git rev-parse --show-toplevel)"
path_safety_checker="$root_dir/scripts/macos/reject-symlink-components.py"
reject_symlink_components() {
  python3 "$path_safety_checker" "$1"
}
reject_symlink_components "$dmg"
reject_symlink_components "$output_dir"
mkdir -p "$output_dir"

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-snapshot-helper.XXXXXX")"
mount_point="$work_dir/mount"
mkdir -p "$mount_point"
attached_device=""
attach_attempted=false

device_from_plist() {
  local plist_path="$1"
  python3 - "$plist_path" "$mount_point" <<'PY'
import plistlib
import sys

plist_path, expected_mount = sys.argv[1:]
try:
    with open(plist_path, "rb") as handle:
        payload = plistlib.load(handle)
except (OSError, plistlib.InvalidFileException, ValueError):
    raise SystemExit(0)
entities = list(payload.get("system-entities", []))
for image in payload.get("images", []):
    entities.extend(image.get("system-entities", []))
for entity in entities:
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"])
        raise SystemExit(0)
PY
}

resolve_device_for_mount() {
  local info_path="$work_dir/hdiutil-info.plist"
  if hdiutil info -plist > "$info_path" 2>/dev/null; then
    device_from_plist "$info_path" 2>/dev/null || true
  fi
}

cleanup() {
  local original_status=$?
  local cleanup_device="$attached_device"
  [[ -n "$cleanup_device" ]] || cleanup_device="$(resolve_device_for_mount)"
  if [[ -n "$cleanup_device" ]]; then
    if ! hdiutil detach "$cleanup_device" >/dev/null 2>&1; then
      echo "failed to detach Snapshot Access helper device: $cleanup_device" >&2
      [[ "$original_status" -ne 0 ]] || original_status=1
    fi
  elif [[ "$attach_attempted" == true ]]; then
    echo "failed to resolve Snapshot Access helper device for cleanup: $mount_point" >&2
    [[ "$original_status" -ne 0 ]] || original_status=1
  fi
  if ! rmdir "$mount_point" >/dev/null 2>&1; then
    echo "failed to remove Snapshot Access helper mount point: $mount_point" >&2
    [[ "$original_status" -ne 0 ]] || original_status=1
  fi
  if ! rmdir "$work_dir" >/dev/null 2>&1; then
    echo "failed to remove Snapshot Access helper work directory: $work_dir" >&2
    [[ "$original_status" -ne 0 ]] || original_status=1
  fi
  if [[ "$original_status" -ne 0 ]]; then
    exit "$original_status"
  fi
}
trap cleanup EXIT

attach_plist="$work_dir/attach.plist"
attach_attempted=true
attach_status=0
if hdiutil attach -plist -nobrowse -readonly -mountpoint "$mount_point" "$dmg" > "$attach_plist"; then
  :
else
  attach_status=$?
fi

attached_device="$(device_from_plist "$attach_plist" 2>/dev/null || true)"
[[ -n "$attached_device" ]] || attached_device="$(resolve_device_for_mount)"
if (( attach_status != 0 )); then
  echo "hdiutil attach failed for $dmg (status $attach_status); cleanup will detach $attached_device" >&2
  exit "$attach_status"
fi
[[ -n "$attached_device" ]] || {
  echo "hdiutil attach plist did not resolve an exact device: $dmg" >&2
  exit 1
}

helper_path="$mount_point/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
reject_symlink_components "$helper_path"
[[ ! -L "$mount_point/TelevyBackup.app" && -d "$mount_point/TelevyBackup.app" ]] || {
  echo "Snapshot Access extraction requires a real TelevyBackup.app directory" >&2
  exit 1
}
[[ ! -L "$helper_path" && -d "$helper_path" ]] || {
  echo "Snapshot Access extraction requires a real nested helper directory" >&2
  exit 1
}
app_real="$(cd "$mount_point/TelevyBackup.app" && pwd -P)"
helper_real="$(cd "$helper_path" && pwd -P)"
[[ "$helper_real" == "$app_real/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app" ]] || {
  echo "Snapshot Access extraction path escapes the main app bundle" >&2
  exit 1
}
helper_binary="$helper_path/Contents/MacOS/televybackup-snapshot-access"
reject_symlink_components "$helper_binary"
[[ ! -L "$helper_binary" && -f "$helper_binary" ]] || {
  echo "Snapshot Access extraction requires a real helper executable" >&2
  exit 1
}
output_helper="$output_dir/TelevyBackup Snapshot Access.app"
[[ ! -e "$output_helper" && ! -L "$output_helper" ]] || {
  echo "Snapshot Access extraction refuses an existing output helper path: $output_helper" >&2
  exit 1
}
ditto "$helper_path" "$output_helper"
python3 - "$attached_device" "$dmg" "$mount_point" <<'PY'
import json
import sys

print(json.dumps({
    "device": sys.argv[1],
    "dmg": sys.argv[2],
    "event": "snapshot_helper_extract",
    "mount_point": sys.argv[3],
}, sort_keys=True))
PY
