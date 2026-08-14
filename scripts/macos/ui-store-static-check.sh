#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
ui_dir="$root_dir/macos/TelevyBackupApp"

search_swift() {
  local pattern="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n "$pattern" "$ui_dir" --glob '*.swift'
  else
    /usr/bin/grep -REn --include='*.swift' "$pattern" "$ui_dir"
  fi
}

if search_swift '@EnvironmentObject.*AppModel|environmentObject\((self|model|ModelStore\.shared)\)'; then
  echo "ERROR: root AppModel must not be injected or observed by SwiftUI" >&2
  exit 1
fi

if /usr/bin/grep -En 'final class AppModel: ObservableObject|@Published' "$ui_dir/TelevyBackupApp.swift"; then
  echo "ERROR: AppModel must remain a non-observable runtime coordinator" >&2
  exit 1
fi

for store in StatusStore RunHistoryStore SettingsStore TaskPresentationStore; do
  search_swift "final class $store: ObservableObject" >/dev/null || {
    echo "ERROR: missing domain store: $store" >&2
    exit 1
  }
done

echo "OK: UI store isolation"
