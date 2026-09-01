#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

assert_contains() {
  local haystack="$1"
  local needle="$2"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "Expected output to contain: $needle" >&2
    echo "Actual output:" >&2
    printf '%s\n' "$haystack" >&2
    exit 1
  fi
}

assert_not_contains() {
  local haystack="$1"
  local needle="$2"
  if [[ "$haystack" == *"$needle"* ]]; then
    echo "Expected output not to contain: $needle" >&2
    exit 1
  fi
}

assert_count() {
  local file="$1"
  local needle="$2"
  local expected="$3"
  local actual
  actual="$(awk -v needle="$needle" 'index($0, needle) { count++ } END { print count + 0 }' "$file")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Expected $expected occurrences of '$needle' in $file, got $actual" >&2
    exit 1
  fi
}

setup_git_repo() {
  local repo_dir="$1"
  mkdir -p "$repo_dir/crates/daemon"
  cat > "$repo_dir/crates/daemon/Cargo.toml" <<'TOML'
[package]
name = "televybackupd"
version = "0.1.0"
TOML

  git -C "$repo_dir" init -q
  git -C "$repo_dir" config user.name test
  git -C "$repo_dir" config user.email test@example.com
  touch "$repo_dir/.gitkeep"
  git -C "$repo_dir" add .
  git -C "$repo_dir" commit -qm 'test commit'
}

run_compute_version_tests() {
  local repo_dir="$tmp_dir/compute-version"
  setup_git_repo "$repo_dir"

  git -C "$repo_dir" tag v0.1.9

  local out
  out="$(
    cd "$repo_dir"
    BUMP_LEVEL=patch GITHUB_ENV=/dev/stdout "$root_dir/.github/scripts/compute-version.sh"
  )"
  assert_contains "$out" 'APP_EFFECTIVE_VERSION=0.1.10'

  out="$(
    cd "$repo_dir"
    BUMP_LEVEL=minor GITHUB_ENV=/dev/stdout "$root_dir/.github/scripts/compute-version.sh"
  )"
  assert_contains "$out" 'APP_EFFECTIVE_VERSION=0.2.0'

  out="$(
    cd "$repo_dir"
    BUMP_LEVEL=major GITHUB_ENV=/dev/stdout "$root_dir/.github/scripts/compute-version.sh"
  )"
  assert_contains "$out" 'APP_EFFECTIVE_VERSION=1.0.0'
}

run_label_gate_tests() {
  local out
  out="$(LABELS_JSON='[{"name":"type:minor"},{"name":"channel:rc"}]' "$root_dir/.github/scripts/label-gate.sh")"
  assert_contains "$out" 'Intent label OK: type:minor'
  assert_contains "$out" 'release_channel=rc'
  assert_contains "$out" 'bump_level=minor'

  if LABELS_JSON='[{"name":"type:minor"}]' "$root_dir/.github/scripts/label-gate.sh" >/dev/null 2>&1; then
    echo 'label-gate should fail when channel label is missing' >&2
    exit 1
  fi
}


run_freeze_release_intent_tests() {
  local out_file="$tmp_dir/release-intent.json"
  PULLS_JSON='[{"number":54,"html_url":"https://github.com/IvanLi-CN/televy-backup/pull/54"}]'   LABELS_JSON='[{"name":"type:minor"},{"name":"channel:rc"}]'   WORKFLOW_RUN_SHA='cafebabe'   bash "$root_dir/.github/scripts/freeze-release-intent.sh" "$out_file"

  python3 - <<'PY_INNER' "$out_file"
from __future__ import annotations

import json
import sys
from pathlib import Path

payload = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert payload["should_release"] == "true", payload
assert payload["bump_level"] == "minor", payload
assert payload["release_channel"] == "rc", payload
assert payload["pr_number"] == "54", payload
PY_INNER
}

