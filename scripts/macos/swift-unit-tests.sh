#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

out_dir="${OUT_DIR:-$root_dir/target/swift-unit-tests}"
mkdir -p "$out_dir"

sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
swiftc="$(xcrun --find swiftc)"

bin_rebind="$out_dir/import-bundle-rebind-logic-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -o "$bin_rebind" \
  "$root_dir/macos/TelevyBackupApp/ImportBundleRebindLogic.swift" \
  "$root_dir/macos/TelevyBackupAppTests/ImportBundleRebindLogicTests.swift"
"$bin_rebind"

bin_progress="$out_dir/backup-progress-projection-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -o "$bin_progress" \
  "$root_dir/macos/TelevyBackupApp/StatusModels.swift" \
  "$root_dir/macos/TelevyBackupApp/BackupProgressProjection.swift" \
  "$root_dir/macos/TelevyBackupAppTests/BackupProgressProjectionTests.swift"
"$bin_progress"

bin_popover="$out_dir/popover-layout-size-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$bin_popover" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/PopoverLayoutSizeTests.swift"
"$bin_popover"

bin_demo_paths="$out_dir/ui-demo-sandbox-path-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$bin_demo_paths" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/UIDemoSandboxPathTests.swift"
"$bin_demo_paths"
