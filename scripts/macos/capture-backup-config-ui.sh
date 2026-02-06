#!/usr/bin/env bash
set -euo pipefail

# Deterministic UI snapshots for the Backup Config (export/import bundle) flows.
# Uses the in-app UISnapshot capture (no Screen Recording permission needed).
#
# Usage:
#   ./scripts/macos/capture-backup-config-ui.sh <out_dir>
#
# Output:
#   <out_dir>/settings-demo-backup-config*.png

out_dir="${1:-}"
if [[ -z "$out_dir" ]]; then
  echo "Usage: $0 <out_dir>" >&2
  exit 2
fi

root_dir="$(git rev-parse --show-toplevel)"
app_bin="$root_dir/target/macos-app/TelevyBackup.app/Contents/MacOS/TelevyBackup"

mkdir -p "$out_dir"

# A tiny demo `.tbconfig` (key string) for the import flow. Keep it in `.dev/` which is gitignored.
demo_root="$root_dir/.dev/ui-snapshot"
mkdir -p "$demo_root"
demo_file="$demo_root/demo.tbconfig"
if [[ ! -f "$demo_file" ]]; then
  python3 - <<PY
import base64, json
obj = {"hint": "Demo: passphrase set on export"}
raw = json.dumps(obj, separators=(",", ":")).encode("utf-8")
key = "TBC2:" + base64.urlsafe_b64encode(raw).decode("ascii").rstrip("=")
open("$demo_file","w",encoding="utf-8").write(key + "\\n")
print("wrote", "$demo_file")
PY
fi

data_dir="$demo_root/data"
config_dir="$demo_root/config"
mkdir -p "$data_dir" "$config_dir"

quit_app() {
  osascript -e 'tell application "TelevyBackup" to quit' >/dev/null 2>&1 || true
  pkill -x TelevyBackup >/dev/null 2>&1 || true
}

run_scene() {
  local scene="$1"
  local prefix="$2"
  local delay_ms="${3:-1600}"
  shift 3 || true

  quit_app

  TELEVYBACKUP_UI_DEMO=1 \
  TELEVYBACKUP_UI_DEMO_SCENE="$scene" \
  TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
  TELEVYBACKUP_UI_SNAPSHOT_PREFIX="$prefix" \
  TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="$delay_ms" \
  TELEVYBACKUP_UI_SNAPSHOT_MODE=timer \
  TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
  TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
  TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
  TELEVYBACKUP_DATA_DIR="$data_dir" \
  TELEVYBACKUP_CONFIG_DIR="$config_dir" \
  "$@" \
  "$app_bin"
}

# Backup Config page itself.
run_scene "backup-config" "settings-demo-backup-config" 1600

# Export flow (Save Panel + accessory prefilled).
quit_app
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="backup-config-export" \
TELEVYBACKUP_UI_DEMO_EXPORT_PASSPHRASE="1234" \
TELEVYBACKUP_UI_DEMO_EXPORT_HINT=$'Demo: my MacBook\nSecond line (optional)' \
TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
TELEVYBACKUP_UI_SNAPSHOT_PREFIX="settings-demo-backup-config-export" \
TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="2200" \
TELEVYBACKUP_UI_SNAPSHOT_MODE=timer \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
"$app_bin"

# Import pre-inspect (passphrase entry) with hint preview.
quit_app
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="backup-config-import" \
TELEVYBACKUP_UI_DEMO_IMPORT_FILE="$demo_file" \
TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
TELEVYBACKUP_UI_SNAPSHOT_PREFIX="settings-demo-backup-config-import-preinspect" \
TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="1800" \
TELEVYBACKUP_UI_SNAPSHOT_MODE=timer \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
"$app_bin"

# Import inspect/result page with multiple targets + conflicts, snapshot at the exact moment the
# demo inspection is loaded.
quit_app
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="backup-config-import-result" \
TELEVYBACKUP_UI_DEMO_IMPORT_FILE="$demo_file" \
TELEVYBACKUP_UI_DEMO_IMPORT_PASSPHRASE="1234" \
TELEVYBACKUP_UI_DEMO_IMPORT_TARGETS_COUNT="8" \
TELEVYBACKUP_UI_DEMO_IMPORT_CONFLICTS="1" \
TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
TELEVYBACKUP_UI_SNAPSHOT_PREFIX="settings-demo-backup-config-import-result" \
TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="600" \
TELEVYBACKUP_UI_SNAPSHOT_MODE=manual \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
"$app_bin"

# Import result page with a simulated rebind compare mismatch (no network).
quit_app
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="backup-config-import-result" \
TELEVYBACKUP_UI_DEMO_IMPORT_FILE="$demo_file" \
TELEVYBACKUP_UI_DEMO_IMPORT_PASSPHRASE="1234" \
TELEVYBACKUP_UI_DEMO_IMPORT_TARGETS_COUNT="8" \
TELEVYBACKUP_UI_DEMO_IMPORT_CONFLICTS="1" \
TELEVYBACKUP_UI_DEMO_REBIND_COMPARE_STATE="mismatch" \
TELEVYBACKUP_UI_DEMO_REBIND_PATH="/Users/ivan/Demo/RebindFolder" \
TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
TELEVYBACKUP_UI_SNAPSHOT_PREFIX="settings-demo-backup-config-import-result-rebind-compare" \
TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="600" \
TELEVYBACKUP_UI_SNAPSHOT_MODE=manual \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
"$app_bin"

# Import result page with the Change folder picker open (to validate NSOpenPanel UI).
quit_app
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="backup-config-import-result-change-folder-picker" \
TELEVYBACKUP_UI_DEMO_IMPORT_FILE="$demo_file" \
TELEVYBACKUP_UI_DEMO_IMPORT_PASSPHRASE="1234" \
TELEVYBACKUP_UI_DEMO_IMPORT_TARGETS_COUNT="8" \
TELEVYBACKUP_UI_DEMO_IMPORT_CONFLICTS="1" \
TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
TELEVYBACKUP_UI_SNAPSHOT_PREFIX="settings-demo-backup-config-import-result-change-folder-picker" \
TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="1400" \
TELEVYBACKUP_UI_SNAPSHOT_MODE=manual \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
"$app_bin"

quit_app

echo "Wrote snapshots under: $out_dir" >&2
