#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --app /path/to/TelevyBackup.app" >&2
  exit 2
}

app=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done

[[ -d "$app" ]] || usage

plist="$app/Contents/Library/LaunchAgents/com.ivan.televybackup.snapshot-access.plist"
[[ -f "$plist" ]] || { echo "Snapshot Access LaunchAgent plist is missing" >&2; exit 1; }

expected_program="/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access"
program="$(/usr/bin/plutil -extract Program raw -o - "$plist")"
[[ "$program" == "$expected_program" ]] || {
  echo "Snapshot Access LaunchAgent Program does not use the canonical installed app path" >&2
  exit 1
}

if /usr/bin/plutil -extract BundleProgram raw -o - "$plist" >/dev/null 2>&1; then
  echo "Snapshot Access LaunchAgent must not use BundleProgram with direct launchctl bootstrap" >&2
  exit 1
fi

fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/televy-snapshot-launchagent.XXXXXX")"
fixture_label="com.ivan.televybackup.snapshot-access.contract.$RANDOM"
fixture_plist="$fixture_dir/$fixture_label.plist"
domain="gui/$(id -u)"
service="$domain/$fixture_label"

cleanup() {
  /bin/launchctl bootout "$service" >/dev/null 2>&1 || true
}
trap cleanup EXIT

cp "$plist" "$fixture_plist"
/usr/bin/plutil -replace Label -string "$fixture_label" "$fixture_plist"
/usr/bin/plutil -replace Program -string /usr/bin/true "$fixture_plist"
/usr/bin/plutil -replace RunAtLoad -bool false "$fixture_plist"
/usr/bin/plutil -replace KeepAlive -bool false "$fixture_plist"

/bin/launchctl bootstrap "$domain" "$fixture_plist"
if ! /bin/launchctl print "$service" >/dev/null 2>&1; then
  echo "Snapshot Access LaunchAgent fixture was not registered" >&2
  exit 1
fi
/bin/launchctl bootout "$service"
if /bin/launchctl print "$service" >/dev/null 2>&1; then
  echo "Snapshot Access LaunchAgent fixture remained registered after bootout" >&2
  exit 1
fi

echo "OK: Snapshot Access LaunchAgent integration"
