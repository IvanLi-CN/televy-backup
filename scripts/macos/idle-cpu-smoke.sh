#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
perf_root="$root_dir/.dev/perf-idle"
config_dir="$perf_root/config"
data_dir="$perf_root/data"
fixture_dir="$perf_root/fixtures"
artifact_dir="$root_dir/target/perf-idle"
app="$root_dir/target/macos-app/TelevyBackup Dev.app"
app_bin="$app/Contents/MacOS/TelevyBackup"
daemon_bin="$app/Contents/MacOS/televybackupd"
cli_bin="$app/Contents/MacOS/televybackup-cli"
samples="$artifact_dir/cpu-samples.txt"
report="$artifact_dir/report.txt"
duration="${TELEVYBACKUP_CPU_SAMPLE_SECONDS:-30}"

mkdir -p "$config_dir" "$data_dir" "$fixture_dir/a" "$fixture_dir/b" "$artifact_dir"
sed \
  -e "s|__FIXTURE_A__|$fixture_dir/a|g" \
  -e "s|__FIXTURE_B__|$fixture_dir/b|g" \
  "$root_dir/scripts/macos/fixtures/perf-idle/config.toml" > "$config_dir/config.toml"

TELEVYBACKUP_APP_VARIANT=dev TELEVYBACKUP_CODESIGN_IDENTITY=- "$root_dir/scripts/macos/build-app.sh"

for exact_bin in "$app_bin" "$daemon_bin"; do
  while IFS= read -r old_pid; do
    [ -n "$old_pid" ] && kill -TERM "$old_pid"
  done < <(pgrep -f "$exact_bin" || true)
done
while IFS= read -r old_pid; do
  [ -n "$old_pid" ] && kill -TERM "$old_pid"
done < <(pgrep -f "$cli_bin --json status stream" || true)
sleep 1

open -n "$app" --args \
  --disable-keychain \
  --config-dir "$config_dir" \
  --data-dir "$data_dir" \
  --open-main-window

gui_pid=""
for _ in {1..100}; do
  gui_pid="$(pgrep -f "$app_bin" | head -1 || true)"
  [ -n "$gui_pid" ] && break
  sleep 0.1
done
[ -n "$gui_pid" ] || { echo "ERROR: Dev GUI did not start" >&2; exit 1; }

bundle_id="$(lsappinfo info -only bundleid -pid "$gui_pid" | tr -d '"' | awk -F= '{print $2}')"
[ "$bundle_id" = "com.ivan.televybackup.dev" ] || {
  echo "ERROR: unexpected bundle id: $bundle_id" >&2
  exit 1
}

gui_command="$(ps -p "$gui_pid" -o command=)"
case "$gui_command" in
  *--disable-keychain*"$config_dir"*"$data_dir"*) ;;
  *) echo "ERROR: Dev GUI is not using the isolated no-Keychain arguments" >&2; exit 1 ;;
esac

daemon_pid=""
cli_pid=""
for _ in {1..100}; do
  daemon_pid="$(pgrep -f "$daemon_bin" | head -1 || true)"
  cli_pid="$(pgrep -f "$cli_bin --json status stream" | head -1 || true)"
  [ -n "$daemon_pid" ] && [ -n "$cli_pid" ] && break
  sleep 0.1
done
[ -n "$daemon_pid" ] && [ -n "$cli_pid" ] || {
  echo "ERROR: bundled daemon/status CLI did not start" >&2
  exit 1
}

status_json="$data_dir/status/status.json"
for _ in {1..100}; do
  [ -s "$status_json" ] && break
  sleep 0.1
done
[ -s "$status_json" ] || { echo "ERROR: daemon status stream fixture is missing" >&2; exit 1; }
target_count="$(/usr/bin/python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["targets"]))' "$status_json")"
[ "$target_count" = "2" ] || { echo "ERROR: expected two fixture targets, got $target_count" >&2; exit 1; }

if lsof -p "$gui_pid,$daemon_pid,$cli_pid" 2>/dev/null | rg -q "$HOME/Library/Application Support/TelevyBackup"; then
  echo "ERROR: isolated Dev process has a production directory handle" >&2
  exit 1
