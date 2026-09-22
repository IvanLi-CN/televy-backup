#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 --app /path/to/TelevyBackup.app [--lifecycle-app /path/to/test-hook.app]" >&2
  exit 2
}

app=""
lifecycle_app=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app) app="${2:-}"; shift 2 ;;
    --lifecycle-app) lifecycle_app="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done

[[ -d "$app" ]] || usage
[[ -z "$lifecycle_app" || -d "$lifecycle_app" ]] || usage

root_dir="$(git rev-parse --show-toplevel)"
app_bin="$app/Contents/MacOS/TelevyBackup"
cli_bin="$app/Contents/MacOS/televybackup-cli"
plist="$app/Contents/Library/LaunchAgents/com.ivan.televybackup.snapshot-access.plist"
helper="$app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access"
expected_program="/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access"

[[ -x "$app_bin" && -x "$cli_bin" && -x "$helper" ]] || {
  echo "production bundle entrypoints are incomplete" >&2
  exit 1
}
[[ -f "$plist" ]] || { echo "production Snapshot Access plist is missing" >&2; exit 1; }
[[ "$(/usr/bin/plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")" == "com.ivan.televybackup" ]] || {
  echo "HIL requires the production bundle identifier" >&2
  exit 1
}
[[ "$(/usr/bin/plutil -extract Program raw -o - "$plist")" == "$expected_program" ]] || {
  echo "production Snapshot Access plist does not use the canonical Program" >&2
  exit 1
}
if /usr/bin/plutil -extract BundleProgram raw -o - "$plist" >/dev/null 2>&1; then
  echo "production Snapshot Access plist must not contain BundleProgram" >&2
  exit 1
fi
codesign --verify --deep --strict "$app" >/dev/null

# launchd-launched helpers reject the per-process macOS TMPDIR path on this host;
# keep the fixture under /tmp while retaining private 0700 subdirectories.
test_root="$(mktemp -d "/tmp/televybackup-release-hil.XXXXXX")"
fixture_dir="$test_root/launchctl-fixture"
fixture_config="$test_root/config"
fixture_data="$test_root/data"
source_a="$test_root/source-a"
source_b="$test_root/source-b"
fixture_label="com.ivan.televybackup.snapshot-access.hil.$RANDOM"
fixture_plist="$fixture_dir/$fixture_label.plist"
fixture_domain="gui/$(id -u)"
fixture_service="$fixture_domain/$fixture_label"
fixture_socket="$fixture_data/snapshot-access/access.sock"
fixture_journal="$test_root/journal.sqlite"
stream_file="$test_root/status-stream.jsonl"
gui_pid=""
stream_pid=""

stop_isolated_helper() {
  local pid
  for pid in $(/usr/bin/pgrep -f -x "$helper" || true); do
    kill "$pid" >/dev/null 2>&1 || true
  done
  for _ in {1..20}; do
    if ! /usr/bin/pgrep -f -x "$helper" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
  done
}

