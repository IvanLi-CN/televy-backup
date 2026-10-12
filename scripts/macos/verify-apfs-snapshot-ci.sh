#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage: verify-apfs-snapshot-ci.sh --app PATH [--evidence-dir PATH]

Runs the sanitized APFS timepoint and long-read checks against a disposable APFS fixture. The
application and mount helper must already be signed with the stable identity that has been granted
Full Disk Access on this dedicated runner.
USAGE
  exit 2
}

app=""
evidence_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app)
      app="${2:-}"
      shift 2
      ;;
    --evidence-dir)
      evidence_dir="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      ;;
    *)
      usage
      ;;
  esac
done

[[ -n "$app" && -d "$app" ]] || usage
app="$(cd "$app" && pwd -P)"
evidence_dir="${evidence_dir:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/televybackup-apfs-evidence}"
mkdir -p "$evidence_dir"
evidence_dir="$(cd "$evidence_dir" && pwd -P)"
chmod 700 "$evidence_dir"

for command in jq sqlite3 uuidgen diskutil python3 sudo; do
  command -v "$command" >/dev/null || {
    echo "required command is unavailable: $command" >&2
    exit 2
  }
done
sudo -n true 2>/dev/null || {
  echo "dedicated APFS runner requires passwordless sudo; interactive authorization is not supported" >&2
  exit 2
}

access_binary="$app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access"
mount_helper_binary="$app/Contents/MacOS/televybackup-snapshot-mount-helper"
cli_binary="$app/Contents/MacOS/televybackup-cli"
[[ -x "$access_binary" && -x "$mount_helper_binary" && -x "$cli_binary" ]] || {
  echo "candidate app is missing the Snapshot Access, mount helper, or CLI entrypoint" >&2
  exit 1
}

run_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
fixture_root="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/televybackup-apfs-fixture.XXXXXX")"
source_root="$fixture_root/source"
config_root="$fixture_root/config"
data_root="$fixture_root/data"
access_runtime_root="/tmp/televybackup-apfs-access-$run_id"
mount_runtime_root="/tmp/televybackup-apfs-mount-$run_id"
access_socket="$access_runtime_root/access.sock"
access_journal="$data_root/snapshot-access/journal.sqlite"
mount_socket="$mount_runtime_root/helper.sock"
mount_journal="$mount_runtime_root/journal.sqlite"
access_pid=""
mount_pid=""
a5_status=2
a6_status=2

write_failure_card() {
  local path="$1"
  local schema="$2"
  local result="$3"
  local reason="$4"
  jq -cn \
    --arg schema "$schema" \
    --arg result "$result" \
    --arg reason "$reason" \
    '{schema:$schema,result:$result,privacy:"sanitized",reason:$reason}' >"$path"
}

cleanup() {
  local exit_status=$?
  if [[ -n "$access_pid" ]]; then
    kill "$access_pid" >/dev/null 2>&1 || true
    wait "$access_pid" >/dev/null 2>&1 || true
  fi
  if [[ -n "$mount_pid" ]]; then
    sudo -n kill "$mount_pid" >/dev/null 2>&1 || true
    wait "$mount_pid" >/dev/null 2>&1 || true
  fi
  if [[ -d "$mount_runtime_root" ]]; then
    sudo -n rm -rf -- "$mount_runtime_root" >/dev/null 2>&1 || true
  fi
  rm -f -- \
    "$evidence_dir/mount-helper.log" \
    "$evidence_dir/access.log" \
    "$evidence_dir/a5.stdout" \
    "$evidence_dir/a5.stderr" \
    "$evidence_dir/a6.stdout" \
    "$evidence_dir/a6.stderr"
  rm -rf -- "$fixture_root" "$access_runtime_root"
  if [[ "$exit_status" -ne 0 && ! -s "$evidence_dir/summary.json" ]]; then
    write_failure_card \
      "$evidence_dir/summary.json" \
      "televybackup.apfs_snapshot_ci.v1" \
      "failed" \
      "orchestrator_failed"
  fi
}
trap cleanup EXIT

