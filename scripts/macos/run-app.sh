#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

variant="${TELEVYBACKUP_APP_VARIANT:-dev}"
export TELEVYBACKUP_APP_VARIANT="$variant"
case "$variant" in
  prod)
    bundle_display_name="TelevyBackup"
    bundle_id="com.ivan.televybackup"
    ;;
  dev)
    bundle_display_name="TelevyBackup Dev"
    bundle_id="com.ivan.televybackup.dev"
    ;;
  *)
    echo "ERROR: invalid TELEVYBACKUP_APP_VARIANT=$variant (expected: dev|prod)" >&2
    exit 2
    ;;
esac

# Development default: disable Keychain to avoid prompts and keep local runs reproducible.
# Override with `TELEVYBACKUP_DISABLE_KEYCHAIN=0` for production-like behavior.
disable_keychain="${TELEVYBACKUP_DISABLE_KEYCHAIN:-}"
if [ -z "$disable_keychain" ]; then
  if [ "$variant" = "dev" ]; then
    disable_keychain="1"
  else
    disable_keychain="0"
  fi
fi

if [ "$variant" = "prod" ] && [ "$disable_keychain" = "1" ]; then
  echo "WARN: starting prod variant with Keychain disabled (TELEVYBACKUP_DISABLE_KEYCHAIN=1)" >&2
  echo "      Recommended: unset TELEVYBACKUP_DISABLE_KEYCHAIN or set it to 0 for normal prod behavior" >&2
fi

# When Keychain is disabled we also default to ad-hoc codesigning so `security find-identity`
# doesn't trigger interactive Keychain authorization prompts.
if [ "$disable_keychain" = "1" ] && [ -z "${TELEVYBACKUP_CODESIGN_IDENTITY:-}" ]; then
  export TELEVYBACKUP_CODESIGN_IDENTITY="-"
fi

"$root_dir/scripts/macos/build-app.sh"

app="$root_dir/target/macos-app/${bundle_display_name}.app"
app_bin="$app/Contents/MacOS/TelevyBackup"
app_daemon="$app/Contents/MacOS/televybackupd"
app_cli="$app/Contents/MacOS/televybackup-cli"

osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
for _ in {1..40}; do
  if ! pgrep -f "$app_bin" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

# Always kill any leftover processes from previous runs (we've seen cases where the GUI exits
# but the daemon/helper keeps running, causing the "wrong version" to be used).
pkill -f "$app_bin" >/dev/null 2>&1 || true
pkill -f "$app_daemon" >/dev/null 2>&1 || true
pkill -f "$app_cli --json status stream" >/dev/null 2>&1 || true
pkill -f "$app/Contents/MacOS/televybackup-mtproto-helper" >/dev/null 2>&1 || true

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
  rm -f "$data_dir/ipc/"*.sock >/dev/null 2>&1 || true
  # NOTE: launching via LaunchServices keeps the menu bar app alive; env vars are not reliably
  # inherited by `open`, so pass overrides via `--args` and let the app propagate to subprocesses.
  args=(--disable-keychain --data-dir "$data_dir" --config-dir "$config_dir")
  if [ "${TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH:-0}" = "1" ]; then
    args+=(--open-settings)
  fi
  open -n "$app" --args "${args[@]}"
else
  args=()
  # DEV defaults to disabling Keychain; allow explicit override for prod-like testing.
  if [ "$variant" = "dev" ]; then
    args+=(--enable-keychain)
  fi
  if [ "${TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH:-0}" = "1" ]; then
    args+=(--open-settings)
  fi
  if [ "${#args[@]}" -gt 0 ]; then
    open -n "$app" --args "${args[@]}"
  else
    open "$app"
  fi
fi
