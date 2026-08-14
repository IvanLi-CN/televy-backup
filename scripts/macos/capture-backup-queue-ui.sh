#!/usr/bin/env bash
set -euo pipefail

appearance="${1:-}"
out_dir="${2:-}"

if [[ -z "$appearance" || -z "$out_dir" ]]; then
  echo "Usage: $0 <light|dark> <out-dir>" >&2
  exit 2
fi

case "$appearance" in
  light|dark) ;;
  *)
    echo "ERROR: invalid appearance=$appearance (expected: light or dark)" >&2
    exit 2
    ;;
esac

root_dir="$(git rev-parse --show-toplevel)"
: "${TELEVYBACKUP_CODESIGN_IDENTITY:=-}"
TELEVYBACKUP_CODESIGN_IDENTITY="$TELEVYBACKUP_CODESIGN_IDENTITY" "$root_dir/scripts/macos/build-app.sh" >/dev/null

app_bin="$root_dir/target/macos-app/TelevyBackup.app/Contents/MacOS/TelevyBackup"
demo_root="$root_dir/.dev/ui-snapshot/backup-queue"
data_dir="$demo_root/data"
config_dir="$demo_root/config"
mkdir -p "$out_dir" "$data_dir" "$config_dir"

capture_popover() {
  local scene="$1"
  local prefix="$2"
  rm -f "$out_dir/$prefix-popover.png"
  env \
    TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1 \
    TELEVYBACKUP_UI_DEMO=1 \
    TELEVYBACKUP_UI_DEMO_SCENE="$scene" \
    TELEVYBACKUP_UI_APPEARANCE="$appearance" \
    TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir" \
    TELEVYBACKUP_UI_SNAPSHOT_PREFIX="$prefix" \
    TELEVYBACKUP_UI_SNAPSHOT_MODE=timer \
    TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS=1000 \
    TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
    TELEVYBACKUP_DATA_DIR="$data_dir" \
    TELEVYBACKUP_CONFIG_DIR="$config_dir" \
    TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=1 \
    TELEVYBACKUP_OPEN_MAIN_WINDOW_ON_LAUNCH=0 \
    TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=0 \
    "$app_bin" >/dev/null 2>&1
}

capture_main_window() {
  local scene="$1"
  local prefix="$2"
  TELEVYBACKUP_UI_APPEARANCE="$appearance" \
    bash "$root_dir/scripts/macos/capture-main-window.sh" \
    "$scene" \
    "$out_dir/$prefix-main-window.png"
}

connecting_prefix="backup-queue-$appearance-connecting-queued"
running_prefix="backup-queue-$appearance-running-next-queued"

capture_popover "main-window-target-connecting-queued" "$connecting_prefix"
capture_main_window "main-window-target-connecting-queued" "$connecting_prefix"
capture_popover "main-window-target-running-next-queued" "$running_prefix"
capture_main_window "main-window-target-running-next-queued" "$running_prefix"

for expected in \
  "$out_dir/$connecting_prefix-popover.png" \
  "$out_dir/$connecting_prefix-main-window.png" \
  "$out_dir/$running_prefix-popover.png" \
  "$out_dir/$running_prefix-main-window.png"
do
  if [[ ! -f "$expected" ]]; then
    echo "ERROR: missing scoped UI snapshot: $expected" >&2
    exit 1
  fi
done

echo "Wrote backup queue UI snapshots under: $out_dir" >&2
