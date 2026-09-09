#!/usr/bin/env bash
set -euo pipefail

if ! command -v jq >/dev/null; then
  echo "jq is required" >&2
  exit 2
fi

sanitized=false
if [ "${1:-}" = "--sanitized" ]; then
  sanitized=true
  shift
fi

if [ "$#" -eq 0 ]; then
  cat >&2 <<'USAGE'
Usage: verify-apfs-snapshot-integration.sh [--sanitized] TARGET_ID:SOURCE_PATH [...]

Creates one small, uniquely named test file inside each source root. For every target it creates an
APFS snapshot lease, changes only that test file in the live source, verifies the brokered snapshot
stream still returns the original bytes, then releases the lease and deletes the test file.

--sanitized emits a shareable JSON assertion report. It removes the temporary raw IPC transcript and
does not print source paths, target IDs, volume identifiers, snapshot identifiers, mount roots, or
test bytes.
USAGE
  exit 2
fi

integration_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
data_dir="${TELEVYBACKUP_DATA_DIR:-$HOME/Library/Application Support/TelevyBackup}"
socket_path="${TELEVYBACKUP_SNAPSHOT_SOCKET:-$data_dir/snapshot-access/access.sock}"
if "$sanitized"; then
  evidence_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-snapshot-evidence.XXXXXX")"
  chmod 700 "$evidence_dir"
  evidence_path="$evidence_dir/transcript.jsonl"
else
  evidence_dir="${TELEVYBACKUP_SNAPSHOT_EVIDENCE_DIR:-$PWD/target/apfs-snapshot-integration}"
  evidence_path="$evidence_dir/$integration_id.jsonl"
fi

mkdir -p "$evidence_dir"
test_files=()
leases=()
last_response=""
completed_cases=0

request() {
  local payload="$1"
  if ! last_response="$(perl -MIO::Socket::UNIX -e '
    my ($socket_path, $request) = @ARGV;
    my $socket = IO::Socket::UNIX->new(
      Type => SOCK_STREAM,
      Peer => $socket_path,
    ) or die "connect: $!\n";
    print {$socket} $request, "\n" or die "write: $!\n";
    shutdown($socket, 1) or die "shutdown: $!\n";
    my $response = <$socket>;
    print $response if defined $response;
  ' "$socket_path" "$payload" 2>"$evidence_dir/socket-error")"; then
    if "$sanitized"; then
      echo "Snapshot Access request failed" >&2
    else
      cat "$evidence_dir/socket-error" >&2
    fi
    return 1
  fi
  if [ -z "$last_response" ]; then
    echo "empty response from Snapshot Access" >&2
    return 1
  fi
  if "$sanitized"; then
    printf '%s\n' "$last_response" >>"$evidence_path"
  else
    printf '%s\n' "$last_response" | tee -a "$evidence_path"
  fi
}

release_lease() {
  local lease_id="$1"
  local request_id
  request_id="$(uuidgen)"
  request "$(jq -cn --arg request_id "$request_id" --arg lease_id "$lease_id" \
    '{version:2,request_id:$request_id,method:"release_lease",lease_id:$lease_id}')" >/dev/null || true
}

cleanup() {
  local exit_status=$?
  local lease_id test_file
  for lease_id in "${leases[@]:-}"; do
    release_lease "$lease_id"
  done
  for test_file in "${test_files[@]:-}"; do
    rm -f -- "$test_file"
  done
  if "$sanitized"; then
    rm -f -- "$evidence_path" "$evidence_dir/socket-error"
    rmdir "$evidence_dir" 2>/dev/null || true
    if [ "$exit_status" -ne 0 ]; then
      printf '%s\n' "{\"schema\":\"televybackup.apfs_snapshot_evidence.v1\",\"result\":\"failed\",\"completedCases\":$completed_cases,\"privacy\":\"sanitized\"}" >&2
    fi
  fi
}
trap cleanup EXIT

if [ ! -S "$socket_path" ]; then
  if "$sanitized"; then
    echo "Snapshot Access socket is unavailable" >&2
  else
    echo "Snapshot Access socket is unavailable: $socket_path" >&2
  fi
  exit 1
fi

