#!/usr/bin/env bash
set -euo pipefail
root_dir="$(git rev-parse --show-toplevel)"
output_dir="${1:?Usage: render-snapshot-operation-preview.sh <output-dir>}"
build_dir="$(mktemp -d)"
trap 'rm -f "$build_dir/preview"; rmdir "$build_dir"' EXIT
"$(xcrun --find swiftc)" -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -framework SwiftUI -framework AppKit \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountOperationProgressView.swift" \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountHelperStatusView.swift" \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountHelperReadiness.swift" \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountHelperInstallation.swift" \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountHelperControlAction.swift" \
  "$root_dir/macos/TelevyBackupAppTests/SnapshotMountOperationProgressPreview.swift" \
  -o "$build_dir/preview"
"$build_dir/preview" "$output_dir"