run_release_intent_tests() {
  local out
  out="$(
    PULLS_JSON='[{"number":52,"html_url":"https://github.com/IvanLi-CN/televy-backup/pull/52"}]' \
    LABELS_JSON='[{"name":"type:patch"},{"name":"channel:stable"}]' \
    WORKFLOW_RUN_SHA='deadbeef' \
    "$root_dir/.github/scripts/release-intent.sh"
  )"
  assert_contains "$out" 'should_release=true'
  assert_contains "$out" 'bump_level=patch'
  assert_contains "$out" 'release_channel=stable'

  out="$(
    PULLS_JSON='[{"number":53,"html_url":"https://github.com/IvanLi-CN/televy-backup/pull/53"}]' \
    LABELS_JSON='[{"name":"type:docs"},{"name":"channel:stable"}]' \
    WORKFLOW_RUN_SHA='deadbeef' \
    "$root_dir/.github/scripts/release-intent.sh"
  )"
  assert_contains "$out" 'should_release=false'
  assert_contains "$out" 'reason=intent_skip'

  out="$(
    PULLS_JSON='[]' \
    WORKFLOW_RUN_SHA='deadbeef' \
    "$root_dir/.github/scripts/release-intent.sh"
  )"
  assert_contains "$out" 'should_release=false'
  assert_contains "$out" 'reason=ambiguous_or_missing_pr(count=0)'
}

run_notify_workflow_contract_tests() {
  local workflow_file="$root_dir/.github/workflows/notify-release-failure.yml"
  local workflow_text
  workflow_text="$(<"$workflow_file")"

  assert_count "$workflow_file" 'IvanLi-CN/oidrune/.github/workflows/notify.yml@e48822f99c6402a753ed86557ea029754cbab20b' 2
  assert_count "$workflow_file" 'id-token: write' 2
  assert_count "$workflow_file" 'summary: |' 2
  assert_count "$workflow_file" 'outcome: failure' 2
  assert_not_contains "$workflow_text" 'IvanLi-CN/github-workflows/.github/workflows/release-failure-telegram.yml@main'
  assert_not_contains "$workflow_text" 'SHOUTRRR_URL'
  assert_not_contains "$workflow_text" 'alert_kind:'
  assert_not_contains "$workflow_text" 'gateway_url:'
  assert_not_contains "$workflow_text" 'oidc_audience:'

  assert_contains "$workflow_text" 'workflow_run:'
  assert_contains "$workflow_text" 'workflows:'
  assert_contains "$workflow_text" '- Release'
  assert_contains "$workflow_text" 'types:'
  assert_contains "$workflow_text" '- completed'
  assert_contains "$workflow_text" 'branches:'
  assert_contains "$workflow_text" 'if: ${{ github.event_name == '\''workflow_run'\'' && github.event.workflow_run.conclusion == '\''failure'\'' }}'
  assert_contains "$workflow_text" 'workflow_dispatch:'

  assert_contains "$workflow_text" '🚨 Release Failed · ${{ github.repository }}'
  assert_contains "$workflow_text" 'status: ${{ github.event.workflow_run.conclusion }}'
  assert_contains "$workflow_text" 'target_sha: ${{ needs.resolve_release_context.outputs.head_sha }}'
  assert_contains "$workflow_text" 'run_url: ${{ github.event.workflow_run.html_url }}'
  assert_contains "$workflow_text" '🧪 Smoke Test · ${{ github.repository }}'
  assert_contains "$workflow_text" 'status: smoke test'
  assert_contains "$workflow_text" 'target_sha: ${{ github.sha }}'
  assert_contains "$workflow_text" "run_url: \${{ format('{0}/{1}/actions/runs/{2}', github.server_url, github.repository, github.run_id) }}"
}

run_compute_version_tests
run_label_gate_tests
run_freeze_release_intent_tests
run_release_intent_tests
run_notify_workflow_contract_tests

echo 'release script and notification workflow contract tests passed'
