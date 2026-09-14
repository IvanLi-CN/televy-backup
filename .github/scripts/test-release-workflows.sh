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
assert_contains "prepared-head gate dispatch permission" "$preparation_text" "actions: write"
assert_contains "prepared-head label gate dispatch" "$preparation_text" "gh workflow run label-gate.yml"
assert_contains "prepared-head completion dispatch" "$preparation_text" "gh workflow run release-completion.yml"
label_gate_text="$(<"$root_dir/.github/workflows/label-gate.yml")"
assert_contains "release intent label gate job" "$label_gate_text" "name: Release intent label gate"
notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
assert_contains "notifier Release Product trigger" "$notify_text" "- Release Product"
if [[ "$notify_text" == *"Release exact-tag backfill"* ]]; then
  printf 'legacy backfill notifier trigger remains\n' >&2
  exit 1
fi
assert_contains "notifier recovery candidate output" "$notify_text" "recovery_candidate:"
assert_contains "notifier superseded handling" "$notify_text" "not-applicable: superseded_by_product_tag"
release_text="$(<"$root_dir/.github/workflows/release.yml")"
assert_contains "release full history checkout" "$release_text" "fetch-depth: 0"
assert_contains "release full tag fetch" "$release_text" "git fetch --force origin main '+refs/tags/*:refs/tags/*'"
assert_contains "release sequence gate" "$release_text" "verify-release-sequence"
assert_contains "release reservation verification" "$release_text" "verify_github_reservation"
assert_contains "recovery existing bound verification" "$release_text" "verify_github_receipt"
assert_contains "release bound receipt" "$release_text" "--state bound"
assert_contains "release consumed receipt" "$release_text" "--state consumed"
assert_contains "release intent artifact" "$release_text" "name: release-intent"
assert_contains "release intent covered merge" "$release_text" "covered_merge_sha"
assert_contains "release intent type" "$release_text" "RELEASE_TYPE"
assert_contains "release publish recheck" "$release_text" "Create or verify immutable product tag"
assert_contains "release state fail closed" "$release_text" "unable to resolve GitHub Release state"
if [[ "$release_text" == *'Product release became published for "${PRODUCT_TAG}"; no asset overwrite'*$'\n'*'exit 0'* ]]; then
  printf 'published release path exits before consumed receipt\n' >&2
  exit 1
fi
assert_contains "draft release publish" "$release_text" "gh release edit \"\${PRODUCT_TAG}\" --draft=false"
assert_contains "draft-only asset overwrite" "$release_text" '[[ "${runtime_state}" == draft ]]'
if (( $(printf '%s' "$release_text" | grep -Fc 'verify-release-sequence') < 2 )); then
  printf 'release workflow must verify sequence before and during publication\n' >&2
  exit 1
fi
if [[ "$release_text" == *"gh release upload \"\${PRODUCT_TAG}\" release-assets/*"* || "$release_text" == *"gh release create \"\${PRODUCT_TAG}\" release-assets/*"* ]]; then
  printf 'release workflow must not pass app bundle directories to gh release\n' >&2
  exit 1
