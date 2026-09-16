#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: finder-dmg-acceptance.sh --dmg FILE --evidence-dir DIR" >&2
  exit 2
}

dmg=""
evidence_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dmg) dmg="${2:-}"; shift 2 ;;
    --evidence-dir) evidence_dir="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -s "$dmg" && -n "$evidence_dir" ]] || usage
[[ "${TELEVYBACKUP_RUN_FINDER_ACCEPTANCE:-0}" == "1" ]] || {
  echo "set TELEVYBACKUP_RUN_FINDER_ACCEPTANCE=1 in the controlled GUI session" >&2
  exit 2
}
mkdir -p "$evidence_dir"
root_dir="$(git rev-parse --show-toplevel)"
layout_path="$root_dir/assets/brand/macos/dmg/layout.json"
"$root_dir/scripts/macos/verify-dmg-layout.sh" --dmg "$dmg"
dmg_sha256="$(shasum -a 256 "$dmg" | awk '{print $1}')"
manifest_path="$(dirname "$dmg")/BUILD-MANIFEST.json"
[[ -s "$manifest_path" ]] || {
  echo "Finder acceptance requires the adjacent BUILD-MANIFEST.json" >&2
  exit 1
}
macos_version="$(sw_vers -productVersion)"
machine_arch="$(uname -m)"
mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-finder-acceptance.XXXXXX")"
mount_point="$(cd "$mount_point" && pwd -P)"
attached_device=""
mounted=false
previous_show_all=""
cleanup() {
  original_status=$?
  cleanup_failed=false
  if [[ -n "$previous_show_all" ]]; then
    defaults write com.apple.finder AppleShowAllFiles "$previous_show_all" >/dev/null 2>&1 || true
  else
    defaults delete com.apple.finder AppleShowAllFiles >/dev/null 2>&1 || true
  fi
  if [[ -n "$previous_show_all" || "$finder_was_visible_changed" == true ]]; then
    killall Finder >/dev/null 2>&1 || true
  fi
  if [[ -n "$attached_device" ]]; then
    if ! hdiutil detach "$attached_device" >/dev/null 2>&1; then
      echo "failed to detach Finder acceptance device: $attached_device" >&2
      cleanup_failed=true
    fi
  fi
  if ! rmdir "$mount_point" >/dev/null 2>&1; then
    echo "failed to remove Finder acceptance mount point: $mount_point" >&2
    cleanup_failed=true
  fi
  if [[ "$cleanup_failed" == true && "$original_status" -eq 0 ]]; then
    exit 1
  fi
}
trap cleanup EXIT

previous_show_all="$(defaults read com.apple.finder AppleShowAllFiles 2>/dev/null || true)"
finder_was_visible_changed=false
hdiutil verify "$dmg" >/dev/null
attach_plist="$(hdiutil attach -plist -nobrowse -readonly -mountpoint "$mount_point" "$dmg")"
read -r attached_device attached_mount < <(
  python3 -c 'import plistlib, sys
expected_mount = sys.argv[1]
payload = plistlib.loads(sys.argv[2].encode())
fallback = ""
for entity in payload.get("system-entities", []):
    if entity.get("dev-entry") and not fallback:
        fallback = entity["dev-entry"]
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
print(fallback, "")' "$mount_point" "$attach_plist"
  )
[[ "$attached_mount" == "$mount_point" && -n "$attached_device" ]] || {
  echo "could not resolve exact attached Finder device" >&2
  exit 1
}
mounted=true
diskutil verifyVolume "$attached_device" >/dev/null

open "$mount_point"
osascript -e 'tell application "Finder" to activate'
sleep 2
finder_json="$evidence_dir/finder-observation.json"
for attempt in 1 2 3 4 5; do
  if [[ "$attempt" -gt 1 ]]; then
    open "$mount_point" >/dev/null 2>&1 || true
  fi
  if osascript "$root_dir/scripts/macos/finder-dmg-observe.applescript" "$mount_point" "$finder_json"; then
    break
  fi
  sleep 1
done
[[ -s "$finder_json" ]] || {
  echo "Finder observation did not resolve the mounted DMG window" >&2
  exit 1
}
osascript -e 'tell application "Finder" to set selection of front window to {}' >/dev/null 2>&1 || true

window_id="$(osascript -e 'tell application "Finder" to id of front window' 2>/dev/null || true)"
[[ "$window_id" =~ ^[0-9]+$ ]] || window_id=""
[[ -n "$window_id" ]] || {
  echo "Finder window was not found; refusing an unscoped screenshot" >&2
  exit 1
}
screencapture -x -l "$window_id" "$evidence_dir/finder-window.png"
[[ -s "$evidence_dir/finder-window.png" ]] || {
  echo "Finder window screenshot was not created" >&2
  exit 1
}

