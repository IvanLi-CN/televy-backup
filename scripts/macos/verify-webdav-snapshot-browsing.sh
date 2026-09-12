#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "SKIP: daemon WebDAV verification requires macOS" >&2
  exit 0
fi
root_dir="$(git rev-parse --show-toplevel)"
if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: cargo is required for the daemon WebDAV verification" >&2
  exit 1
fi

expected_tests=(
  webdav_service_closes_an_established_connection_after_unmount
  webdav_service_lists_snapshot_children_for_encoded_snapshot_directory
)
test_list="$(cargo test --manifest-path "$root_dir/Cargo.toml" -p televybackupd webdav_service -- --list)"
for test_name in "${expected_tests[@]}"; do
  full_test_name="snapshot_browse::tests::$test_name"
  grep -F "$full_test_name" <<<"$test_list" >/dev/null || {
    echo "ERROR: expected daemon WebDAV test is missing: $test_name" >&2
    exit 1
  }
  cargo test --manifest-path "$root_dir/Cargo.toml" -p televybackupd "$full_test_name" -- --exact --nocapture
done
if [[ "${TELEVYBACKUP_RUN_WEBDAV_MOUNT_ACCEPTANCE:-0}" == "1" ]]; then
  full_test_name="snapshot_browse::tests::webdav_mount_webdav_enumerates_copies_and_recovers"
  mount_test_list="$(cargo test --manifest-path "$root_dir/Cargo.toml" -p televybackupd snapshot_browse::tests -- --list)"
  grep -F "$full_test_name" <<<"$mount_test_list" >/dev/null || {
    echo "ERROR: expected macOS WebDAV mount acceptance test is missing" >&2
    exit 1
  }
  cargo test --manifest-path "$root_dir/Cargo.toml" -p televybackupd "$full_test_name" -- --exact --ignored --nocapture
else
  echo "SKIP: set TELEVYBACKUP_RUN_WEBDAV_MOUNT_ACCEPTANCE=1 for real mount_webdav acceptance" >&2
fi
echo "OK: daemon-owned WebDAV service passed the macOS daemon checks"