fi
assert_contains "release regular-file collection" "$release_text" "find release-assets -maxdepth 1 -type f"
assert_not_contains "source PR release comment step" "$release_text" "Upsert PR release version comment"
assert_not_contains "source PR release comment marker" "$release_text" "televybackup-release-version-comment"
assert_not_contains "source PR lookup" "$release_text" "/commits/\${TARGET_INPUT}/pulls"
assert_not_contains "source PR number output" "$release_text" "pr_number"
assert_contains "prerelease release behavior" "$release_text" "--prerelease --latest=false"
completion_text="$(<"$root_dir/.github/workflows/release-completion.yml")"
assert_contains "completion native signature gate" "$completion_text" "--require-github-verification"
assert_contains "completion reservation provenance gate" "$completion_text" "--reservation-json"
assert_contains "completion GitHub verification fetch" "$completion_text" 'gh api "repos/${GITHUB_REPOSITORY}/commits/${HEAD_SHA}"'
assert_contains "completion GitHub verification SHA gate" "$completion_text" 'jq -e --arg commit "${HEAD_SHA}"'
assert_contains "completion GitHub verification status gate" "$completion_text" '.commit.verification.verified == true'
assert_contains "completion GitHub verification compatibility probe" "$completion_text" 'release_completion.py --help'
assert_contains "completion GitHub verification argument" "$completion_text" "--github-verification-json"
assert_contains "completion product-only verification selector" "$completion_text" 'product_release='
assert_contains "completion job timeout covers native CI" "$completion_text" "timeout-minutes: 35"
assert_contains "completion source-check wait budget" "$completion_text" 'deadline=$((SECONDS + 1800))'
assert_contains "completion current PR head recheck" "$completion_text" 'current_head="$(gh api "repos/${GITHUB_REPOSITORY}/pulls/${PR_NUMBER}" --jq '\''.head.sha'\'')"'
assert_contains "completion current PR head match" "$completion_text" 'test "${current_head}" = "${HEAD_SHA}"'
assert_contains "completion workflow dispatch input" "$completion_text" "pr_number:"
assert_contains "completion dispatch PR resolution" "$completion_text" 'pulls/${pr_number}'
assert_contains "completion trusted dispatch ref" "$completion_text" 'refs/heads/main'
preparation_text="$(<"$root_dir/.github/workflows/release-preparation.yml")"
assert_contains "prepared-head gates use trusted main" "$preparation_text" "--ref main"
assert_not_contains "prepared-head gates use mutable PR workflow" "$preparation_text" '--ref "${HEAD_REF}"'
assert_contains "existing preparation verification source" "$preparation_text" 'SOURCE_SHA: ${{ steps.prepare.outputs.source_sha }}'
ruby -ryaml - "$root_dir/.github/workflows/release.yml" "$root_dir/.github/workflows/notify-release-failure.yml" <<'RUBY'
release = YAML.load_file(ARGV.fetch(0))
expected_permissions = {"contents" => "write"}
abort "release workflow permissions are broader than contents: write" unless release.fetch("permissions") == expected_permissions
abort "publish job permissions are broader than contents: write" unless release.fetch("jobs").fetch("publish").fetch("permissions") == expected_permissions

notify = YAML.load_file(ARGV.fetch(1))
workflow_run = notify.fetch(true).fetch("workflow_run")
abort "failure notifier must watch Release Product" unless workflow_run.fetch("workflows") == ["Release Product"]
abort "failure notifier must run on completed main workflow runs" unless workflow_run.fetch("types") == ["completed"] && workflow_run.fetch("branches") == ["main"]
expected_failure = "${{ github.event_name == 'workflow_run' && github.event.workflow_run.conclusion == 'failure' }}"
%w[resolve_release_context notify_failure].each do |job_name|
  abort "#{job_name} must keep the release failure condition" unless notify.fetch("jobs").fetch(job_name).fetch("if") == expected_failure
end
notifier = notify.fetch("jobs").fetch("notify_failure")
abort "failure notifier must call the pinned Oidrune workflow" unless notifier.fetch("uses") == "IvanLi-CN/oidrune/.github/workflows/notify.yml@e48822f99c6402a753ed86557ea029754cbab20b"
summary = notifier.fetch("with").fetch("summary")
%w[target_sha recovery_candidate].each do |field|
  abort "failure notification summary must include #{field}" unless summary.include?("#{field}:")
end
RUBY
quality_gates_text="$(<"$root_dir/docs/quality-gates.md")"
assert_contains "release owner confirmation boundary" "$quality_gates_text" "Successful publication is reported directly to the owner by the release-owning agent; Release Product does not write a result comment to the source PR."
release_spec_text="$(<"$root_dir/docs/specs/product-version-release-chain/SPEC.md")"
assert_contains "release spec owner confirmation boundary" "$release_spec_text" "The release-owning agent MUST report successful publication directly to the owner, and Release Product MUST NOT create or update a result comment on the source PR."
echo "release workflow contract tests passed"
