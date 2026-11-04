#!/usr/bin/env bash
set -euo pipefail
root_dir="$(git rev-parse --show-toplevel)"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
ulimit -c 0
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
source_file="$root_dir/macos/TelevyBackupApp/SnapshotMountHelperControlAction.swift"
test_file="$root_dir/macos/TelevyBackupAppTests/SnapshotMountHelperControlActionTests.swift"
cp "$source_file" "$fixture_dir/unguarded.swift"
python3 - "$fixture_dir/unguarded.swift" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1]); s = p.read_text()
fence = 'guard request == generation else { return false }'
assert s.count(fence) == 1, 'Refresh fence changed; update regression mutation'
p.write_text(s.replace(fence, '// Legacy callback accepts an out-of-order response.'))
PY
"$(xcrun --find swiftc)" -sdk "$sdk_path" "$fixture_dir/unguarded.swift" "$test_file" -o "$fixture_dir/unguarded"
python3 - "$fixture_dir/unguarded" <<'PY'
import subprocess, sys, os
r = subprocess.run([sys.argv[1]], capture_output=True, text=True,
                   env={**os.environ, 'SWIFT_BACKTRACE': 'disable'})
assert r.returncode != 0, 'Unguarded stale response unexpectedly passed'
assert 'A delayed old response must not restore stale blockers' in r.stderr, 'Unexpected mutation failure'
print('RED: late stale response restores 72 blockers without the production generation fence')
PY
"$(xcrun --find swiftc)" -sdk "$sdk_path" "$source_file" "$test_file" -o "$fixture_dir/candidate"
"$fixture_dir/candidate"
# Check that the production Settings call site actually uses the tested admission fence.
python3 - "$root_dir/macos/TelevyBackupApp/SettingsWindow.swift" <<'PY'
from pathlib import Path
import sys
s = Path(sys.argv[1]).read_text()
a = s.index('    private func refreshSnapshotStatus(')
b = s.index('    private func recoverSnapshotMountRecords', a)
f = s[a:b]
assert f.index('snapshotRefresh.complete(') < f.index('if let status { snapshotStatus = status }')
assert 'snapshotRecoveryPollLimit' not in s, 'Fixed poll counts must not declare daemon work failed'
assert 'SnapshotMountOperationProgressView(' in s
assert 'refreshSnapshotStatus(background: true)' in s
assert 'get { settingsStore.snapshotMountOperation }' in s
assert 'snapshotError = preservingError' not in f.replace('if let preservingError { snapshotError = preservingError }', '')
assert f.index('if let preservingError { snapshotError = preservingError }') < f.index('snapshotRefresh.begin()')
print('GREEN: Settings applies the tested latest-response guard and keeps polling daemon work')
PY
bash "$root_dir/scripts/macos/ui-store-static-check.sh"
