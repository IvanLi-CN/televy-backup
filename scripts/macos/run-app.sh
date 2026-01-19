#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

"$root_dir/scripts/macos/build-app.sh"

osascript -e 'tell application "TelevyBackup" to quit' >/dev/null 2>&1 || true
sleep 0.5

open "$root_dir/target/macos-app/TelevyBackup.app"
