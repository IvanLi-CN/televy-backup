#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
build_root="$(mktemp -d /tmp/televybackup-webdav-app-hil-build.XXXXXX)"
test_root="$(mktemp -d /tmp/televybackup-webdav-app-hil.XXXXXX)"
app_pid=""

cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" >/dev/null 2>&1 || true
    wait "$app_pid" >/dev/null 2>&1 || true
  fi
  local cli="$build_root/macos-app/TelevyBackup.app/Contents/MacOS/televybackup-cli"
  if [[ -x "$cli" ]]; then
    "$cli" --json --config-dir "$test_root/config" --data-dir "$test_root/data" daemon stop >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

TELEVYBACKUP_GUI_LIFECYCLE_TESTING=1 \
  TELEVYBACKUP_APP_VARIANT=prod \
  TELEVYBACKUP_APP_OUT_ROOT="$build_root/macos-app" \
  TELEVYBACKUP_CODESIGN_IDENTITY=- \
  TELEVYBACKUP_BUILD_MODE=release \
  bash "$root_dir/scripts/macos/build-app.sh" >/dev/null

app="$build_root/macos-app/TelevyBackup.app"
app_bin="$app/Contents/MacOS/TelevyBackup"
cli="$app/Contents/MacOS/televybackup-cli"
config_dir="$test_root/config"
data_dir="$test_root/data"
result_path="$test_root/browse-result"
mkdir -p "$config_dir" "$data_dir/index" "$test_root/source"

cp "$root_dir/scripts/macos/fixtures/perf-idle/config.toml" "$config_dir/config.toml"
sed -i '' "s#__FIXTURE_A__#$test_root/source#g; s#__FIXTURE_B__#$test_root/source#g" \
  "$config_dir/config.toml"
cat >> "$config_dir/config.toml" <<EOF

[[targets]]
id = "browse-hil"
label = "Browse HIL"
source_path = "$test_root/source"
endpoint_id = "fixture"
enabled = false
EOF

sqlite3 "$data_dir/index/index.fixture.sqlite" < "$root_dir/crates/core/migrations/0001_init.sql"
sqlite3 "$data_dir/index/index.fixture.sqlite" \
  "INSERT INTO snapshots(snapshot_id,created_at,source_path,label,base_snapshot_id) VALUES('snapshot-hil-1','2026-09-23T04:00:00Z','$test_root/source','Browse HIL',NULL);"
mkdir -p "$data_dir/index/filemaps/fixture"
cp "$data_dir/index/index.fixture.sqlite" \
  "$data_dir/index/filemaps/fixture/snapshot-hil-1.sqlite"

TELEVYBACKUP_ALLOW_MULTI_INSTANCE=1 \
  TELEVYBACKUP_UI_DEMO=1 \
  TELEVYBACKUP_SHOW_POPOVER_ON_LAUNCH=0 \
  TELEVYBACKUP_DISABLE_KEYCHAIN=1 \
  TELEVYBACKUP_BROWSE_HIL_TARGET_ID=browse-hil \
  TELEVYBACKUP_BROWSE_HIL_RESULT="$result_path" \
  TELEVYBACKUP_BROWSE_HIL_NO_OPEN=1 \
  "$app_bin" --disable-keychain --config-dir "$config_dir" --data-dir "$data_dir" \
  >"$test_root/app.log" 2>&1 &
app_pid=$!

for _ in {1..300}; do
  [[ -s "$result_path" ]] && break
  sleep 0.1
done

grep -Fx 'ok:mounted' "$result_path" >/dev/null || {
  echo "ERROR: source-built App browse mount did not succeed" >&2
  cat "$result_path" 2>/dev/null || true
  tail -80 "$test_root/app.log" >&2 || true
  exit 1
}
echo "OK: source-built App browse mount HIL"
