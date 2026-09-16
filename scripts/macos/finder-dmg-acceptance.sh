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
macos_version="$(sw_vers -productVersion)"
machine_arch="$(uname -m)"
mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-finder-acceptance.XXXXXX")"
mount_point="$(cd "$mount_point" && pwd -P)"
attached_device=""
mounted=false
previous_show_all=""
cleanup() {
  if [[ -n "$previous_show_all" ]]; then
    defaults write com.apple.finder AppleShowAllFiles "$previous_show_all" >/dev/null 2>&1 || true
  else
    defaults delete com.apple.finder AppleShowAllFiles >/dev/null 2>&1 || true
  fi
  if [[ -n "$previous_show_all" || "$finder_was_visible_changed" == true ]]; then
    killall Finder >/dev/null 2>&1 || true
  fi
  if [[ "$mounted" == true ]]; then
    hdiutil detach "$attached_device" >/dev/null 2>&1 || true
  fi
  rmdir "$mount_point" >/dev/null 2>&1 || true
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
for entity in payload.get("system-entities", []):
    if entity.get("mount-point") == expected_mount and entity.get("dev-entry"):
        print(entity["dev-entry"], entity["mount-point"])
        raise SystemExit(0)
raise SystemExit("attach plist did not identify the requested Finder mount")' "$mount_point" "$attach_plist"
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
app_x, app_y = observation["app_position"]
applications_x, applications_y = observation["applications_position"]
if abs((applications_x - app_x) - 340) > 50 or abs(applications_y - app_y) > 50:
    raise SystemExit("Finder icon positions differ materially from the layout schema")
PY
python3 - "$attached_device" "$dmg" "$evidence_dir/finder-window.png" "$machine_arch" "$macos_version" "$hidden_json" <<'PY' > "$evidence_dir/acceptance.json"
import json
import sys

hidden = json.load(open(sys.argv[6], encoding="utf-8"))
print(json.dumps({
    "architecture": sys.argv[4],
    "capture_scope": "finder-window-only",
    "device": sys.argv[1],
    "dmg": sys.argv[2],
    "event": "finder_acceptance",
    "show_all_files": hidden,
    "macos_version": sys.argv[5],
    "screenshot": sys.argv[3],
}, sort_keys=True))
PY
echo "Finder DMG acceptance evidence: $evidence_dir"