fi

sleep 5
: > "$samples"
first_generated_at=""
last_generated_at=""
for second in $(seq 1 "$duration"); do
  for live_pid in "$gui_pid" "$daemon_pid" "$cli_pid"; do
    kill -0 "$live_pid" 2>/dev/null || {
      sample "$gui_pid" 5 1 -file "$artifact_dir/TelevyBackup.sample.txt" >/dev/null 2>&1 || true
      echo "ERROR: fixture process $live_pid exited during sampling; Dev instance preserved" >&2
      exit 1
    }
  done
  gui_cpu="$(ps -p "$gui_pid" -o %cpu= | tr -d ' ')"
  daemon_cpu="$(ps -p "$daemon_pid" -o %cpu= | tr -d ' ')"
  cli_cpu="$(ps -p "$cli_pid" -o %cpu= | tr -d ' ')"
  generated_at="$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["generatedAt"])' "$status_json")"
  [ -n "$first_generated_at" ] || first_generated_at="$generated_at"
  last_generated_at="$generated_at"
  [ -n "$gui_cpu" ] || { echo "ERROR: Dev GUI exited during sampling" >&2; exit 1; }
  printf '%s gui=%s daemon=%s cli=%s\n' "$second" "$gui_cpu" "$daemon_cpu" "$cli_cpu" >> "$samples"
  sleep 1
done

avg="$(awk -F'[ =]' '{sum += $3} END {printf "%.2f", sum / NR}' "$samples")"
peak="$(awk -F'[ =]' 'BEGIN {max=0} $3 > max {max=$3} END {printf "%.2f", max}' "$samples")"
daemon_avg="$(awk -F'[ =]' '{sum += $5} END {printf "%.2f", sum / NR}' "$samples")"
daemon_peak="$(awk -F'[ =]' 'BEGIN {max=0} $5 > max {max=$5} END {printf "%.2f", max}' "$samples")"
cli_avg="$(awk -F'[ =]' '{sum += $7} END {printf "%.2f", sum / NR}' "$samples")"
cli_peak="$(awk -F'[ =]' 'BEGIN {max=0} $7 > max {max=$7} END {printf "%.2f", max}' "$samples")"
printf 'gui_pid=%s\ndaemon_pid=%s\ncli_pid=%s\navg_cpu=%s\npeak_cpu=%s\ndaemon_avg_cpu=%s\ndaemon_peak_cpu=%s\ncli_avg_cpu=%s\ncli_peak_cpu=%s\nfirst_generated_at=%s\nlast_generated_at=%s\n' \
  "$gui_pid" "$daemon_pid" "$cli_pid" "$avg" "$peak" "$daemon_avg" "$daemon_peak" "$cli_avg" "$cli_peak" "$first_generated_at" "$last_generated_at" > "$report"

if [ "$first_generated_at" = "$last_generated_at" ]; then
  sample "$gui_pid" 5 1 -file "$artifact_dir/TelevyBackup.sample.txt" >/dev/null 2>&1 || true
  echo "ERROR: status generatedAt did not advance during sampling; Dev instance preserved" >&2
  exit 1
fi

if ! awk -v avg="$avg" -v peak="$peak" -v da="$daemon_avg" -v dp="$daemon_peak" -v ca="$cli_avg" -v cp="$cli_peak" \
  'BEGIN {exit !(avg < 5 && peak < 20 && da < 5 && dp < 20 && ca < 5 && cp < 20)}'; then
  sample "$gui_pid" 5 1 -file "$artifact_dir/TelevyBackup.sample.txt" >/dev/null 2>&1 || true
  echo "ERROR: GUI CPU threshold failed (avg=$avg peak=$peak); Dev instance preserved" >&2
  exit 1
fi

echo "OK: idle CPU gui=$avg%/$peak% daemon=$daemon_avg%/$daemon_peak% cli=$cli_avg%/$cli_peak% (avg/peak; Dev instance preserved)"
