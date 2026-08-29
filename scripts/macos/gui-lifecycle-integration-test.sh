#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
mode="${1:-gui-only}"

case "$mode" in
  gui-only|--complete-exit) ;;
  *)
    echo "Usage: $0 [gui-only|--complete-exit]" >&2
    exit 2
    ;;
esac

: "${TELEVYBACKUP_CODESIGN_IDENTITY:=-}"
if [[ "$mode" == "--complete-exit" ]]; then
  TELEVYBACKUP_GUI_LIFECYCLE_TESTING=1 \
    TELEVYBACKUP_APP_VARIANT=dev \
    TELEVYBACKUP_CODESIGN_IDENTITY="$TELEVYBACKUP_CODESIGN_IDENTITY" \
    "$root_dir/scripts/macos/build-app.sh" >/dev/null
else
  TELEVYBACKUP_APP_VARIANT=dev \
    TELEVYBACKUP_CODESIGN_IDENTITY="$TELEVYBACKUP_CODESIGN_IDENTITY" \
    "$root_dir/scripts/macos/build-app.sh" >/dev/null
fi

app_bin="$root_dir/target/macos-app/TelevyBackup Dev.app/Contents/MacOS/TelevyBackup"
cli_bin="$root_dir/target/macos-app/TelevyBackup Dev.app/Contents/MacOS/televybackup-cli"
test_root="$(mktemp -d /tmp/televybackup-gui-lifecycle.XXXXXX)"
data_dir="$test_root/data"
config_dir="$test_root/config"
gui_pid=""

cleanup() {
  if [[ -n "$gui_pid" ]] && kill -0 "$gui_pid" 2>/dev/null; then
    kill "$gui_pid" 2>/dev/null || true
    wait "$gui_pid" 2>/dev/null || true
  fi
  if [[ -x "$cli_bin" ]]; then
    "$cli_bin" --data-dir "$data_dir" daemon stop >/dev/null 2>&1 || true
  fi
  rm -rf "$test_root"
}
trap cleanup EXIT

mkdir -p "$data_dir" "$config_dir"

env \
  TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1 \
  TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
  TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
  TELEVYBACKUP_TEST_COMPLETE_EXIT="$([[ "$mode" == "--complete-exit" ]] && echo 1 || echo 0)" \
  "$app_bin" \
  --disable-keychain \
  --data-dir "$data_dir" \
  --config-dir "$config_dir" \
  >/dev/null 2>&1 &
gui_pid=$!

state_path="$data_dir/ipc/gui.state.json"
lock_path="$data_dir/ipc/gui.lock"
for _ in {1..100}; do
  [[ -f "$state_path" && -f "$lock_path" ]] && break
  sleep 0.1
done

[[ -f "$state_path" && -f "$lock_path" ]] || {
  echo "ERROR: GUI control lease was not created" >&2
  exit 1
}

[[ "$(stat -f '%Lp' "$data_dir/ipc")" == "700" ]] || {
  echo "ERROR: GUI control directory permissions are not 0700" >&2
  exit 1
}
[[ "$(stat -f '%Lp' "$state_path")" == "600" ]] || {
  echo "ERROR: GUI control state permissions are not 0600" >&2
  exit 1
}
[[ "$(stat -f '%Lp' "$lock_path")" == "600" ]] || {
  echo "ERROR: GUI lifecycle lock permissions are not 0600" >&2
  exit 1
}

for _ in {1..100}; do
  if "$cli_bin" --json --data-dir "$data_dir" daemon status >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
"$cli_bin" --json --data-dir "$data_dir" daemon status >/dev/null

if [[ "$mode" == "--complete-exit" ]]; then
  for _ in {1..100}; do
    if ! kill -0 "$gui_pid" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if kill -0 "$gui_pid" 2>/dev/null; then
    echo "ERROR: GUI process is still running after complete exit" >&2
    exit 1
  fi
  if "$cli_bin" --json --data-dir "$data_dir" daemon status >/dev/null 2>&1; then
    echo "ERROR: complete exit left the isolated daemon running" >&2
    exit 1
  fi
  echo "OK: complete exit stops the isolated daemon"
  exit 0
fi

result="$("$cli_bin" --json --data-dir "$data_dir" gui quit)"
printf '%s' "$result" | rg -q '"exited":true' || {
  echo "ERROR: gui quit did not report an orderly exit: $result" >&2
  exit 1
}

for _ in {1..100}; do
  if ! kill -0 "$gui_pid" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
if kill -0 "$gui_pid" 2>/dev/null; then
  echo "ERROR: GUI process is still running after gui quit" >&2
  exit 1
fi

"$cli_bin" --json --data-dir "$data_dir" daemon status >/dev/null

already_stopped="$("$cli_bin" --json --data-dir "$data_dir" gui quit)"
printf '%s' "$already_stopped" | rg -q '"alreadyNotRunning":true' || {
  echo "ERROR: stopped GUI lease was not idempotent: $already_stopped" >&2
  exit 1
}

echo "OK: GUI-only handoff preserves the isolated daemon"
