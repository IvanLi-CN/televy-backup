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

open "$root_dir/target/macos-app/TelevyBackup.app"
