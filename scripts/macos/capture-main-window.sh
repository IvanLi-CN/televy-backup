#!/usr/bin/env bash
set -euo pipefail

scene="${1:-}"
out="${2:-}"

if [[ -z "$scene" || -z "$out" ]]; then
  echo "Usage: $0 <scene> <out.png>" >&2
  echo "Scenes: main-window-targets | main-window-target-detail | main-window-target-connecting-queued | main-window-target-running-next-queued | main-window-target-starting | main-window-snapshot-changes | main-window-snapshot-baseline-unavailable | main-window-snapshot-failed-unavailable" >&2
  exit 2
fi

root_dir="$(git rev-parse --show-toplevel)"
variant="${TELEVYBACKUP_APP_VARIANT:-prod}"
case "$variant" in
  prod) app_name="TelevyBackup" ;;
  dev) app_name="TelevyBackup Dev" ;;
  *)
    echo "ERROR: invalid TELEVYBACKUP_APP_VARIANT=$variant (expected: dev|prod)" >&2
    exit 2
    ;;
esac
app_bin="$root_dir/target/macos-app/$app_name.app/Contents/MacOS/TelevyBackup"
demo_root="$root_dir/.dev/ui-snapshot"
data_dir="$demo_root/data"
config_dir="$demo_root/config"

mkdir -p "$(dirname "$out")"
mkdir -p "$data_dir" "$config_dir"

export TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1
TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="$scene" \
TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
TELEVYBACKUP_DATA_DIR="$data_dir" \
TELEVYBACKUP_CONFIG_DIR="$config_dir" \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_MAIN_WINDOW_ON_LAUNCH=1 \
"$app_bin" >/dev/null 2>&1 &
app_pid=$!
workdir="$(mktemp -d)"
cleanup() {
  if kill -0 "$app_pid" >/dev/null 2>&1; then
    kill "$app_pid" >/dev/null 2>&1 || true
    wait "$app_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$workdir"
}
trap cleanup EXIT

# Give SwiftUI time to render the main window.
sleep 1.4

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

cat > "$workdir/find_window.swift" <<'SWIFT'
import Foundation
import CoreGraphics

guard CommandLine.arguments.count > 2, let targetPid = Int32(CommandLine.arguments[1]) else {
    exit(1)
}

let targetOwner = CommandLine.arguments[2]
let targetName = targetOwner

let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let windowInfoAny = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as NSArray? ?? []

var bestId: Int?
var bestArea: Double = 0

for case let w as NSDictionary in windowInfoAny {
    guard let owner = w[kCGWindowOwnerName as String] as? String else { continue }
    guard owner == targetOwner else { continue }
    guard let ownerPid = w[kCGWindowOwnerPID as String] as? NSNumber,
          ownerPid.int32Value == targetPid else { continue }

    let name = (w[kCGWindowName as String] as? String) ?? ""
    let windowNumber = w[kCGWindowNumber as String] as? Int
    let layer = (w[kCGWindowLayer as String] as? Int) ?? -1
    if let windowNumber {
        if let bounds = w[kCGWindowBounds as String] as? NSDictionary,
           let widthNum = bounds["Width"] as? NSNumber,
           let heightNum = bounds["Height"] as? NSNumber
        {
            let width = widthNum.doubleValue
            let height = heightNum.doubleValue
            if width >= 200, height >= 200 {
                let area = width * height
                // Prefer normal windows (layer 0).
                if layer == 0, area > bestArea {
                    bestArea = area
                    bestId = windowNumber
                } else if bestId == nil, area > bestArea {
                    bestArea = area
                    bestId = windowNumber
                }
            }
        } else if bestId == nil, layer == 0 {
            bestId = windowNumber
        }
    }

    if name == targetName, layer == 0, let windowNumber {
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
wid="$($workdir/find_window "$app_pid" "$app_name" 2>/dev/null || true)"

if [[ -z "$wid" ]]; then
  echo "ERROR: main window for demo PID $app_pid was not found; refusing an unscoped capture" >&2
  exit 1
fi
screencapture -x -l "$wid" "$out"