defaults write com.apple.finder AppleShowAllFiles -bool true
finder_was_visible_changed=true
killall Finder >/dev/null 2>&1 || true
sleep 1
open "$mount_point"
hidden_json="$evidence_dir/show-all-files.json"
python3 - "$mount_point" "$layout_path" <<'PY' > "$hidden_json"
import json
import os
import sys

mount_point, layout_path = sys.argv[1:]
layout = json.load(open(layout_path, encoding="utf-8"))
suffix = os.path.splitext(layout["composed_background"])[1]
expected = sorted([".DS_Store", ".background" + suffix])
observed = sorted(name for name in os.listdir(mount_point) if name.startswith("."))
if observed != expected:
    raise SystemExit(f"Show All Files allowlist mismatch: {observed!r}")
print(json.dumps({
    "allowlist": layout["hidden_resource_allowlist"],
    "observed": observed,
    "visible_window_region": "outside-default-icon-region",
}, sort_keys=True))
PY

python3 - "$finder_json" "$layout_path" <<'PY'
import json
import sys

observation = json.load(open(sys.argv[1], encoding="utf-8"))
layout = json.load(open(sys.argv[2], encoding="utf-8"))
if observation["window_role"] != "Finder":
    raise SystemExit("observation is not a Finder window")
if observation["app_name"] != "TelevyBackup.app":
    raise SystemExit("Finder observation is missing TelevyBackup.app")
if observation["applications_name"] != "Applications":
    raise SystemExit("Finder observation is missing Applications")
if observation["drag_direction"] != "right":
    raise SystemExit("Finder observation does not show the expected drag direction")
if observation["instruction"] != layout["overlay"]["instruction"]:
    raise SystemExit("Finder observation instruction differs from the layout schema")
expected_app = tuple(layout["icon_locations"]["TelevyBackup.app"])
expected_applications = tuple(layout["icon_locations"]["Applications"])
if tuple(observation["app_position"]) != expected_app:
    raise SystemExit(f"TelevyBackup.app position differs from schema: {observation['app_position']!r}")
if tuple(observation["applications_position"]) != expected_applications:
    raise SystemExit(f"Applications position differs from schema: {observation['applications_position']!r}")
PY
python3 - "$attached_device" "$dmg" "$dmg_sha256" "$layout_path" "$evidence_dir/finder-window.png" "$machine_arch" "$macos_version" "$hidden_json" <<'PY' > "$evidence_dir/acceptance.json"
import hashlib
import json
import pathlib
import sys

hidden = json.load(open(sys.argv[8], encoding="utf-8"))
layout = json.load(open(sys.argv[4], encoding="utf-8"))
canonical_layout = {
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
        "digests": layout["asset_digests"],
    },
    "hidden_resource_allowlist": sorted(layout["hidden_resource_allowlist"]),
    "symlinks": layout["symlinks"],
}
semantic_layout_digest = hashlib.sha256(json.dumps(canonical_layout, ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
dmg_path = pathlib.Path(sys.argv[2])
manifest_sha256 = ""
manifest_path = dmg_path.with_name("BUILD-MANIFEST.json")
manifest_bytes = manifest_path.read_bytes()
manifest_sha256 = hashlib.sha256(manifest_bytes).hexdigest()
manifest = json.loads(manifest_bytes.decode())
record = next((asset for asset in manifest.get("assets", []) if asset.get("name") == dmg_path.name), None)
if record is None or record.get("sha256") != sys.argv[3] or record.get("dmg_layout_digest") != semantic_layout_digest:
    raise SystemExit("Finder acceptance DMG does not match its adjacent BUILD-MANIFEST.json")
print(json.dumps({
    "architecture": sys.argv[6],
    "capture_scope": "finder-window-only",
    "device": sys.argv[1],
    "dmg": sys.argv[2],
    "dmg_sha256": sys.argv[3],
    "event": "finder_acceptance",
    "manifest": str(manifest_path),
    "manifest_sha256": manifest_sha256,
    "manifest_verified": True,
    "semantic_layout_digest": semantic_layout_digest,
    "show_all_files": hidden,
    "macos_version": sys.argv[7],
    "screenshot": sys.argv[5],
}, sort_keys=True))
PY
echo "Finder DMG acceptance evidence: $evidence_dir"
