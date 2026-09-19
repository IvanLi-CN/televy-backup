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

for workflow in ci-pr.yml ci-main.yml label-gate.yml package-ci.yml release-preparation.yml release-completion.yml release.yml notify-release-failure.yml homebrew-cask.yml homebrew-cask-update.yml; do
  ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$root_dir/.github/workflows/$workflow"
done
python3 "$root_dir/.agents/skills/quality-gates/assets/scripts/check_quality_gates.py" \
  --repo-root "$root_dir" \
  --declaration "$root_dir/.github/quality-gates.json" \
  --allow-unchecked-branch-protection >/dev/null
for workflow in release-preparation.yml release.yml; do
  text="$(<"$root_dir/.github/workflows/$workflow")"
  if [[ "$text" != *"GITHUB_TOKEN"* && "$text" != *"github.token"* ]]; then
    printf 'missing native token in %s\n' "$workflow" >&2
    exit 1
  fi
done
for workflow in package-ci.yml release-preparation.yml release-completion.yml release.yml; do
  if grep -nE 'uses: [^[:space:]]+@(v[0-9]+|main|master)$' "$root_dir/.github/workflows/$workflow" >/dev/null; then
    printf 'release-gate workflow contains a mutable action reference: %s\n' "$workflow" >&2
    exit 1
  fi
done
preparation_text="$(<"$root_dir/.github/workflows/release-preparation.yml")"
assert_contains "release preparation expectedHeadOid" "$preparation_text" "expectedHeadOid"
assert_contains "prepared-head gate dispatch permission" "$preparation_text" "actions: write"
assert_contains "prepared-head label gate dispatch" "$preparation_text" "gh workflow run label-gate.yml"
assert_contains "prepared-head completion dispatch" "$preparation_text" "gh workflow run release-completion.yml"
assert_contains "preparation source check readiness output" "$preparation_text" 'echo "source_checks_ready=${source_checks_ready}"'
assert_contains "preparation reserve requires ready source checks" "$preparation_text" "steps.pr.outputs.source_checks_ready == 'true'"
assert_contains "preparation reservation uses Actions token" "$preparation_text" 'GH_TOKEN: ${{ github.token }}'
assert_contains "reservation recovery fetches immutable parent" "$preparation_text" '"git", "fetch", "--no-tags", "origin", reserved_source'
assert_contains "reservation recovery binds channel intent" "$preparation_text" '"channel": release_channel.removeprefix("channel:")'
assert_not_contains "preparation App token" "$preparation_text" "actions/create-github-app-token@v1"
assert_not_contains "preparation extra credential variable" "$preparation_text" "TELEVYBACKUP_RELEASE_APP_ID"
assert_not_contains "preparation extra credential secret" "$preparation_text" "TELEVYBACKUP_RELEASE_APP_PRIVATE_KEY"
label_gate_text="$(<"$root_dir/.github/workflows/label-gate.yml")"
assert_contains "release intent label gate job" "$label_gate_text" "name: Release intent label gate"
assert_contains "workflow dispatch trusted main token" "$label_gate_text" 'GH_TOKEN: ${{ github.token }}'
assert_contains "label gate merge-group validation" "$label_gate_text" "merge-group-release-gate.sh labels"
assert_contains "label gate merge-group fetch credentials" "$label_gate_text" "persist-credentials: true"
assert_not_contains "label gate merge-group echo bridge" "$label_gate_text" "reuses the Release intent label gate"
assert_contains "label gate non-preemptive concurrency" "$label_gate_text" "cancel-in-progress: false"
assert_not_contains "label gate unsupported queue key" "$label_gate_text" "queue: max"
notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
assert_contains "notifier Release Product trigger" "$notify_text" "- Release Product"
if [[ "$notify_text" == *"Release exact-tag backfill"* ]]; then
  printf 'legacy backfill notifier trigger remains\n' >&2
  exit 1
