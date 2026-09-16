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
lock_dir="${TMPDIR:-/tmp}/televybackup-finder-dmg-acceptance.lock"
lock_pid_path="$lock_dir/pid"
lock_held=true
snapshot_dir=""
acquire_lock() {
  if mkdir "$lock_dir" 2>/dev/null; then
    printf '%s\n' "$$" > "$lock_pid_path"
    return 0
  fi
  owner_pid=""
  if [[ -s "$lock_pid_path" ]]; then
    owner_pid="$(<"$lock_pid_path")"
  fi
  if [[ "$owner_pid" =~ ^[0-9]+$ ]] && ! kill -0 "$owner_pid" >/dev/null 2>&1; then
    rm -f "$lock_pid_path"
    rmdir "$lock_dir" 2>/dev/null || true
    if mkdir "$lock_dir" 2>/dev/null; then
      printf '%s\n' "$$" > "$lock_pid_path"
      return 0
    fi
  fi
  echo "another Finder acceptance run already owns the Finder session: $lock_dir" >&2
  exit 1
}
acquire_lock
release_lock() {
  if [[ "$lock_held" == true ]]; then
    [[ -s "$lock_pid_path" && "$(<"$lock_pid_path")" == "$$" ]] || return 1
    rm -f "$lock_pid_path" || return 1
    rmdir "$lock_dir" >/dev/null 2>&1 || return 1
    lock_held=false
  fi
}
cleanup_preflight() {
  if [[ -n "$snapshot_dir" ]]; then
    rm -rf "$snapshot_dir"
  fi
  release_lock || true
}
trap cleanup_preflight EXIT
root_dir="$(git rev-parse --show-toplevel)"
layout_path="$root_dir/assets/brand/macos/dmg/layout.json"
source_dmg="$dmg"
source_asset_dir="$(dirname "$source_dmg")"
source_manifest_path="$source_asset_dir/BUILD-MANIFEST.json"
source_checksums_path="$source_asset_dir/SHA256SUMS"
[[ -s "$source_manifest_path" ]] || {
  echo "Finder acceptance requires the adjacent BUILD-MANIFEST.json" >&2
  exit 1
}
[[ -s "$source_checksums_path" ]] || {
  echo "Finder acceptance requires the adjacent SHA256SUMS" >&2
  exit 1
}
snapshot_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-finder-acceptance-input.XXXXXX")"
dmg="$snapshot_dir/$(basename "$source_dmg")"
manifest_path="$snapshot_dir/BUILD-MANIFEST.json"
checksums_path="$snapshot_dir/SHA256SUMS"
cp "$source_dmg" "$dmg"
cp "$source_manifest_path" "$manifest_path"
cp "$source_checksums_path" "$checksums_path"
dmg_sha256="$(shasum -a 256 "$dmg" | awk '{print $1}')"
"$root_dir/scripts/macos/verify-dmg-layout.sh" --dmg "$dmg"
python3 - "$checksums_path" "$(basename "$dmg")" "$dmg_sha256" <<'PY'
import sys

checksums_path, dmg_name, expected_digest = sys.argv[1:]
records = {}
for line in open(checksums_path, encoding="utf-8"):
    fields = line.rstrip("\n").split(maxsplit=1)
    if len(fields) != 2:
        raise SystemExit("malformed SHA256SUMS entry")
    name = fields[1].removeprefix("*")
    if name in records:
        raise SystemExit(f"duplicate SHA256SUMS entry: {name}")
    records[name] = fields[0]
if records.get(dmg_name) != expected_digest:
    raise SystemExit("Finder acceptance DMG does not match adjacent SHA256SUMS")
