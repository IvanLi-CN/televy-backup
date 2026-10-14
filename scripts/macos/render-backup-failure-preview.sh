#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
output_dir="${1:?Usage: render-backup-failure-preview.sh <output-dir>}"
build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT

"$(xcrun --find swiftc)" \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$build_dir/backup-failure-preview" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/BackupFailureHistoryPreview.swift"
"$build_dir/backup-failure-preview" "$output_dir"