fi
assert_contains "notifier recovery candidate output" "$notify_text" "recovery_candidate:"
assert_contains "notifier superseded handling" "$notify_text" "not-applicable: superseded_by_product_tag"
assert_contains "notifier intent artifact resolver" "$notify_text" "load_release_intent"
assert_contains "notifier fail-closed target" "$notify_text" 'target_sha: ${{ needs.resolve_release_context.outputs.merge_sha }}'
assert_contains "notifier canonical merge identity" "$notify_text" 'merge_commit_sha: ${{ needs.resolve_release_context.outputs.merge_commit_sha }}'
assert_contains "notifier immutable reservation validation" "$notify_text" "verify_reservation"
assert_contains "notifier immutable bound receipt validation" "$notify_text" "verify_bound_receipt"
assert_contains "notifier optional product tag validation" "$notify_text" "api_json_optional"
assert_contains "notifier product tag status" "$notify_text" 'tag_status: ${{ needs.resolve_release_context.outputs.tag_status }}'
assert_contains "notifier artifact origin validation" "$notify_text" "GitHub API URL origin mismatch"
assert_not_contains "notifier raw resolver exception" "$notify_text" "resolver_error = f"
assert_not_contains "notifier raw log exception" "$notify_text" "logs_error = f"
completion_text="$(<"$root_dir/.github/workflows/release-completion.yml")"
assert_contains "release completion initializes prepared release state" "$completion_text" 'prepared_json=""'
assert_contains "release completion initializes preparation SHA" "$completion_text" 'preparation_sha=""'
assert_contains "completion ready-for-review trigger" "$completion_text" "ready_for_review"
assert_contains "completion merge-group validation" "$completion_text" "merge-group-release-gate.sh completion"
assert_contains "completion merge-group fetch credentials" "$completion_text" "persist-credentials: true"
assert_not_contains "completion merge-group echo bridge" "$completion_text" "reuses Release completion"
assert_contains "completion merge-group full history" "$completion_text" "fetch-depth: 0"
merge_group_checkout_line="$(grep -n 'name: Checkout trusted merge-group gate scripts' "$root_dir/.github/workflows/release-completion.yml" | cut -d: -f1)"
immutable_fetch_line="$(grep -n 'name: Fetch immutable release identity refs' "$root_dir/.github/workflows/release-completion.yml" | cut -d: -f1)"
if [[ -z "$merge_group_checkout_line" || -z "$immutable_fetch_line" || "$merge_group_checkout_line" -ge "$immutable_fetch_line" ]]; then
  printf 'merge-group completion must checkout before fetching immutable refs\n' >&2
  exit 1
fi
assert_contains "completion current PR API snapshot" "$completion_text" 'gh api "repos/${GITHUB_REPOSITORY}/pulls/${PR_NUMBER}"'
assert_contains "completion current PR head validation" "$completion_text" 'current PR head changed while Release completion was queued'
assert_contains "completion current PR labels" "$completion_text" 'jq '\''.labels'\'' "${RUNNER_TEMP}/current-pr.json"'
assert_contains "completion final PR revalidation" "$completion_text" 'validate_current_pr "${RUNNER_TEMP}/current-pr-final.json"'
assert_contains "completion final PR labels" "$completion_text" 'jq '\''.labels'\'' "${RUNNER_TEMP}/current-pr-final.json"'
final_pr_validation_line="$(grep -n 'current-pr-final.json' "$root_dir/.github/workflows/release-completion.yml" | head -1 | cut -d: -f1)"
final_checks_refresh_line="$(grep -n 'commits/${HEAD_SHA}/check-runs' "$root_dir/.github/workflows/release-completion.yml" | tail -1 | cut -d: -f1)"
if [[ -z "$final_pr_validation_line" || -z "$final_checks_refresh_line" || "$final_pr_validation_line" -ge "$final_checks_refresh_line" ]]; then
  printf 'completion must refresh checks after final PR identity validation\n' >&2
  exit 1
fi
preparation_refresh_line="$(grep -n 'prepared_json=.*find-prepared' "$root_dir/.github/workflows/release-completion.yml" | tail -1 | cut -d: -f1)"
preparation_sha_line="$(grep -n 'preparation_sha=.*preparationSha' "$root_dir/.github/workflows/release-completion.yml" | head -1 | cut -d: -f1)"
if [[ -z "$preparation_refresh_line" || -z "$preparation_sha_line" || "$preparation_refresh_line" -le "$final_checks_refresh_line" || "$preparation_refresh_line" -ge "$preparation_sha_line" ]]; then
  printf 'completion must refresh preparation after final source checks and before using its SHA\n' >&2
  exit 1
