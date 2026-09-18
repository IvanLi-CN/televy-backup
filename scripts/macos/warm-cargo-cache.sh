#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: warm-cargo-cache.sh --arch arm64|x86_64" >&2
  exit 2
}

arch=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) arch="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done

case "$arch" in
  arm64) cargo_target="aarch64-apple-darwin" ;;
  x86_64) cargo_target="x86_64-apple-darwin" ;;
  *) usage ;;
esac

# Populate the target cache before the packaging job starts. The workspace
# and MTProto helper use separate target trees, so compiling them concurrently
# removes the avoidable serial build time on a cold Intel runner.
workspace_log="$(mktemp)"
helper_log="$(mktemp)"
trap 'rm -f "$workspace_log" "$helper_log"' EXIT

cargo build --locked --release --target "$cargo_target" \
  -p televybackup \
  -p televybackupd \
  -p televybackup-snapshot-access >"$workspace_log" 2>&1 &
workspace_pid=$!
cargo build --manifest-path crates/mtproto-helper/Cargo.toml \
  --locked --release --target "$cargo_target" >"$helper_log" 2>&1 &
helper_pid=$!

workspace_status=0
helper_status=0
wait "$workspace_pid" || workspace_status=$?
wait "$helper_pid" || helper_status=$?
cat "$workspace_log" "$helper_log"
if [[ "$workspace_status" -ne 0 || "$helper_status" -ne 0 ]]; then
  exit 1
fi
