#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

cargo build -q -p televybackup
mkdir -p "$tmp_dir/bin" "$tmp_dir/config-a" "$tmp_dir/data-a" "$tmp_dir/config-b" "$tmp_dir/data-b"
cp "$root_dir/target/debug/televybackup" "$tmp_dir/bin/televybackup"
printf '#!/bin/sh\nexit 0\n' > "$tmp_dir/bin/televybackupd"
printf '#!/bin/sh\nexit 0\n' > "$tmp_dir/bin/televybackup-mtproto-helper"
chmod 755 "$tmp_dir/bin/"*
printf 'preserve\n' > "$tmp_dir/data-a/backup-marker"

service_env=(
  TELEVYBACKUP_SERVICE_ROOT="$tmp_dir/service"
  TELEVYBACKUP_LAUNCHAGENT_PLIST="$tmp_dir/LaunchAgents/com.ivan.televybackup.daemon.plist"
  TELEVYBACKUP_LAUNCHCTL=/usr/bin/true
)

output="$(env "${service_env[@]}" "$tmp_dir/bin/televybackup" --json --config-dir "$tmp_dir/config-a" --data-dir "$tmp_dir/data-a" daemon install-service)"
printf '%s\n' "$output" | grep -F '"installed":true' >/dev/null

printf 'changed\n' >> "$tmp_dir/bin/televybackupd"
failing_env=("${service_env[@]}" TELEVYBACKUP_LAUNCHCTL=/usr/bin/false)
if env "${failing_env[@]}" "$tmp_dir/bin/televybackup" --json --config-dir "$tmp_dir/config-a" --data-dir "$tmp_dir/data-a" daemon install-service >/dev/null 2>&1; then
  echo 'service install should fail when launchctl bootstrap fails' >&2
  exit 1
fi
grep -F 'config-a' "$tmp_dir/LaunchAgents/com.ivan.televybackup.daemon.plist" >/dev/null

if env "${service_env[@]}" "$tmp_dir/bin/televybackup" --json --config-dir "$tmp_dir/config-b" --data-dir "$tmp_dir/data-b" daemon install-service >/dev/null 2>&1; then
  echo 'service install should fail for a different environment without --replace' >&2
  exit 1
fi

output="$(env "${service_env[@]}" "$tmp_dir/bin/televybackup" --json --config-dir "$tmp_dir/config-b" --data-dir "$tmp_dir/data-b" daemon install-service --replace)"
printf '%s\n' "$output" | grep -F '"replaced":true' >/dev/null
output="$(env "${service_env[@]}" "$tmp_dir/bin/televybackup" --json --config-dir "$tmp_dir/config-b" --data-dir "$tmp_dir/data-b" daemon service-status)"
printf '%s\n' "$output" | grep -F '"environmentMatch":true' >/dev/null

env "${service_env[@]}" "$tmp_dir/bin/televybackup" --json --config-dir "$tmp_dir/config-b" --data-dir "$tmp_dir/data-b" daemon uninstall-service >/dev/null
[[ -f "$tmp_dir/data-a/backup-marker" ]]
[[ ! -e "$tmp_dir/service" ]]
echo 'daemon service transaction contract tests passed'