fi
assert_contains "completion missing preparation fail-closed message" "$completion_text" "product release is missing a valid VERSION preparation commit"
assert_contains "completion non-preemptive concurrency" "$completion_text" "cancel-in-progress: false"
assert_not_contains "completion unsupported queue key" "$completion_text" "queue: max"
assert_contains "completion job timeout covers native CI" "$completion_text" "timeout-minutes: 35"
assert_contains "completion source-check wait budget" "$completion_text" 'deadline=$((SECONDS + 1800))'
assert_not_contains "completion preemptive cancellation" "$completion_text" "cancel-in-progress: true"
release_text="$(<"$root_dir/.github/workflows/release.yml")"
[[ "$(grep -Fc 'timeout-minutes: 15' <<<"$release_text")" -eq 2 ]] || {
  printf 'release native package jobs must have a 15-minute timeout\n' >&2
  exit 1
}
[[ "$(grep -Fc 'uses: actions/cache@' <<<"$release_text")" -ge 2 ]] || {
  printf 'release native package jobs must cache build dependencies\n' >&2
  exit 1
}
[[ "$(grep -Fc 'uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830' <<<"$release_text")" -eq 3 ]] || {
  printf 'release dependency cache action must be pinned to a full commit SHA\n' >&2
  exit 1
}
assert_contains "release dependency cache architecture key" "$release_text" "runner.arch"
assert_contains "release publication is non-preemptive" "$release_text" "group: release-product-main"
assert_contains "release publication is serialized" "$release_text" "cancel-in-progress: false"
[[ "$(grep -Fc '${{ runner.os }}-ARM64-cargo-macos-' <<<"$release_text")" -eq 0 && "$(grep -Fc '${{ runner.os }}-X64-cargo-macos-' <<<"$release_text")" -eq 0 ]] || {
  printf 'release dependency cache must not restore host-specific artifacts across architectures\n' >&2
  exit 1
}
[[ "$(grep -Fc 'timeout-minutes: 10' <<<"$release_text")" -eq 1 ]] || {
  printf 'release Universal 2 assembly must have a 10-minute timeout\n' >&2
  exit 1
}
assert_contains "release arm64 package retry limit" "$release_text" "Build arm64 package (up to 2 attempts)"
assert_contains "release x86_64 package retry limit" "$release_text" "Build x86_64 package (up to 2 attempts)"
assert_contains "release snapshot head" "$release_text" "head_sha"
assert_contains "release snapshot labels" "$release_text" "labels_json"
assert_contains "release snapshot components" "$release_text" "components_json"
assert_contains "protected release tag owner" "$release_text" "protected-release-automation"
reservation_text="$(<"$root_dir/.github/scripts/release_reservation.py")"
assert_contains "decision receipt namespace" "$reservation_text" "release-decision"
assert_contains "release full history checkout" "$release_text" "fetch-depth: 0"
assert_contains "release full tag fetch" "$release_text" "git fetch --force origin main '+refs/tags/*:refs/tags/*'"
assert_contains "release trusted main API lookup" "$release_text" "git/ref/heads/main"
assert_contains "release immutable policy checkout" "$release_text" 'ref: ${{ steps.trusted-main.outputs.sha }}'
assert_contains "native helper identity policy SHA" "$release_text" 'git show "${POLICY_SHA}:scripts/macos/verify-component-identity.sh"'
assert_contains "native helper identity policy invocation" "$release_text" 'bash "$TELEVYBACKUP_POLICY_VERIFY_COMPONENT_IDENTITY"'
assert_contains "native helper extraction policy SHA" "$release_text" 'git show "${POLICY_SHA}:scripts/macos/extract-snapshot-access-helper.sh"'
assert_contains "native helper extraction policy invocation" "$release_text" 'bash "${policy_extract_snapshot_access_helper}"'
assert_contains "release PR merge association" "$release_text" "merge_commit_sha // empty"
assert_contains "release PR preparation association" "$release_text" 'pull_request_head_sha}" = "${preparation_sha}'
assert_contains "release sequence gate" "$release_text" "verify-release-sequence"
assert_contains "helper tag provenance gate" "$release_text" "verify-tag-provenance --tag \"\${candidate}\""
assert_contains "RC tag provenance gate" "$release_text" "verify-tag-provenance --tag \"\${rc_tag}\""
assert_contains "release reservation verification" "$release_text" "verify_github_reservation"
assert_contains "recovery existing bound verification" "$release_text" "verify_github_receipt"
assert_contains "release bound receipt" "$release_text" "--state bound"
assert_contains "recovery missing bound repair" "$release_text" "RELEASE_RECOVERY_BOUND_REPAIR"
assert_contains "recovery repair uses same merge SHA" "$release_text" '--merge-sha "${merge_sha}"'
assert_contains "bound state after append" "$release_text" "bound_state=present"
assert_contains "release consumed receipt" "$release_text" "--state consumed"
assert_contains "release intent artifact" "$release_text" "name: release-intent"
assert_contains "release intent covered merge" "$release_text" "covered_merge_sha"
assert_contains "release intent type" "$release_text" "RELEASE_TYPE"
assert_contains "release intent tag target" "$release_text" "tag_target_sha"
assert_contains "release bound identity recovery" "$release_text" "bound_identity"
assert_contains "release recovery skips successor sequence" "$release_text" 'if [[ "${BOUND_IDENTITY}" != present ]]'
assert_contains "release annotated tag object" "$release_text" "git/tags"
assert_contains "release helper extraction cleanup" "$release_text" "extract-snapshot-access-helper.sh"
assert_contains "release channel flags" "$release_text" "/releases/tags/\${tag}"
assert_contains "release latest tag lookup" "$release_text" "/releases/latest"
assert_not_contains "unsupported gh release latest field" "$release_text" "isLatest"
assert_contains "release intent verified provenance" "$release_text" "provenance_verified:true"
assert_contains "release publish recheck" "$release_text" "Create or verify immutable product tag"
assert_contains "release identity writes use Actions token" "$release_text" '--token "${GH_TOKEN}"'
assert_contains "release consumed receipt uses Actions token" "$release_text" 'GH_TOKEN: ${{ github.token }}'
assert_not_contains "release App token" "$release_text" "actions/create-github-app-token@v1"
assert_not_contains "release extra credential variable" "$release_text" "TELEVYBACKUP_RELEASE_APP_ID"
assert_not_contains "release extra credential secret" "$release_text" "TELEVYBACKUP_RELEASE_APP_PRIVATE_KEY"
assert_not_contains "release alternate ref token" "$release_text" "RELEASE_REF_TOKEN"
assert_contains "release state fail closed" "$release_text" "unable to resolve GitHub Release state"
if [[ "$release_text" == *'Product release became published for "${PRODUCT_TAG}"; no asset overwrite'*$'\n'*'exit 0'* ]]; then
  printf 'published release path exits before consumed receipt\n' >&2
  exit 1
