#!/usr/bin/env bash
set -euo pipefail

scene="${1:-}"
out="${2:-}"

if [[ -z "$scene" || -z "$out" ]]; then
  echo "Usage: $0 <scene> <out.png>" >&2
  echo "Scenes: targets-empty | endpoints-empty | targets-unselected | endpoints-unselected | diagnostics-normal | diagnostics-debug | diagnostics-override | diagnostics-retention" >&2
  exit 2
fi

root_dir="$(git rev-parse --show-toplevel)"
app_bin="$root_dir/target/macos-app/TelevyBackup.app/Contents/MacOS/TelevyBackup"
demo_root="$root_dir/.dev/ui-snapshot"
data_dir="$demo_root/data"
config_dir="$demo_root/config"

mkdir -p "$(dirname "$out")"
mkdir -p "$data_dir" "$config_dir"

TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="$scene" \
TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1 \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
"$app_bin" >/dev/null 2>&1 &
app_pid=$!
workdir=""

cleanup() {
  kill -TERM "$app_pid" >/dev/null 2>&1 || true
  wait "$app_pid" >/dev/null 2>&1 || true
  if [[ -n "$workdir" ]]; then
    rm -rf "$workdir"
  fi
}
trap cleanup EXIT

# Give SwiftUI time to render the Settings window.
sleep 0.9

# Ensure the exact app process we launched is active/key before capturing. Otherwise macOS
# renders toolbar/titlebar controls in their inactive appearance, which makes segmented controls
# and traffic-light buttons look washed out in screenshots. Activating by PID avoids focusing a
# different installed Release/Dev variant that happens to share the same display name.
swift -e '
import AppKit
import Foundation

guard CommandLine.arguments.count > 1, let pid = Int32(CommandLine.arguments[1]) else {
    exit(1)
}

guard let app = NSRunningApplication(processIdentifier: pid) else {
    exit(1)
}

if !app.activate(options: [.activateIgnoringOtherApps]) {
    exit(1)
}
' "$app_pid" >/dev/null 2>&1 || true
sleep 0.2

workdir="$(mktemp -d)"
cat > "$workdir/find_window.swift" <<'SWIFT'
import Foundation
import CoreGraphics

let targetOwner = "TelevyBackup"
let targetName = "Settings"
guard CommandLine.arguments.count > 1, let targetPid = Int(CommandLine.arguments[1]) else {
    exit(2)
}

let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let windowInfoAny = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as NSArray? ?? []

var bestId: Int?

for case let w as NSDictionary in windowInfoAny {
    guard let owner = w[kCGWindowOwnerName as String] as? String else { continue }
    guard owner == targetOwner else { continue }
    guard let ownerPid = w[kCGWindowOwnerPID as String] as? Int, ownerPid == targetPid else { continue }

    let name = (w[kCGWindowName as String] as? String) ?? ""
    let windowNumber = w[kCGWindowNumber as String] as? Int

    if bestId == nil, let windowNumber {
        bestId = windowNumber
    }

    if name == targetName, let windowNumber {
        print(windowNumber)
        exit(0)
    }
}

if let bestId {
    print(bestId)
    exit(0)
}

exit(1)
SWIFT

swiftc "$workdir/find_window.swift" -o "$workdir/find_window" >/dev/null 2>&1
wid="$($workdir/find_window "$app_pid" 2>/dev/null || true)"

if [[ -n "$wid" ]]; then
  screencapture -x -l "$wid" "$out"
else
  echo "ERROR: Settings window not found; refusing full-screen capture" >&2
  exit 1
fi