PY
macos_version="$(sw_vers -productVersion)"
machine_arch="$(uname -m)"
mount_point="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-finder-acceptance.XXXXXX")"
mount_point="$(cd "$mount_point" && pwd -P)"
attached_device=""
mounted=false
previous_show_all=""
previous_show_all_type=""
previous_show_all_present=false
cleanup() {
  original_status=$?
  cleanup_failed=false
  if [[ "$previous_show_all_present" == true ]]; then
    case "$previous_show_all_type" in
      boolean) restore_command=(defaults write com.apple.finder AppleShowAllFiles -bool "$previous_show_all") ;;
      integer) restore_command=(defaults write com.apple.finder AppleShowAllFiles -int "$previous_show_all") ;;
      real) restore_command=(defaults write com.apple.finder AppleShowAllFiles -float "$previous_show_all") ;;
      string) restore_command=(defaults write com.apple.finder AppleShowAllFiles -string "$previous_show_all") ;;
      *)
        echo "unsupported Finder AppleShowAllFiles preference type: $previous_show_all_type" >&2
        cleanup_failed=true
        restore_command=()
        ;;
    esac
    if [[ "${#restore_command[@]}" -gt 0 ]] && ! "${restore_command[@]}" >/dev/null 2>&1; then
      echo "failed to restore Finder AppleShowAllFiles preference" >&2
      cleanup_failed=true
    fi
  else
    if ! defaults delete com.apple.finder AppleShowAllFiles >/dev/null 2>&1; then
      if defaults read com.apple.finder AppleShowAllFiles >/dev/null 2>&1; then
        echo "failed to remove Finder AppleShowAllFiles preference" >&2
        cleanup_failed=true
      fi
    fi
  fi
  if [[ "$previous_show_all_present" == true || "$finder_was_visible_changed" == true ]]; then
    if ! killall Finder >/dev/null 2>&1; then
      echo "failed to restart Finder after restoring preferences" >&2
      cleanup_failed=true
    fi
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
  if [[ -n "$snapshot_dir" ]]; then
    if ! rm -rf "$snapshot_dir"; then
      echo "failed to remove Finder acceptance input snapshot: $snapshot_dir" >&2
      cleanup_failed=true
    fi
    snapshot_dir=""
  fi
  if ! release_lock; then
    echo "failed to release Finder acceptance evidence lock: $lock_dir" >&2
    cleanup_failed=true
  fi
  if [[ "$cleanup_failed" == true && "$original_status" -eq 0 ]]; then
    exit 1
  fi
}
previous_show_all="$(defaults read com.apple.finder AppleShowAllFiles 2>/dev/null || true)"
previous_show_all_type="$(defaults read-type com.apple.finder AppleShowAllFiles 2>/dev/null | awk '$1 == "Type" && $2 == "is" { print $3; exit }' || true)"
if [[ -n "$previous_show_all_type" ]]; then
  previous_show_all_present=true
  case "$previous_show_all_type" in
    boolean|integer|real|string) ;;
    *)
      echo "unsupported Finder AppleShowAllFiles preference type: $previous_show_all_type" >&2
      rmdir "$mount_point"
      exit 1
      ;;
  esac
fi
finder_was_visible_changed=false
trap cleanup EXIT
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
rm -f "$finder_json"
instruction_text="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["overlay"]["instruction"])' "$layout_path")"
for attempt in 1 2 3 4 5; do
  if [[ "$attempt" -gt 1 ]]; then
    open "$mount_point" >/dev/null 2>&1 || true
  fi
  if osascript "$root_dir/scripts/macos/finder-dmg-observe.applescript" "$mount_point" "$finder_json" "$instruction_text"; then
    break
  fi
  sleep 1
done
[[ -s "$finder_json" ]] || {
  echo "Finder observation did not resolve the mounted DMG window" >&2
  exit 1
}
osascript -e 'tell application "Finder" to set selection of front window to {}' >/dev/null 2>&1 || true

window_id="$(python3 - "$finder_json" <<'PY'
import json
import sys

value = json.load(open(sys.argv[1], encoding="utf-8")).get("window_id")
print(value if isinstance(value, int) else "")
PY
)"
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
python3 - "$attached_device" "$source_dmg" "$dmg" "$dmg_sha256" "$manifest_path" "$checksums_path" "$layout_path" "$evidence_dir/finder-window.png" "$machine_arch" "$macos_version" "$hidden_json" <<'PY' > "$evidence_dir/acceptance.json"
import hashlib
import json
import pathlib
import sys

hidden = json.load(open(sys.argv[11], encoding="utf-8"))
layout = json.load(open(sys.argv[7], encoding="utf-8"))
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
verified_dmg_path = pathlib.Path(sys.argv[3])
manifest_path = pathlib.Path(sys.argv[5])
checksums_path = pathlib.Path(sys.argv[6])
manifest_bytes = manifest_path.read_bytes()
manifest_sha256 = hashlib.sha256(manifest_bytes).hexdigest()
checksums_sha256 = hashlib.sha256(checksums_path.read_bytes()).hexdigest()
manifest = json.loads(manifest_bytes.decode())
record = next((asset for asset in manifest.get("assets", []) if asset.get("name") == verified_dmg_path.name), None)
if record is None or record.get("sha256") != sys.argv[4] or record.get("dmg_layout_digest") != semantic_layout_digest:
    raise SystemExit("Finder acceptance DMG does not match its adjacent BUILD-MANIFEST.json")
print(json.dumps({
    "architecture": sys.argv[9],
    "capture_scope": "finder-window-only",
    "device": sys.argv[1],
    "dmg": sys.argv[2],
    "dmg_sha256": sys.argv[4],
    "event": "finder_acceptance",
    "manifest": str(manifest_path),
    "manifest_sha256": manifest_sha256,
    "manifest_verified": True,
    "semantic_layout_digest": semantic_layout_digest,
    "checksums": str(checksums_path),
    "checksums_sha256": checksums_sha256,
    "checksums_verified": True,
    "show_all_files": hidden,
    "macos_version": sys.argv[10],
    "screenshot": sys.argv[8],
}, sort_keys=True))
PY
echo "Finder DMG acceptance evidence: $evidence_dir"
