#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

out_dir="${OUT_DIR:-$root_dir/target/swift-unit-tests}"
mkdir -p "$out_dir"

sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
swiftc="$(xcrun --find swiftc)"

search_file() {
  local pattern="$1"
  local path="$2"
  if command -v rg >/dev/null 2>&1; then
    rg -n "$pattern" "$path"
  else
    grep -En "$pattern" "$path"
  fi
}

search_stdin() {
  local pattern="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n "$pattern"
  else
    grep -En "$pattern"
  fi
}

search_stdin_quiet() {
  local pattern="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -q "$pattern"
  else
    grep -Eq "$pattern"
  fi
}

# SettingsWindow is daemon-facing UI and must stay on control IPC. Keep this red-capable seam
# close to the existing Swift checks so a future edit cannot silently reintroduce the CLI process
# boundary that caused signal=9 failures.
if search_file 'runCommandCapture|cliPath\(' "$root_dir/macos/TelevyBackupApp/SettingsWindow.swift"; then
  echo "SettingsWindow must not invoke the CLI" >&2
  exit 1
fi

# AppModel refreshes shared settings after launch and backup completion. Keep that path on the
# daemon control socket too; otherwise a background refresh can reintroduce the signal=9 failure
# even though the Settings window itself is IPC-only.
refresh_settings_source="$(sed -n '/private func refreshSettings(withSecrets:/,/func openSettingsWindow/p' "$root_dir/macos/TelevyBackupApp/TelevyBackupApp.swift")"
if printf '%s\n' "$refresh_settings_source" | search_stdin 'runCommandCapture|cliPath\('; then
  echo "AppModel.refreshSettings must not invoke the CLI" >&2
  exit 1
fi
if ! printf '%s\n' "$refresh_settings_source" | search_stdin_quiet 'ControlIPCClient\.request'; then
  echo "AppModel.refreshSettings must use ControlIPCClient" >&2
  exit 1
fi

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

bin_status_store="$out_dir/status-store-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -framework Combine \
  -o "$bin_status_store" \
  "$root_dir/macos/TelevyBackupApp/StatusModels.swift" \
  "$root_dir/macos/TelevyBackupApp/StatusStore.swift" \
  "$root_dir/macos/TelevyBackupAppTests/StatusStoreTests.swift"
"$bin_status_store"

bin_target_presentation="$out_dir/target-presentation-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$bin_target_presentation" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/TargetPresentationTests.swift"
"$bin_target_presentation"

bin_menu_bar="$out_dir/menu-bar-presentation-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$bin_menu_bar" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/MenuBarPresentationTests.swift"
"$bin_menu_bar"

bin_menu_bar_icon="$out_dir/menu-bar-status-item-icon-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -framework AppKit \
  -o "$bin_menu_bar_icon" \
  "$root_dir/macos/TelevyBackupApp/MenuBarActivityState.swift" \
  "$root_dir/macos/TelevyBackupApp/MenuBarStatusItemIcon.swift" \
  "$root_dir/macos/TelevyBackupAppTests/MenuBarStatusItemIconTests.swift"
"$bin_menu_bar_icon"

"$swiftc" \
  -typecheck \
  -sdk "$sdk_path" \
  -framework SwiftUI \
  -framework AppKit \
  "$root_dir/macos/TelevyBackupApp"/*.swift

"$root_dir/scripts/macos/ui-store-static-check.sh"

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

bin_snapshot_inspection="$out_dir/snapshot-inspection-presentation-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$bin_snapshot_inspection" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/SnapshotInspectionPresentationTests.swift"
"$bin_snapshot_inspection"

bin_diagnostics="$out_dir/diagnostics-settings-tests"
"$swiftc" \
  -sdk "$sdk_path" \
  -O \
  -D TELEVYBACKUP_TESTING \
  -framework SwiftUI \
  -framework AppKit \
  -o "$bin_diagnostics" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/DiagnosticsSettingsTests.swift"
"$bin_diagnostics"
