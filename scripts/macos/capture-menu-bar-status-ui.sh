#!/usr/bin/env bash
set -euo pipefail

out_dir="${1:-}"
if [[ -z "$out_dir" ]]; then
  echo "Usage: $0 <out-dir>" >&2
  exit 2
fi

root_dir="$(git rev-parse --show-toplevel)"
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
bin_dir="$root_dir/target/menu-bar-preview"
bin_path="$bin_dir/menu-bar-status-item-icon-preview"
output="$out_dir/menu-bar-activity-states.png"

mkdir -p "$bin_dir" "$out_dir"
xcrun swiftc \
  -sdk "$sdk_path" \
  -O \
  -framework AppKit \
  -o "$bin_path" \
  "$root_dir/macos/TelevyBackupApp/MenuBarStatusItemIcon.swift" \
  "$root_dir/macos/TelevyBackupAppTests/MenuBarStatusItemIconPreview.swift"

"$bin_path" "$output"
test -s "$output"
echo "Wrote controlled menu-bar preview: $output" >&2