for target_spec in "$@"; do
  target_id="${target_spec%%:*}"
  source_path="${target_spec#*:}"
  if [ "$target_id" = "$target_spec" ] || [ -z "$source_path" ] || [ ! -d "$source_path" ]; then
    if "$sanitized"; then
      echo "invalid target specification" >&2
    else
      echo "invalid target specification: $target_spec" >&2
    fi
    exit 2
  fi

  basename=".televybackup-snapshot-probe-$integration_id"
  test_file="$source_path/$basename"
  original="snapshot-original-$integration_id-$target_id"
  changed="live-changed-$integration_id-$target_id"
  printf '%s' "$original" >"$test_file"
  test_files+=("$test_file")

  probe_request_id="$(uuidgen)"
  request "$(jq -cn --arg request_id "$probe_request_id" --arg target_id "$target_id" \
    '{version:2,request_id:$request_id,method:"probe_volume",target_id:$target_id}')" >/dev/null
  probe_response="$last_response"
  printf '%s' "$probe_response" | jq -e '.ok and .result.kind == "probe" and .result.snapshot_supported' >/dev/null
  volume_uuid="$(printf '%s' "$probe_response" | jq -er '.result.volume_uuid')"

  lease_request_id="$(uuidgen)"
  request "$(jq -cn --arg request_id "$lease_request_id" --arg target_id "$target_id" --arg volume_uuid "$volume_uuid" --arg run_id "integration-$integration_id-$target_id" \
    '{version:2,request_id:$request_id,method:"acquire_lease",target_id:$target_id,expected_volume_uuid:$volume_uuid,run_id:$run_id}')" >/dev/null
  lease_response="$last_response"
  printf '%s' "$lease_response" | jq -e '.ok and .result.kind == "lease"' >/dev/null
  lease_id="$(printf '%s' "$lease_response" | jq -er '.result.lease_id')"
  leases+=("$lease_id")

  printf '%s' "$changed" >"$test_file"
  read_request_id="$(uuidgen)"
  request "$(jq -cn --arg request_id "$read_request_id" --arg lease_id "$lease_id" --arg relative_path "$basename" \
    '{version:2,request_id:$request_id,method:"open_read_stream",lease_id:$lease_id,relative_path:$relative_path}')" >/dev/null
  open_response="$last_response"
  printf '%s' "$open_response" | jq -e '.ok and .result.kind == "read_stream"' >/dev/null
  stream_id="$(printf '%s' "$open_response" | jq -er '.result.stream_id')"

  stream_request_id="$(uuidgen)"
  request "$(jq -cn --arg request_id "$stream_request_id" --arg stream_id "$stream_id" \
    '{version:2,request_id:$request_id,method:"read_stream",stream_id:$stream_id,max_bytes:1048576}')" >/dev/null
  read_response="$last_response"
  printf '%s' "$read_response" | jq -e '.ok and .result.kind == "read_stream"' >/dev/null
  snapshot_bytes="$(printf '%s' "$read_response" | jq -er '.result.bytes_base64' | base64 -D)"
  if [ "$snapshot_bytes" != "$original" ] || [ "$(cat "$test_file")" != "$changed" ]; then
    if "$sanitized"; then
      echo "snapshot byte verification failed" >&2
    else
      echo "snapshot byte verification failed for $target_spec" >&2
    fi
    exit 1
  fi

  close_request_id="$(uuidgen)"
  request "$(jq -cn --arg request_id "$close_request_id" --arg stream_id "$stream_id" \
    '{version:2,request_id:$request_id,method:"close_read_stream",stream_id:$stream_id}')" >/dev/null
  release_lease "$lease_id"
  leases=("${leases[@]:0:${#leases[@]}-1}")
  rm -f -- "$test_file"
  test_files=("${test_files[@]:0:${#test_files[@]}-1}")
  completed_cases=$((completed_cases + 1))
done

status_request_id="$(uuidgen)"
request "$(jq -cn --arg request_id "$status_request_id" \
  '{version:2,request_id:$request_id,method:"status"}')" >/dev/null
status_response="$last_response"
printf '%s' "$status_response" | jq -e '.ok and .result.active_leases == 0 and .result.pending_cleanup == 0' >/dev/null

if "$sanitized"; then
  printf '%s\n' "{\"schema\":\"televybackup.apfs_snapshot_evidence.v1\",\"result\":\"passed\",\"cases\":$completed_cases,\"assertions\":{\"snapshotReadPreMutation\":true,\"liveMutationObserved\":true,\"leasesReleased\":true,\"cleanupComplete\":true},\"redacted\":[\"sourcePath\",\"targetId\",\"volumeUuid\",\"snapshotUuid\",\"mountRoot\",\"testBytes\",\"rawIpc\"]}"
else
  echo "APFS snapshot integration verification passed: $evidence_path"
fi
