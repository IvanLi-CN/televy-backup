#!/usr/bin/env bash
set -euo pipefail

appearance="${1:-}"
out_dir="${2:-}"

if [[ -z "$appearance" || -z "$out_dir" ]]; then
  echo "Usage: $0 <light|dark|system> <out-dir>" >&2
  exit 2
fi

case "$appearance" in
  light|dark|system) ;;
  *)
    echo "ERROR: invalid appearance=$appearance (expected: light|dark|system)" >&2
    exit 2
    ;;
esac

root_dir="$(git rev-parse --show-toplevel)"
: "${TELEVYBACKUP_CODESIGN_IDENTITY:=-}"
TELEVYBACKUP_CODESIGN_IDENTITY="$TELEVYBACKUP_CODESIGN_IDENTITY" "$root_dir/scripts/macos/build-app.sh" >/dev/null

app_bin="$root_dir/target/macos-app/TelevyBackup.app/Contents/MacOS/TelevyBackup"
demo_root="$root_dir/.dev/ui-snapshot"
data_dir="$demo_root/data"
config_dir="$demo_root/config"
mkdir -p "$out_dir"
mkdir -p "$data_dir" "$config_dir"
rm -f "$out_dir/theme-$appearance-"*.png >/dev/null 2>&1 || true

common_env=(
  TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1
  TELEVYBACKUP_UI_DEMO=1
  TELEVYBACKUP_UI_APPEARANCE="$appearance"
  TELEVYBACKUP_UI_SNAPSHOT_DIR="$out_dir"
  TELEVYBACKUP_UI_SNAPSHOT_PREFIX="theme-$appearance"
  TELEVYBACKUP_UI_SNAPSHOT_MODE=timer
  TELEVYBACKUP_DATA_DIR="$data_dir"
  TELEVYBACKUP_CONFIG_DIR="$config_dir"
)

# Popover uses the main-window demo scene for seeded status data, but should remain the only visible window.
capture_popover() {
  env \
    "${common_env[@]}" \
    TELEVYBACKUP_UI_DEMO_SCENE="main-window-target-detail" \
    TELEVYBACKUP_UI_SNAPSHOT_DELAY_MS="1000" \
    TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=1 \
    TELEVYBACKUP_OPEN_MAIN_WINDOW_ON_LAUNCH=0 \
    TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=0 \
    "$app_bin" >/dev/null 2>&1
}

capture_popover
if [[ ! -f "$out_dir/theme-$appearance-popover.png" ]]; then
  capture_popover
fi

# Main/settings use target-window screenshots because NavigationSplitView-backed windows
# render more faithfully there than in deterministic content snapshots.
TELEVYBACKUP_UI_APPEARANCE="$appearance" \
  "$root_dir/scripts/macos/capture-main-window.sh" \
  main-window-target-detail \
  "$out_dir/theme-$appearance-main-window.png"

TELEVYBACKUP_UI_APPEARANCE="$appearance" \
  "$root_dir/scripts/macos/capture-settings-window.sh" \
  schedule \
  "$out_dir/theme-$appearance-settings.png"

for expected in \
  "$out_dir/theme-$appearance-popover.png" \
  "$out_dir/theme-$appearance-main-window.png" \
  "$out_dir/theme-$appearance-settings.png"
do
  if [[ ! -f "$expected" ]]; then
    echo "ERROR: missing snapshot: $expected" >&2
    exit 1
  fi
done

echo "Wrote theme snapshots under: $out_dir" >&2
