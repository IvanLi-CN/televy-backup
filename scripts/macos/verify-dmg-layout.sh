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
cleanup() {
  if [[ "$mounted" == true ]]; then
    hdiutil detach "$attached_device" >/dev/null 2>&1 || true
  fi
  rmdir "$mount_point" >/dev/null 2>&1 || true
}
trap cleanup EXIT

hdiutil verify "$dmg"
attach_plist="$(hdiutil attach -plist -nobrowse -readonly -mountpoint "$mount_point" "$dmg")"
read -r attached_device attached_mount < <(
  python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.argv[2].encode())
for entity in payload.get("system-entities", []):
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
raise SystemExit("hdiutil attach plist did not identify the requested mount point")' "$mount_point" "$attach_plist"
  )
[[ "$attached_mount" == "$mount_point" && -n "$attached_device" ]] || {
  echo "hdiutil attach plist did not resolve an exact device: $dmg" >&2
  exit 1
}
mounted=true
python3 - "$dmg" "$mount_point" "$attached_device" "$layout_path" <<'PY'
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
logical_hidden = sorted(".background" if name.startswith(".background.") else name for name in hidden)
if logical_hidden != allowlist:
    raise SystemExit(f"DMG hidden-resource allowlist mismatch: {hidden!r}")
if not os.path.isfile(os.path.join(mount_point, ".background" + background_suffix)):
    raise SystemExit("DMG .background resource is missing")
if not os.path.isfile(os.path.join(mount_point, ".DS_Store")):
    raise SystemExit("DMG .DS_Store resource is missing")
applications = os.path.join(mount_point, "Applications")
if not os.path.islink(applications) or os.readlink(applications) != "/Applications":
    raise SystemExit("DMG Applications alias does not resolve to /Applications")
if set(logical_hidden) & set(layout["icon_locations"]):
    raise SystemExit("hidden DMG resources have Finder icon locations")
PY
diskutil verifyVolume "$attached_device"
mounted=false
hdiutil detach "$attached_device"
python3 - "$attached_device" "$dmg" "$mount_point" <<'PY'
import json
import sys

print(json.dumps({
    "device": sys.argv[1],
    "dmg": sys.argv[2],
    "event": "dmg_detach",
    "mount_point": sys.argv[3],
}, sort_keys=True))
PY
echo "DMG layout verified: $dmg"
