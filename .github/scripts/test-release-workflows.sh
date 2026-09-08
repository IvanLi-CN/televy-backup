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

assert_not_contains() {
  local label="$1"
  local haystack="$2"
  local needle="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    printf 'unexpected %s: %s\n' "$label" "$needle" >&2
    exit 1
  fi
}

for workflow in ci-pr.yml ci-main.yml label-gate.yml release-intent-label-gate.yml package-ci.yml release-preparation.yml release-completion.yml release.yml notify-release-failure.yml; do
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
label_gate_text="$(<"$root_dir/.github/workflows/release-intent-label-gate.yml")"
assert_contains "release intent label gate job" "$label_gate_text" "name: Release intent label gate"
notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
assert_contains "notifier Release Product trigger" "$notify_text" "- Release Product"
if [[ "$notify_text" == *"Release exact-tag backfill"* ]]; then
  printf 'legacy backfill notifier trigger remains\n' >&2
  exit 1
fi
assert_contains "notifier recovery output" "$notify_text" "recovery:"
release_text="$(<"$root_dir/.github/workflows/release.yml")"
if [[ "$release_text" == *"gh release upload \"\${PRODUCT_TAG}\" release-assets/*"* || "$release_text" == *"gh release create \"\${PRODUCT_TAG}\" release-assets/*"* ]]; then
  printf 'release workflow must not pass app bundle directories to gh release\n' >&2
  exit 1
fi
assert_contains "release regular-file collection" "$release_text" "find release-assets -maxdepth 1 -type f"
assert_not_contains "source PR release comment step" "$release_text" "Upsert PR release version comment"
assert_not_contains "source PR release comment marker" "$release_text" "televybackup-release-version-comment"
assert_not_contains "source PR release comment API" "$release_text" "gh api"
assert_not_contains "source PR lookup" "$release_text" "/commits/\${TARGET_INPUT}/pulls"
assert_not_contains "source PR number output" "$release_text" "pr_number"
assert_not_contains "issues write permission" "$release_text" "issues: write"
assert_not_contains "pull requests read permission" "$release_text" "pull-requests: read"
assert_not_contains "pull requests write permission" "$release_text" "pull-requests: write"
quality_gates_text="$(<"$root_dir/docs/quality-gates.md")"
assert_contains "release owner confirmation boundary" "$quality_gates_text" "Successful publication is reported directly to the owner by the release-owning agent; Release Product does not write a result comment to the source PR."
release_spec_text="$(<"$root_dir/docs/specs/product-version-release-chain/SPEC.md")"
assert_contains "release spec owner confirmation boundary" "$release_spec_text" "The release-owning agent MUST report successful publication directly to the owner, and Release Product MUST NOT create or update a result comment on the source PR."
echo "release workflow contract tests passed"
