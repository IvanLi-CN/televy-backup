#!/usr/bin/env bash
set -euo pipefail

[[ "${GITHUB_ACTIONS:-}" == true ]] || { echo 'GitHub Actions runner required' >&2; exit 1; }
root_dir="$(git rev-parse --show-toplevel)"
fixture_dir="$(mktemp -d)"
source_file="$root_dir/crates/cli/src/snapshot_mount_service.rs"
cp "$source_file" "$fixture_dir/candidate.rs"
cleanup() {
  cp "$fixture_dir/candidate.rs" "$source_file"
  rm -rf "$fixture_dir"
}
trap cleanup EXIT
cp "$source_file" "$fixture_dir/four-probes.rs"
python3 - "$fixture_dir/four-probes.rs" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
old = 'while elapsed() < INSTALL_READY_TIMEOUT {'
assert text.count(old) == 1, 'startup loop changed; update the regression mutation'
text = text.replace(old, 'for attempt in 0..4 {')
assert text.count('if !remaining.is_zero() {') == 1
p.write_text(text.replace('if !remaining.is_zero() {', 'if attempt < 3 && !remaining.is_zero() {'))
PY
diff -u "$source_file" "$fixture_dir/four-probes.rs" || test "$?" -eq 1
cp "$fixture_dir/four-probes.rs" "$source_file"
if cargo test --locked -p televybackup \
  snapshot_mount_service::tests::install_waits_through_connection_refusal_until_slow_startup_is_ready \
  -- --exact > "$fixture_dir/legacy-result.txt" 2>&1; then
  echo 'Four fast startup probes unexpectedly passed' >&2
  exit 1
fi
cat "$fixture_dir/legacy-result.txt"
grep -F 'slow-starting helper must not exhaust four immediate refusals' "$fixture_dir/legacy-result.txt"
cp "$fixture_dir/candidate.rs" "$source_file"
cmp "$source_file" "$fixture_dir/candidate.rs"
cargo test --locked -p televybackup snapshot_mount_service::tests
echo 'PASS: four-probe startup regression is red and candidate startup contracts are green'
