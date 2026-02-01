#!/usr/bin/env bash
set -euo pipefail

scene="${1:-}"
out="${2:-}"

if [[ -z "$scene" || -z "$out" ]]; then
  echo "Usage: $0 <scene> <out.png>" >&2
  echo "Scenes: targets-empty | endpoints-empty | targets-unselected | endpoints-unselected" >&2
  exit 2
fi

app_bin="$(git rev-parse --show-toplevel)/target/macos-app/TelevyBackup.app/Contents/MacOS/TelevyBackup"

mkdir -p "$(dirname "$out")"

pkill -x TelevyBackup >/dev/null 2>&1 || true

TELEVYBACKUP_UI_DEMO=1 \
TELEVYBACKUP_UI_DEMO_SCENE="$scene" \
TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
TELEVYBACKUP_OPEN_SETTINGS_ON_LAUNCH=1 \
"$app_bin" >/dev/null 2>&1 &

# Give SwiftUI time to render the Settings window.
sleep 0.9

workdir="$(mktemp -d)"
cat > "$workdir/find_window.swift" <<'SWIFT'
import Foundation
import CoreGraphics

let targetOwner = "TelevyBackup"
let targetName = "Settings"

let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
let windowInfoAny = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as NSArray? ?? []

var bestId: Int?

for case let w as NSDictionary in windowInfoAny {
    guard let owner = w[kCGWindowOwnerName as String] as? String else { continue }
    guard owner == targetOwner else { continue }

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
wid="$($workdir/find_window 2>/dev/null || true)"

if [[ -n "$wid" ]]; then
  screencapture -x -l "$wid" "$out"
else
  echo "WARN: Settings window not found; capturing full screen" >&2
  screencapture -x "$out"
fi

osascript -e 'tell application "TelevyBackup" to quit' >/dev/null 2>&1 || true

rm -rf "$workdir"
