#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null || ! command -v uuidgen >/dev/null || ! command -v sqlite3 >/dev/null; then
  echo "jq, uuidgen, and sqlite3 are required" >&2
  exit 2
fi

sanitized=false
if [[ "${1:-}" == "--sanitized" ]]; then
  sanitized=true
  shift
fi

if [[ "$#" -ne 1 ]]; then
  cat >&2 <<'USAGE'
Usage: verify-apfs-snapshot-long-read.sh [--sanitized] TARGET_ID:SOURCE_PATH

Runs a controlled long-read acceptance against an already running Snapshot Access process. The
healthy case reads a generated large file while a live control file changes. The fault case
unmounts only the lease mount root and requires the next stream read to fail closed with a stage
and errno diagnostic. The fixture must be disposable and must not contain user data.

Required environment:
  TELEVYBACKUP_SNAPSHOT_SOCKET
  TELEVYBACKUP_SNAPSHOT_JOURNAL
USAGE
  exit 2
fi

target_spec="$1"
target_id="${target_spec%%:*}"
source_path="${target_spec#*:}"
if [[ "$target_id" == "$target_spec" || -z "$source_path" || ! -d "$source_path" ]]; then
  echo "invalid target specification" >&2
  exit 2
fi

socket_path="${TELEVYBACKUP_SNAPSHOT_SOCKET:?TELEVYBACKUP_SNAPSHOT_SOCKET is required}"
journal_path="${TELEVYBACKUP_SNAPSHOT_JOURNAL:?TELEVYBACKUP_SNAPSHOT_JOURNAL is required}"
integration_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
large_file="$source_path/.televybackup-apfs-long-read-$integration_id"
control_file="$source_path/.televybackup-apfs-live-control-$integration_id"
large_file_size=$((32 * 1024 * 1024))
leases=()
streams=()
mutator_pid=""

request() {
  local payload="$1"
  local response
  response="$(perl -MIO::Socket::UNIX -e '
    my ($socket_path, $request) = @ARGV;
    my $socket = IO::Socket::UNIX->new(Type => SOCK_STREAM, Peer => $socket_path)
      or die "connect: $!\n";
    print {$socket} $request, "\n" or die "write: $!\n";
    shutdown($socket, 1) or die "shutdown: $!\n";
    my $response = <$socket>;
    print $response if defined $response;
  ' "$socket_path" "$payload")"
  if [[ -z "$response" ]]; then
    echo "empty response from Snapshot Access" >&2
    return 1
  fi
  printf '%s' "$response"
}

release_lease() {
  local lease_id="$1"
  request "$(jq -cn --arg request_id "$(uuidgen)" --arg lease_id "$lease_id" \
    '{version:2,request_id:$request_id,method:"release_lease",lease_id:$lease_id}')" >/dev/null || true
}

close_stream() {
  local stream_id="$1"
  request "$(jq -cn --arg request_id "$(uuidgen)" --arg stream_id "$stream_id" \
    '{version:2,request_id:$request_id,method:"close_read_stream",stream_id:$stream_id}')" >/dev/null || true
}

cleanup() {
  local exit_status=$?
  if [[ -n "$mutator_pid" ]]; then
    kill "$mutator_pid" 2>/dev/null || true
    wait "$mutator_pid" 2>/dev/null || true
  fi
  local stream_id lease_id
  for stream_id in "${streams[@]:-}"; do
    close_stream "$stream_id"
  done
  for lease_id in "${leases[@]:-}"; do
    release_lease "$lease_id"
  done
  rm -f -- "$large_file" "$control_file"
  if [[ "$exit_status" -ne 0 && "$sanitized" == true ]]; then
    printf '%s\n' '{"schema":"televybackup.apfs_snapshot_long_read.v1","result":"failed","privacy":"sanitized"}' >&2
  fi
}
trap cleanup EXIT

dd if=/dev/zero of="$large_file" bs=1048576 count=32 status=none
printf 'live-control-initial\n' > "$control_file"

probe_response="$(request "$(jq -cn --arg request_id "$(uuidgen)" --arg target_id "$target_id" \
  '{version:2,request_id:$request_id,method:"probe_volume",target_id:$target_id}')")"
printf '%s' "$probe_response" | jq -e '.ok and .result.kind == "probe" and .result.snapshot_supported' >/dev/null
volume_uuid="$(printf '%s' "$probe_response" | jq -er '.result.volume_uuid')"

