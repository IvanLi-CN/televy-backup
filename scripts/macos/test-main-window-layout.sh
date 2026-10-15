#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
output_dir="${1:?Usage: test-main-window-layout.sh <output-dir>}"
build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT
fixture_app="$build_dir/MainWindowLayout.app"
mkdir -p "$fixture_app/Contents/MacOS"
python3 - "$fixture_app/Contents/Info.plist" <<'PY'
import plistlib
import sys
with open(sys.argv[1], "wb") as output:
    plistlib.dump({
        "CFBundleExecutable": "main-window-layout-tests",
        "CFBundleIdentifier": "com.televybackup.tests.layout.dev",
        "CFBundleName": "Main Window Layout Fixture",
        "CFBundlePackageType": "APPL",
        "NSPrincipalClass": "NSApplication",
        "NSHighResolutionCapable": True,
    }, output)
PY

"$(xcrun --find swiftc)" \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -Onone -D TELEVYBACKUP_TESTING -framework SwiftUI -framework AppKit \
  -o "$fixture_app/Contents/MacOS/main-window-layout-tests" \
  "$root_dir/macos/TelevyBackupApp"/*.swift \
  "$root_dir/macos/TelevyBackupAppTests/MainWindowLayoutTests.swift"
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE=main-window-target-detail \
  "$fixture_app/Contents/MacOS/main-window-layout-tests" "$output_dir"
