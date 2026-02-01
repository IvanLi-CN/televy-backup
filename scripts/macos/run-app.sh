#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

"$root_dir/scripts/macos/build-app.sh"

osascript -e 'tell application "TelevyBackup" to quit' >/dev/null 2>&1 || true
for _ in {1..40}; do
  if ! pgrep -x "TelevyBackup" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

app="$root_dir/target/macos-app/TelevyBackup.app"

# Development default: disable Keychain to avoid prompts and keep local runs reproducible.
# Override with `TELEVYBACKUP_DISABLE_KEYCHAIN=0` for production-like behavior.
disable_keychain="${TELEVYBACKUP_DISABLE_KEYCHAIN:-1}"
if [ "$disable_keychain" = "1" ]; then
  data_dir="${TELEVYBACKUP_DATA_DIR:-}"
  config_dir="${TELEVYBACKUP_CONFIG_DIR:-}"
  if [ -z "$data_dir" ] || [ -z "$config_dir" ]; then
    dev_root="$root_dir/.dev/televybackup"
    data_dir="${data_dir:-$dev_root/data}"
    config_dir="${config_dir:-$dev_root/config}"
    mkdir -p "$data_dir" "$config_dir"
    echo "TELEVYBACKUP_DISABLE_KEYCHAIN=1: using workspace dirs:" >&2
    echo "  TELEVYBACKUP_DATA_DIR=$data_dir" >&2
    echo "  TELEVYBACKUP_CONFIG_DIR=$config_dir" >&2
  fi
  # NOTE: launching via LaunchServices keeps the menu bar app alive; env vars are not reliably
  # inherited by `open`, so pass overrides via `--args` and let the app propagate to subprocesses.
  open -n "$app" --args --disable-keychain --data-dir "$data_dir" --config-dir "$config_dir"
else
  open "$app"
fi
