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
if [ "${TELEVYBACKUP_DISABLE_KEYCHAIN:-}" = "1" ]; then
  if [ -z "${TELEVYBACKUP_DATA_DIR:-}" ]; then
    tmpdir="$(mktemp -d)"
    export TELEVYBACKUP_DATA_DIR="$tmpdir"
    export TELEVYBACKUP_CONFIG_DIR="$tmpdir"
    echo "TELEVYBACKUP_DISABLE_KEYCHAIN=1: using temp dir: $tmpdir" >&2
  fi
  "$app/Contents/MacOS/TelevyBackup" >/dev/null 2>&1 &
else
  open "$app"
fi
