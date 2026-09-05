#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

assert_contains() {
  local label="$1"
  local haystack="$2"
  local needle="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    printf 'missing %s: %s\n' "$label" "$needle" >&2
    exit 1
  fi
}

for workflow in ci-pr.yml ci-main.yml label-gate.yml package-ci.yml release-preparation.yml release-completion.yml release.yml notify-release-failure.yml; do
  ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$root_dir/.github/workflows/$workflow"
done
for workflow in release-preparation.yml release.yml; do
  text="$(<"$root_dir/.github/workflows/$workflow")"
  if [[ "$text" != *"GITHUB_TOKEN"* && "$text" != *"github.token"* ]]; then
    printf 'missing native token in %s\n' "$workflow" >&2
    exit 1
  fi
done
preparation_text="$(<"$root_dir/.github/workflows/release-preparation.yml")"
assert_contains "release preparation expectedHeadOid" "$preparation_text" "expectedHeadOid"
notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
assert_contains "notifier Release Product trigger" "$notify_text" "- Release Product"
if [[ "$notify_text" == *"Release exact-tag backfill"* ]]; then
  printf 'legacy backfill notifier trigger remains\n' >&2
  exit 1
fi
assert_contains "notifier recovery output" "$notify_text" "recovery:"
echo "release workflow contract tests passed"