acquire() {
  local run_label="$1"
  local response lease_id
  response="$(request "$(jq -cn --arg request_id "$(uuidgen)" --arg target_id "$target_id" \
    --arg volume_uuid "$volume_uuid" --arg run_id "apfs-long-read-$integration_id-$run_label" \
    '{version:2,request_id:$request_id,method:"acquire_lease",target_id:$target_id,expected_volume_uuid:$volume_uuid,run_id:$run_id}')")"
  printf '%s' "$response" | jq -e '.ok and .result.kind == "lease"' >/dev/null
  lease_id="$(printf '%s' "$response" | jq -er '.result.lease_id')"
  leases+=("$lease_id")
  printf '%s' "$lease_id"
}

mount_root_for() {
  local lease_id="$1"
  sqlite3 "$journal_path" "SELECT mount_root FROM leases WHERE lease_id = '$lease_id';"
}

open_stream() {
  local lease_id="$1"
  local response
  response="$(request "$(jq -cn --arg request_id "$(uuidgen)" --arg lease_id "$lease_id" \
    --arg relative_path "$(basename "$large_file")" \
    '{version:2,request_id:$request_id,method:"open_read_stream",lease_id:$lease_id,relative_path:$relative_path}')")"
  printf '%s' "$response" | jq -e '.ok and .result.kind == "read_stream"' >/dev/null
  printf '%s' "$response" | jq -er '.result.stream_id'
}

read_stream_fully() {
  local stream_id="$1"
  local total=0
  local eof=false
  while [[ "$eof" != true ]]; do
    local response chunk_size
    response="$(request "$(jq -cn --arg request_id "$(uuidgen)" --arg stream_id "$stream_id" \
      '{version:2,request_id:$request_id,method:"read_stream",stream_id:$stream_id,max_bytes:1048576}')")"
    printf '%s' "$response" | jq -e '.ok and .result.kind == "read_stream"' >/dev/null
    chunk_size="$(printf '%s' "$response" | jq -er '.result.bytes_base64' | base64 -D | wc -c | tr -d ' ')"
    total=$((total + chunk_size))
    eof="$(printf '%s' "$response" | jq -er '.result.eof')"
  done
  printf '%s' "$total"
}

healthy_lease="$(acquire healthy)"
healthy_stream="$(open_stream "$healthy_lease")"
streams+=("$healthy_stream")
(
  for tick in $(seq 1 80); do
    printf 'live-control-%s\n' "$tick" > "$control_file"
    sleep 0.05
  done
) &
mutator_pid=$!
healthy_bytes="$(read_stream_fully "$healthy_stream")"
wait "$mutator_pid"
mutator_pid=""
[[ "$healthy_bytes" -eq "$large_file_size" ]] || {
  echo "healthy long read returned $healthy_bytes bytes; expected $large_file_size" >&2
  exit 1
}
close_stream "$healthy_stream"
streams=()
release_lease "$healthy_lease"
leases=()

fault_lease="$(acquire fault)"
fault_stream="$(open_stream "$fault_lease")"
streams+=("$fault_stream")
fault_mount_root="$(mount_root_for "$fault_lease")"
[[ -n "$fault_mount_root" && -d "$fault_mount_root" ]] || {
  echo "fault lease mount root is unavailable" >&2
  exit 1
}
sudo -n /sbin/umount -f "$fault_mount_root"
fault_response="$(request "$(jq -cn --arg request_id "$(uuidgen)" --arg stream_id "$fault_stream" \
  '{version:2,request_id:$request_id,method:"read_stream",stream_id:$stream_id,max_bytes:1048576}')")"
printf '%s' "$fault_response" | jq -e '.ok == false and (.code == "io" or .code == "invalid_request")' >/dev/null
fault_message="$(printf '%s' "$fault_response" | jq -er '.message')"
[[ "$fault_message" == *"snapshot read failed"* && "$fault_message" == *"errno="* ]] || {
  echo "fault response did not contain sanitized stage and errno context" >&2
  exit 1
}
close_stream "$fault_stream"
streams=()
release_lease "$fault_lease"
leases=()

status_response="$(request "$(jq -cn --arg request_id "$(uuidgen)" \
  '{version:2,request_id:$request_id,method:"status"}')")"
printf '%s' "$status_response" | jq -e '.ok and .result.active_leases == 0 and .result.pending_cleanup == 0' >/dev/null

if [[ "$sanitized" == true ]]; then
  printf '%s\n' '{"schema":"televybackup.apfs_snapshot_long_read.v1","result":"passed","privacy":"sanitized","assertions":{"healthyReadCompleted":true,"liveMutationRan":true,"mountHealthLossFailedClosed":true,"stageAndErrnoContext":true,"leasesReleased":true,"cleanupComplete":true,"publishedSnapshot":false}}'
else
  echo "APFS long-read and mount-health verification passed"
fi