fi
assert_contains "draft release publish" "$release_text" "gh release edit \"\${PRODUCT_TAG}\" --draft=false"
assert_contains "draft-only asset overwrite" "$release_text" '[[ "${runtime_state}" == draft ]]'
assert_not_contains "draft publication never overwrites assets" "$release_text" 'gh release upload "${PRODUCT_TAG}" "${release_files[@]}" --clobber'
assert_contains "draft publication rechecks state before upload" "$release_text" 'unable to recheck draft Release state'
assert_contains "draft publication rechecks state after upload" "$release_text" 'Release became published during asset upload'
assert_contains "draft publication reuses matching assets" "$release_text" 'reusing matching draft Release asset'
assert_contains "draft publication checks existing asset digest" "$release_text" 'existing asset digest mismatch'
assert_contains "draft publication rechecks state before each asset" "$release_text" 'Release became published before inspecting'
assert_contains "draft publication documents non-atomic upload guard" "$release_text" 'no conditional draft precondition'
assert_contains "draft publication resolves release id" "$release_text" 'find_draft_release'
assert_contains "draft publication uses release id upload" "$release_text" 'uploads.github.com/repos/${GITHUB_REPOSITORY}/releases/${release_id}/assets?name=${asset_name}'
assert_contains "draft publication rechecks release id" "$release_text" 'repos/${GITHUB_REPOSITORY}/releases/${release_id}'
assert_contains "draft publication verifies final asset set" "$release_text" 'final Release asset set or digest does not match release-assets'
assert_contains "new release starts as draft" "$release_text" 'flags=(--draft --verify-tag --title "${PRODUCT_TAG}" --generate-notes)'
assert_contains "new release is published after verification" "$release_text" 'runtime_state=draft'
assert_not_contains "draft publication uses one bulk upload" "$release_text" 'gh release upload "${PRODUCT_TAG}" "${release_files[@]}"'
if (( $(printf '%s' "$release_text" | grep -Fc 'verify-release-sequence') < 2 )); then
  printf 'release workflow must verify sequence before and during publication\n' >&2
  exit 1
