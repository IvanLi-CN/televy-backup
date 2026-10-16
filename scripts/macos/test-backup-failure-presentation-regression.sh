#!/usr/bin/env bash
set -euo pipefail

[[ "${GITHUB_ACTIONS:-}" == true && "$(uname -s)" == Darwin ]] || { echo 'GitHub-hosted macOS required' >&2; exit 1; }
root_dir="$(git rev-parse --show-toplevel)"
fixture_dir="$(mktemp -d)"
trap 'rm -rf "$fixture_dir"' EXIT
source_file="$root_dir/macos/TelevyBackupApp/BackupFailurePresentation.swift"
cp "$source_file" "$fixture_dir/BackupFailurePresentation.swift"
python3 - "$fixture_dir/BackupFailurePresentation.swift" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
old = 'case "telegram.unavailable":'
assert text.count(old) == 1, 'Telegram presentation changed; update the regression mutation'
p.write_text(text.replace(old, 'case "fixture.legacy-unmapped-telegram":'))
PY
diff -u "$source_file" "$fixture_dir/BackupFailurePresentation.swift" || test "$?" -eq 1
sources=()
for source in "$root_dir/macos/TelevyBackupApp"/*.swift; do
  [[ "$source" == "$source_file" ]] || sources+=("$source")
done
swiftc="$(xcrun --find swiftc)"
sdk_path="$(xcrun --sdk macosx --show-sdk-path)"
"$swiftc" -sdk "$sdk_path" -D TELEVYBACKUP_TESTING -framework SwiftUI -framework AppKit \
  "${sources[@]}" "$fixture_dir/BackupFailurePresentation.swift" \
  "$root_dir/macos/TelevyBackupAppTests/BackupFailureHistoryTests.swift" \
  -o "$fixture_dir/legacy-tests"
if "$fixture_dir/legacy-tests" > "$fixture_dir/legacy-result.txt" 2>&1; then
  echo 'Unmapped Telegram failure unexpectedly passed' >&2
  exit 1
fi
cat "$fixture_dir/legacy-result.txt"
grep -F 'FAIL: known service failure must not become The run failed: telegram.unavailable' "$fixture_dir/legacy-result.txt"
echo 'PASS: generic Telegram failure regression is red; unmodified candidate runs in Swift unit tests'
