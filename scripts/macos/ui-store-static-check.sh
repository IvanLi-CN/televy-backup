#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
ui_dir="$root_dir/macos/TelevyBackupApp"

if rg -n '@EnvironmentObject[^\n]*AppModel|environmentObject\((self|model|ModelStore\.shared)\)' "$ui_dir" --glob '*.swift'; then
  echo "ERROR: root AppModel must not be injected or observed by SwiftUI" >&2
  exit 1
fi

if rg -n 'final class AppModel: ObservableObject|@Published' "$ui_dir/TelevyBackupApp.swift"; then
  echo "ERROR: AppModel must remain a non-observable runtime coordinator" >&2
  exit 1
fi

for store in StatusStore RunHistoryStore SettingsStore TaskPresentationStore; do
  rg -q "final class $store: ObservableObject" "$ui_dir" --glob '*.swift' || {
    echo "ERROR: missing domain store: $store" >&2
    exit 1
  }
done

echo "OK: UI store isolation"