mkdir -p "$source_root" "$config_root" "$data_root" "$access_runtime_root"
chmod 700 "$fixture_root" "$source_root" "$config_root" "$data_root" "$access_runtime_root"
if [[ -n "$(find "$source_root" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  echo "APFS fixture source root must be empty before validation" >&2
  exit 1
fi
fixture_device="$(df -P "$source_root" 2>/dev/null | awk 'NR == 2 { print $1 }')"
[[ "$fixture_device" == /dev/* ]] || {
  echo "APFS fixture filesystem device could not be resolved" >&2
  exit 1
}
diskutil info "$fixture_device" 2>/dev/null | grep -Eiq 'File System Personality:[[:space:]]*APFS|Type:[[:space:]]*APFS' || {
  echo "APFS fixture source root is not on an APFS volume" >&2
  exit 1
}

python3 - "$config_root/config.toml" "$source_root" <<'PY'
import pathlib
import sys

config_path = pathlib.Path(sys.argv[1])
source_path = sys.argv[2].replace("\\", "\\\\").replace('"', '\\"')
config_path.write_text(
    'version = 2\n\n[[targets]]\nid = "apfs-ci"\nsource_path = "' + source_path + '"\n',
    encoding="utf-8",
)
PY

  # The user shell opens this private log before sudo; the root helper only inherits the fd.
  # shellcheck disable=SC2024
sudo -n env \
  TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET="$mount_socket" \
  TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL="$mount_journal" \
  "$mount_helper_binary" >"$evidence_dir/mount-helper.log" 2>&1 &
mount_pid=$!
for _ in $(seq 1 120); do
  [[ -S "$mount_socket" ]] && break
  sleep 0.1
done
[[ -S "$mount_socket" ]] || {
  echo "snapshot mount helper did not become ready" >&2
  exit 1
}
mount_status="$(
  TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET="$mount_socket" \
    TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL="$mount_journal" \
    "$cli_binary" --json --config-dir "$config_root" --data-dir "$data_root" \
    snapshot-mount-helper status 2>/dev/null || true
)"
printf '%s' "$mount_status" | jq -e '.helperVersion == "0.2.0" and .activeMounts == 0' >/dev/null || {
  echo "snapshot mount helper preflight did not report version 0.2.0 with no active mounts" >&2
  exit 1
}

TELEVYBACKUP_CONFIG_DIR="$config_root" \
  TELEVYBACKUP_DATA_DIR="$data_root" \
  TELEVYBACKUP_SNAPSHOT_SOCKET="$access_socket" \
  TELEVYBACKUP_SNAPSHOT_JOURNAL="$access_journal" \
  TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET="$mount_socket" \
  TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL="$mount_journal" \
  "$access_binary" >"$evidence_dir/access.log" 2>&1 &
access_pid=$!
for _ in $(seq 1 120); do
  [[ -S "$access_socket" ]] || {
    sleep 0.1
    continue
  }
  access_status="$(
    "$cli_binary" --json --config-dir "$config_root" --data-dir "$data_root" \
      snapshot-access status 2>/dev/null || true
  )"
  if printf '%s' "$access_status" | jq -e \
    '.serviceReachable == true and .fdaReady == true and .mountHelperReachable == true' >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
[[ -S "$access_socket" ]] || {
  echo "Snapshot Access did not create its isolated socket" >&2
  exit 1
}
printf '%s' "${access_status:-}" | jq -e \
  '.serviceReachable == true and .fdaReady == true and .mountHelperReachable == true' >/dev/null || {
  echo "Snapshot Access FDA or mount-helper preflight failed" >&2
  exit 1
}

repo_root="$(git rev-parse --show-toplevel)"
a5_output="$evidence_dir/a5.stdout"
set +e
TELEVYBACKUP_SANITIZED_FIXTURE_ROOT="$fixture_root" \
  bash "$repo_root/scripts/macos/verify-apfs-snapshot-integration.sh" \
  --sanitized "apfs-ci:$source_root" >"$a5_output" 2>"$evidence_dir/a5.stderr"
a5_status=$?
set -e
if [[ "$a5_status" -eq 0 ]] && jq -e \
  '.schema == "televybackup.apfs_snapshot_evidence.v1" and .result == "passed" and
   .privacy == "sanitized" and .cases == 1 and
   (.assertions | .snapshotReadPreMutation == true and .liveMutationObserved == true and
    .leasesReleased == true and .cleanupComplete == true)' \
  "$a5_output" >/dev/null; then
  jq -cn '{
    schema: "televybackup.apfs_snapshot_evidence.v1",
    result: "passed",
    privacy: "sanitized",
    cases: 1,
    assertions: {
      snapshotReadPreMutation: true,
      liveMutationObserved: true,
      leasesReleased: true,
      cleanupComplete: true
    }
  }' >"$evidence_dir/a5.json"
else
  write_failure_card \
    "$evidence_dir/a5.json" \
    "televybackup.apfs_snapshot_evidence.v1" \
    "failed" \
    "timepoint_validation_failed"
fi

if [[ "$a5_status" -eq 0 ]]; then
  a6_output="$evidence_dir/a6.stdout"
  set +e
  TELEVYBACKUP_SNAPSHOT_SOCKET="$access_socket" \
    TELEVYBACKUP_SNAPSHOT_JOURNAL="$access_journal" \
    TELEVYBACKUP_SANITIZED_FIXTURE_ROOT="$fixture_root" \
    bash "$repo_root/scripts/macos/verify-apfs-snapshot-long-read.sh" \
    --sanitized "apfs-ci:$source_root" >"$a6_output" 2>"$evidence_dir/a6.stderr"
  a6_status=$?
  set -e
  if [[ "$a6_status" -eq 0 ]] && jq -e \
    '.schema == "televybackup.apfs_snapshot_long_read.v1" and .result == "passed" and
     .privacy == "sanitized" and
     (.assertions | .healthyReadCompleted == true and .liveMutationRan == true and
      .mountHealthLossFailedClosed == true and .stageAndErrnoContext == true and
      .leasesReleased == true and .cleanupComplete == true and .publishedSnapshot == false)' \
    "$a6_output" >/dev/null; then
    jq -cn '{
      schema: "televybackup.apfs_snapshot_long_read.v1",
      result: "passed",
      privacy: "sanitized",
      assertions: {
        healthyReadCompleted: true,
        liveMutationRan: true,
        mountHealthLossFailedClosed: true,
        stageAndErrnoContext: true,
        leasesReleased: true,
        cleanupComplete: true,
        publishedSnapshot: false
      }
    }' >"$evidence_dir/a6.json"
  else
    write_failure_card \
      "$evidence_dir/a6.json" \
      "televybackup.apfs_snapshot_long_read.v1" \
      "failed" \
      "long_read_or_mount_health_validation_failed"
  fi
else
  a6_status=3
  write_failure_card \
    "$evidence_dir/a6.json" \
    "televybackup.apfs_snapshot_long_read.v1" \
    "blocked" \
    "timepoint_validation_failed"
fi

jq -n \
  --arg a5 "$(jq -r '.result' "$evidence_dir/a5.json")" \
  --arg a6 "$(jq -r '.result' "$evidence_dir/a6.json")" \
  --argjson a5_exit "$a5_status" \
  --argjson a6_exit "$a6_status" \
  '{
    schema: "televybackup.apfs_snapshot_ci.v1",
    result: (if $a5 == "passed" and $a6 == "passed" then "passed" else "failed" end),
    privacy: "sanitized",
    assertions: {
      apfsFixture: true,
      snapshotTimepoint: ($a5 == "passed"),
      healthyLongRead: ($a6 == "passed"),
      mountHealthLossFailedClosed: ($a6 == "passed"),
      noLiveFallback: ($a5 == "passed" and $a6 == "passed"),
      publishedSnapshot: false,
      rawLogsAttached: false
    },
    exits: {a5: $a5_exit, a6: $a6_exit}
  }' >"$evidence_dir/summary.json"
cat "$evidence_dir/summary.json"
[[ "$a5_status" -eq 0 && "$a6_status" -eq 0 ]]
