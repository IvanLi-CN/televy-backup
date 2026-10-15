#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
output_dir="${1:?Usage: test-main-window-layout.sh <output-dir>}"
build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT

"$(xcrun --find swiftc)" \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -O -D TELEVYBACKUP_TESTING -framework SwiftUI -framework AppKit \
  -o "$build_dir/main-window-layout-tests" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/MainWindowLayoutTests.swift"
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE=main-window-target-detail \
  "$build_dir/main-window-layout-tests" "$output_dir"