fi
if [[ "$release_text" == *"gh release upload \"\${PRODUCT_TAG}\" release-assets/*"* || "$release_text" == *"gh release create \"\${PRODUCT_TAG}\" release-assets/*"* ]]; then
  printf 'release workflow must not pass app bundle directories to gh release\n' >&2
  exit 1
fi
assert_contains "release manifest-bound asset collection" "$release_text" "release-assets set does not match BUILD-MANIFEST.json"
assert_contains "release regular-file collection" "$release_text" "stat.S_ISREG(os.lstat(path).st_mode)"
assert_contains "release app bundle excluded from upload" "$release_text" 'rm -rf "$GITHUB_WORKSPACE/dist/final/TelevyBackup.app"'
assert_contains "release DMG evidence upload" "$release_text" "name: release-dmg-verification"
assert_contains "release DMG evidence binding" "$release_text" 'DMG_EVIDENCE_FILE="${RUNNER_TEMP}/dmg-release-events.jsonl"'
assert_not_contains "release acceptance screenshot reupload" "$release_text" 'gh release upload "${rc2_tag}" "${screenshot_dir}/${screenshot_name}"'
assert_contains "release product icon verifier" "$release_text" 'policy_verify_app_icon_assets="${policy_checkout}/scripts/macos/verify-app-icon-assets.sh"'
assert_contains "release RC layout verifier" "$release_text" 'bash scripts/macos/verify-dmg-layout.sh'
assert_contains "release complete requirement verifier" "$release_text" 'verify-macos-rc-acceptance.py'
package_text="$(<"$root_dir/.github/workflows/package-ci.yml")"
assert_contains "package matrix checks permission" "$package_text" "checks: read"
assert_contains "prepared package source identity" "$package_text" 'verification_sha=${verification_sha}'
assert_contains "prepared arm64 package evidence" "$package_text" 'commits/${SOURCE_SHA}/check-runs?filter=latest&per_page=100'
assert_contains "prepared package gate preserves verify" "$package_text" 'verify-prepared --commit HEAD'
ruby -ryaml - "$root_dir/.github/workflows/release.yml" <<'RUBY'
workflow = YAML.load_file(ARGV.fetch(0))
checkout = workflow.fetch("jobs").fetch("macos-acceptance").fetch("steps").find { |step| step["uses"] == "actions/checkout@11d5960a326750d5838078e36cf38b85af677262" }
abort "macOS acceptance must use the trusted policy checkout" unless checkout&.fetch("with", {}).fetch("ref", nil) == '${{ needs.resolve.outputs.policy_sha }}'
RUBY
ruby -ryaml - "$root_dir/.github/workflows/release.yml" <<'RUBY'
workflow = YAML.load_file(ARGV.fetch(0))
permissions = workflow.fetch("jobs").fetch("macos-acceptance").fetch("permissions")
abort "macOS acceptance must be able to upload Finder screenshots" unless permissions == {"contents" => "write"}
RUBY
assert_not_contains "source PR release comment step" "$release_text" "Upsert PR release version comment"
assert_not_contains "source PR release comment marker" "$release_text" "televybackup-release-version-comment"
assert_not_contains "source PR lookup" "$release_text" "/commits/\${TARGET_INPUT}/pulls"
assert_not_contains "source PR number output" "$release_text" "pr_number"
assert_contains "prerelease release behavior" "$release_text" "--prerelease --latest=false"
assert_contains "label merge-group immutable checkout" "$label_gate_text" "github.event.merge_group.base_sha || github.sha"
merge_group_text="$(<"$root_dir/.github/scripts/merge-group-release-gate.sh")"
assert_not_contains "merge-group pull request API suppression" "$merge_group_text" "|| true"
assert_contains "merge-group covered merge proof" "$merge_group_text" "merge-group-covered-merge-pulls"
assert_contains "merge-group proof argument" "$merge_group_text" "--covered-merge-proof-json"
assert_contains "merge-group failed check state" "$merge_group_text" 'failed) echo "merge-group gate: required check failed'
poll_line="$(grep -n 'required=("Release intent label gate"' "$root_dir/.github/scripts/merge-group-release-gate.sh" | cut -d: -f1)"
final_pr_line="$(grep -n 'merge-group-final-pr' "$root_dir/.github/scripts/merge-group-release-gate.sh" | cut -d: -f1)"
final_checks_line="$(grep -n 'commits/\${head_sha}/check-runs' "$root_dir/.github/scripts/merge-group-release-gate.sh" | tail -1 | cut -d: -f1)"
completion_line="$(grep -n 'python3 .github/scripts/release_completion.py' "$root_dir/.github/scripts/merge-group-release-gate.sh" | cut -d: -f1)"
assert_contains "merge-group final PR identity check" "$merge_group_text" 'test "$(jq -r '\''.head.sha'\'' "${final_pr_json}")" = "${pr_head_sha}"'
assert_contains "merge-group final labels snapshot" "$merge_group_text" 'labels_json="$(jq -c '\''.labels'\'' "${final_pr_json}")"'
assert_contains "merge-group source-check wait budget" "$merge_group_text" 'deadline=$((SECONDS + 1800))'
assert_contains "merge-group waits for label gate" "$merge_group_text" 'required=("Release intent label gate"'
assert_contains "merge-group verification fetch" "$merge_group_text" 'gh api "repos/${repository}/commits/${preparation_sha}"'
assert_contains "merge-group verification argument" "$merge_group_text" "--github-verification-json"
assert_contains "merge-group reservation source" "$merge_group_text" 'reservationSourceSha // .sourceSha'
assert_contains "merge-group checks bind to merge head" "$merge_group_text" 'gh api "repos/${repository}/commits/${head_sha}/check-runs?filter=latest&per_page=100"'
if [[ -z "$poll_line" || -z "$final_pr_line" || -z "$final_checks_line" || -z "$completion_line" || "$poll_line" -ge "$final_pr_line" || "$final_pr_line" -ge "$final_checks_line" || "$final_checks_line" -ge "$completion_line" ]]; then
  printf 'merge-group completion must wait for required checks before validation\n' >&2
  exit 1
