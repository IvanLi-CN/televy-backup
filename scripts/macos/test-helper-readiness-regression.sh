#!/usr/bin/env bash
set -euo pipefail

# Run only on the hosted macOS fixture. Mutate a private copy of the production
# predicate to the old connectivity-only rule, prove the exact regression is red,
# then run the unchanged candidate through the same test seam.
root_dir="$(git rev-parse --show-toplevel)"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
source_file="$root_dir/macos/TelevyBackupApp/SnapshotMountHelperReadiness.swift"
cp "$source_file" "$fixture_dir/SnapshotMountHelperReadiness.swift"
python3 - "$fixture_dir/SnapshotMountHelperReadiness.swift" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
old = 'isReady = reachable && daemonReady && validExpected && buildCommit == expectedBuildCommit'
assert text.count(old) == 1, 'production predicate changed; update the regression mutation'
p.write_text(text.replace(old, 'isReady = reachable'))
PY
diff -u "$source_file" "$fixture_dir/SnapshotMountHelperReadiness.swift" || test "$?" -eq 1
swiftc="$(xcrun --find swiftc)"
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
"$swiftc" -sdk "$sdk_path" \
  "$fixture_dir/SnapshotMountHelperReadiness.swift" \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountHelperInstallation.swift" \
  "$root_dir/macos/TelevyBackupAppTests/SnapshotMountHelperReadinessTests.swift" \
  -o "$fixture_dir/legacy-tests"
if "$fixture_dir/legacy-tests" > "$fixture_dir/legacy-result.txt" 2>&1; then
  echo 'Connectivity-only readiness unexpectedly passed' >&2
  exit 1
fi
cat "$fixture_dir/legacy-result.txt"
grep -Fx 'FAIL: a reachable legacy helper must require an update' "$fixture_dir/legacy-result.txt"
"$swiftc" -sdk "$sdk_path" \
  "$source_file" \
  "$root_dir/macos/TelevyBackupApp/SnapshotMountHelperInstallation.swift" \
  "$root_dir/macos/TelevyBackupAppTests/SnapshotMountHelperReadinessTests.swift" \
  -o "$fixture_dir/candidate-tests"
"$fixture_dir/candidate-tests"