cleanup() {
  [[ -n "$stream_pid" ]] && kill "$stream_pid" >/dev/null 2>&1 || true
  if [[ -n "$gui_pid" ]]; then
    "$cli_bin" --json --config-dir "$fixture_config" --data-dir "$fixture_data" daemon stop >/dev/null 2>&1 || true
    kill "$gui_pid" >/dev/null 2>&1 || true
    wait "$gui_pid" >/dev/null 2>&1 || true
  fi
  stop_isolated_helper
  /bin/launchctl bootout "$fixture_service" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$fixture_dir" "$fixture_config" "$fixture_data" "$source_a" "$source_b"

# Prove the actual helper binary can run under launchctl while all mutable state stays isolated.
cp "$plist" "$fixture_plist"
/usr/bin/plutil -replace Label -string "$fixture_label" "$fixture_plist"
/usr/bin/plutil -replace Program -string "$helper" "$fixture_plist"
/usr/bin/plutil -replace RunAtLoad -bool true "$fixture_plist"
/usr/bin/plutil -replace KeepAlive -bool false "$fixture_plist"
/usr/bin/plutil -insert EnvironmentVariables -xml "<dict><key>TELEVYBACKUP_SNAPSHOT_SOCKET</key><string>$fixture_socket</string><key>TELEVYBACKUP_SNAPSHOT_JOURNAL</key><string>$fixture_journal</string><key>TELEVYBACKUP_CONFIG_DIR</key><string>$fixture_config</string><key>TELEVYBACKUP_DATA_DIR</key><string>$fixture_data</string></dict>" "$fixture_plist"
/bin/launchctl bootstrap "$fixture_domain" "$fixture_plist"
for _ in {1..300}; do
  [[ -S "$fixture_socket" ]] && break
  sleep 0.1
done
if [[ ! -S "$fixture_socket" ]]; then
  echo "actual helper did not create the isolated socket" >&2
  /bin/launchctl print "$fixture_service" >&2 || true
  exit 1
fi
/bin/launchctl print "$fixture_service" >/dev/null
echo "OK: actual helper bootstrap and isolated socket"
/bin/launchctl bootout "$fixture_service"
if /bin/launchctl print "$fixture_service" >/dev/null 2>&1; then
  echo "actual helper remained loaded after bootout" >&2
  exit 1
fi
echo "OK: actual helper bootout"

cp "$root_dir/scripts/macos/fixtures/perf-idle/config.toml" "$fixture_config/config.toml"
sed -i '' -e "s#__FIXTURE_A__#$source_a#g" -e "s#__FIXTURE_B__#$source_b#g" "$fixture_config/config.toml"

start_gui() {
  local binary="$1"
  local complete_exit="$2"
  TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1 \
    TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
    TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
    TELEVYBACKUP_TEST_COMPLETE_EXIT="$complete_exit" \
    "$binary" --disable-keychain --config-dir "$fixture_config" --data-dir "$fixture_data" \
    >"$test_root/gui.log" 2>&1 &
  gui_pid=$!
}

wait_for_daemon() {
  for _ in {1..120}; do
    if "$cli_bin" --json --config-dir "$fixture_config" --data-dir "$fixture_data" daemon status >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

start_gui "$app_bin" 0
wait_for_daemon || { echo "production bundle daemon did not become ready" >&2; exit 1; }

snapshot_json="$($cli_bin --json --config-dir "$fixture_config" --data-dir "$fixture_data" status get)"
jq -e '.targets | length == 2 and ([.[].targetId] | sort == ["fixture-a", "fixture-b"])' <<<"$snapshot_json" >/dev/null || {
  echo "production bundle status snapshot did not expose both configured targets" >&2
  exit 1
}
echo "OK: status snapshot exposes configured targets"

"$cli_bin" --json --config-dir "$fixture_config" --data-dir "$fixture_data" status stream >"$stream_file" 2>/dev/null &
stream_pid=$!
for _ in {1..100}; do
  [[ -s "$stream_file" ]] && break
  sleep 0.1
done
stream_json="$(sed -n '1p' "$stream_file")"
jq -e '.targets | length == 2 and ([.[].targetId] | sort == ["fixture-a", "fixture-b"])' <<<"$stream_json" >/dev/null || {
  echo "production bundle status stream did not expose both configured targets" >&2
  exit 1
}
kill "$stream_pid" >/dev/null 2>&1 || true
wait "$stream_pid" >/dev/null 2>&1 || true
stream_pid=""
echo "OK: status stream exposes configured targets"

access_status=""
for _ in {1..100}; do
  access_status="$($cli_bin --json --config-dir "$fixture_config" --data-dir "$fixture_data" snapshot-access status 2>/dev/null || true)"
  if jq -e '.serviceReachable == true' <<<"$access_status" >/dev/null 2>&1; then break; fi
  sleep 0.1
done
jq -e '.serviceReachable == true' <<<"$access_status" >/dev/null || {
  echo "production bundle Snapshot Access service was not reachable" >&2
  exit 1
}
echo "OK: Snapshot Access service reachable in production bundle HIL"

"$cli_bin" --json --config-dir "$fixture_config" --data-dir "$fixture_data" daemon stop >/dev/null
kill "$gui_pid" >/dev/null 2>&1 || true
wait "$gui_pid" >/dev/null 2>&1 || true
gui_pid=""

if [[ -n "$lifecycle_app" ]]; then
  lifecycle_bin="$lifecycle_app/Contents/MacOS/TelevyBackup"
  [[ -x "$lifecycle_bin" ]] || { echo "lifecycle app executable is missing" >&2; exit 1; }
  start_gui "$lifecycle_bin" 1
  for _ in {1..240}; do
    if ! kill -0 "$gui_pid" >/dev/null 2>&1; then break; fi
    sleep 0.1
  done
  if kill -0 "$gui_pid" >/dev/null 2>&1; then
    echo "production-shaped complete-exit process did not exit" >&2
    exit 1
  fi
  wait "$gui_pid" >/dev/null 2>&1 || true
  gui_pid=""
  if "$cli_bin" --json --config-dir "$fixture_config" --data-dir "$fixture_data" daemon status >/dev/null 2>&1; then
    echo "complete exit left the isolated daemon running" >&2
    exit 1
  fi
  echo "OK: complete exit stops the isolated daemon"
  start_gui "$app_bin" 0
  wait_for_daemon || { echo "production bundle did not recover after complete exit" >&2; exit 1; }
  echo "OK: production bundle recovers after complete exit"
else
  echo "SKIP: complete-exit assertion requires --lifecycle-app built with TELEVYBACKUP_GUI_LIFECYCLE_TESTING=1" >&2
fi

echo "OK: release bundle HIL"