fi
completion_text="$(<"$root_dir/.github/workflows/release-completion.yml")"
assert_contains "completion native signature gate" "$completion_text" "--require-github-verification"
assert_contains "completion reservation provenance gate" "$completion_text" "--reservation-json"
assert_contains "completion GitHub verification fetch" "$completion_text" 'gh api "repos/${GITHUB_REPOSITORY}/commits/${preparation_sha}"'
assert_contains "completion GitHub verification SHA gate" "$completion_text" 'jq -e --arg commit "${preparation_sha}"'
assert_contains "completion GitHub verification status gate" "$completion_text" '.commit.verification.verified == true'
assert_contains "completion GitHub verification compatibility probe" "$completion_text" 'release_completion.py --help'
assert_contains "completion trusted module path" "$completion_text" 'sys.path.insert(0, ".github/scripts")'
assert_contains "completion GitHub verification argument" "$completion_text" "--github-verification-json"
assert_contains "completion product-only verification selector" "$completion_text" 'product_release='
assert_contains "completion job timeout covers native CI" "$completion_text" "timeout-minutes: 35"
assert_contains "completion source-check wait budget" "$completion_text" 'deadline=$((SECONDS + 1800))'
assert_contains "completion checks bind to current head" "$completion_text" 'commits/${HEAD_SHA}/check-runs'
assert_not_contains "completion checks bind to source SHA" "$completion_text" 'verification_sha="$(printf'
assert_contains "completion immutable identity refs" "$completion_text" "git fetch --force --tags origin"
assert_contains "completion covered PR association" "$completion_text" 'commits/${covered_merge_sha}/pulls'
assert_contains "completion covered merge proof argument" "$completion_text" "--covered-merge-proof-json"
assert_not_contains "completion prepared-head direct verification" "$completion_text" 'verify-prepared --commit "${HEAD_SHA}"'
assert_not_contains "completion prepared-head identity binding" "$completion_text" 'preparation_sha="${HEAD_SHA}"'
assert_contains "completion source-head preparation lookup" "$completion_text" 'find-prepared --commit "${HEAD_SHA}" --base "${BASE_SHA}"'
assert_contains "completion reservation source trailer" "$completion_text" 'Release-Reservation-Source-SHA:'
assert_contains "completion reservation source fallback" "$completion_text" '.reservationSourceSha // .sourceSha'
assert_contains "completion reservation source tree" "$completion_text" 'sourceTreeSha'
preparation_text="$(<"$root_dir/.github/workflows/release-preparation.yml")"
assert_contains "preparation reservation source tree" "$preparation_text" 'sourceTreeSha'
assert_contains "completion workflow dispatch input" "$completion_text" "pr_number:"
assert_contains "completion dispatch PR resolution" "$completion_text" 'pulls/${pr_number}'
assert_contains "completion trusted dispatch ref" "$completion_text" 'refs/heads/${EXPECTED_HEAD_REF}'
assert_contains "completion dispatch trusted checkout" "$completion_text" 'git rev-parse refs/remotes/origin/main'
assert_contains "completion dispatch head SHA validation" "$completion_text" 'test "${GITHUB_SHA}" = "${EXPECTED_HEAD_SHA}"'
preparation_text="$(<"$root_dir/.github/workflows/release-preparation.yml")"
assert_contains "preparation trusted main resolver" "$preparation_text" 'main_sha="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq '\''.object.sha'\'')"'
assert_contains "preparation immutable policy checkout" "$preparation_text" 'ref: ${{ steps.trusted-main.outputs.sha }}'
assert_contains "prepared-head gates use prepared ref" "$preparation_text" '--ref "${HEAD_REF}"'
assert_not_contains "prepared-head gates dispatch to main" "$preparation_text" "--ref main"
assert_contains "existing preparation verification source" "$preparation_text" 'SOURCE_SHA: ${{ steps.prepare.outputs.source_sha }}'
assert_contains "preparation dispatch head SHA validation" "$preparation_text" 'test "${GITHUB_SHA}" = "${EXPECTED_HEAD_SHA}"'
assert_contains "label dispatch trusted main resolver" "$label_gate_text" 'main_sha="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq '\''.object.sha'\'')"'
assert_contains "label dispatch head verification" "$label_gate_text" '"${GITHUB_SHA}"'
ruby -ryaml - "$root_dir/.github/workflows/release.yml" "$root_dir/.github/workflows/notify-release-failure.yml" <<'RUBY'
release = YAML.load_file(ARGV.fetch(0))
abort "release workflow default permissions are not read-only" unless release.fetch("permissions") == {"contents" => "read", "pull-requests" => "read"}
resolve_permissions = release.fetch("jobs").fetch("resolve").fetch("permissions")
abort "release resolver must be able to read PR metadata" unless resolve_permissions == {"contents" => "write", "pull-requests" => "read"}
expected_permissions = {"contents" => "write"}
abort "publish job permissions are broader than contents: write" unless release.fetch("jobs").fetch("publish").fetch("permissions") == expected_permissions
publish_checkout = release.fetch("jobs").fetch("publish").fetch("steps").find { |step| step["uses"] == "actions/checkout@11d5960a326750d5838078e36cf38b85af677262" }
abort "publish job must use the trusted policy checkout" unless publish_checkout&.fetch("with", {}).fetch("ref", nil) == '${{ needs.resolve.outputs.policy_sha }}'
abort "publish checkout must not persist repository credentials" unless publish_checkout.fetch("with", {}).fetch("persist-credentials", nil) == false

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
bash "$root_dir/.github/scripts/test-release-workflow-execution.sh"
