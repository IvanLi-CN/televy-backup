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

for workflow in ci-pr.yml ci-main.yml legacy-label-gate-bridge.yml release-target-gate.yml package-ci.yml release-preparation.yml release-completion.yml release.yml release-orphan-recovery.yml notify-release-failure.yml homebrew-cask.yml homebrew-cask-update.yml; do
  ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$root_dir/.github/workflows/$workflow"
done
test ! -e "$root_dir/.github/workflows/label-gate.yml"
python3 "$root_dir/.agents/skills/quality-gates/assets/scripts/check_quality_gates.py" \
  --repo-root "$root_dir" \
  --declaration "$root_dir/.github/quality-gates.json" \
  --allow-unchecked-branch-protection >/dev/null

for workflow in package-ci.yml release-target-gate.yml release-preparation.yml release-completion.yml release.yml; do
  text="$(<"$root_dir/.github/workflows/$workflow")"
  if grep -nE 'uses: [^[:space:]]+@(v[0-9]+|main|master)$' "$root_dir/.github/workflows/$workflow" >/dev/null; then
    printf 'release workflow contains a mutable action reference: %s\n' "$workflow" >&2
    exit 1
  fi
  if [[ "$text" != *"GITHUB_TOKEN"* && "$text" != *"github.token"* ]]; then
    printf 'release workflow is missing a native GitHub token: %s\n' "$workflow" >&2
    exit 1
  fi
done

preparation_text="$(<"$root_dir/.github/workflows/release-preparation.yml")"
assert_contains "manual target input" "$preparation_text" "release_target:"
assert_contains "target syntax validation" "$preparation_text" "parse_release_target"
assert_contains "target resolution" "$preparation_text" "resolve-target --release-target"
assert_contains "reservation before preparation" "$preparation_text" "Reserve target identity before preparation"
assert_contains "reservation target field" "$preparation_text" "releaseTarget:\$releaseTarget"
assert_contains "signed preparation commit API" "$preparation_text" "createCommitOnBranch"
assert_contains "signed preparation expected head" "$preparation_text" "expectedHeadOid"
assert_contains "trusted main checkout" "$preparation_text" 'ref: ${{ steps.trusted-main.outputs.sha }}'
assert_contains "preparation base lock" "$preparation_text" "PR base"
assert_contains "preparation reservation race check" "$preparation_text" "current-pr-before-reservation.json"
assert_contains "preparation version race check" "$preparation_text" "current-pr-before-version.json"
assert_contains "preparation claim recovery" "$preparation_text" "release_reservation.py find-claim"
assert_contains "preparation quoted reservation owner" "$preparation_text" "jq -r '.\"Reservation-Owner\"'"
assert_contains "preparation quoted reservation claim key" "$preparation_text" "jq -r '.\"Reservation-Claim-Key\"'"
assert_contains "preparation quoted reservation boundary token" "$preparation_text" "jq -r '.\"Reservation-Boundary-Token\"'"
assert_contains "preparation CI dispatch" "$preparation_text" "gh workflow run ci-pr.yml"
assert_contains "preparation package dispatch" "$preparation_text" "gh workflow run package-ci.yml"
assert_contains "preparation manual target gate dispatch" "$preparation_text" "gh workflow run release-target-gate.yml"
assert_contains "preparation manual completion dispatch" "$preparation_text" "gh workflow run release-completion.yml"
assert_contains "preparation dispatch timestamp" "$preparation_text" 'dispatch_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"'
assert_contains "preparation completion dispatch boundary" "$preparation_text" 'checks_since=${dispatch_started_at}'
assert_contains "preparation legacy label bridge dispatch" "$preparation_text" "gh workflow run legacy-label-gate-bridge.yml"
assert_contains "preparation dispatch head binding" "$preparation_text" 'head_sha=${prepared_head_sha}'
ci_pr_text="$(<"$root_dir/.github/workflows/ci-pr.yml")"
assert_contains "ci dispatch head input" "$ci_pr_text" "head_sha:"
assert_contains "ci dispatch exact SHA" "$ci_pr_text" 'test "${GITHUB_SHA}" = "${EXPECTED_HEAD_SHA}"'
for job in quality macos-swift; do
  job_condition="$(sed -n "/^  ${job}:/,/^    steps:/p" <<<"$ci_pr_text")"
  assert_contains "${job} partial rerun guard" "$job_condition" "always() &&"
  assert_contains "${job} cancellation guard" "$job_condition" "!cancelled() &&"
done
package_ci_text="$(<"$root_dir/.github/workflows/package-ci.yml")"
assert_contains "package dispatch head input" "$package_ci_text" "head_sha:"
assert_contains "package dispatch exact SHA" "$package_ci_text" 'test "${GITHUB_SHA}" = "${EXPECTED_HEAD_SHA}"'
assert_contains "preparation descendant head binding" "$preparation_text" "git merge-base --is-ancestor"
assert_not_contains "preparation label input" "$preparation_text" "labels-json"
assert_not_contains "preparation intent trailers" "$preparation_text" "Release-Intent"

legacy_label_bridge_text="$(<"$root_dir/.github/workflows/legacy-label-gate-bridge.yml")"
assert_contains "legacy label bridge workflow" "$legacy_label_bridge_text" "name: Label Gate"
assert_contains "legacy label bridge check" "$legacy_label_bridge_text" "name: Release intent label gate"
assert_contains "legacy label bridge explanation" "$legacy_label_bridge_text" "release labels are not release authority"
assert_contains "legacy label bridge exact head input" "$legacy_label_bridge_text" "EXPECTED_HEAD_SHA"
assert_contains "legacy label bridge exact head predicate" "$legacy_label_bridge_text" '$expected_head_sha == "" or .head.sha == $expected_head_sha'
assert_contains "legacy label bridge exact check publisher" "$legacy_label_bridge_text" "publish-pr-check.py"
assert_contains "legacy label bridge checks permission" "$legacy_label_bridge_text" "checks: write"
assert_not_contains "legacy label bridge label parsing" "$legacy_label_bridge_text" ".labels"

target_gate_text="$(<"$root_dir/.github/workflows/release-target-gate.yml")"
assert_contains "target gate workflow name" "$target_gate_text" "name: Release target gate"
assert_contains "target gate pull request trigger" "$target_gate_text" "pull_request_target:"
assert_contains "target gate trusted main resolution" "$target_gate_text" "git/ref/heads/main"
assert_contains "target gate trusted policy checkout" "$target_gate_text" 'ref: ${{ steps.trusted-main.outputs.sha }}'
assert_contains "target gate runner event path" "$target_gate_text" 'GITHUB_EVENT_PATH'
assert_not_contains "target gate context event path" "$target_gate_text" 'github.event_path'
assert_contains "target gate verifier" "$target_gate_text" "release_target_gate.py"
assert_contains "target gate non-preemptive concurrency" "$target_gate_text" "cancel-in-progress: false"
assert_contains "target gate current main check" "$target_gate_text" "current_main_sha"
assert_contains "target gate exact check publisher" "$target_gate_text" "publish-pr-check.py"
assert_contains "target gate pull request head publisher" "$target_gate_text" "github.event_name != 'merge_group'"

completion_text="$(<"$root_dir/.github/workflows/release-completion.yml")"
assert_contains "completion pull request trigger" "$completion_text" "pull_request_target:"
assert_contains "completion target gate requirement" "$completion_text" "Release target gate"
assert_contains "completion preparation lookup" "$completion_text" "find-prepared"
assert_contains "completion reservation provenance" "$completion_text" "--reservation-json"
assert_contains "completion reservation target" "$completion_text" "releaseTarget:\$releaseTarget"
assert_contains "completion reservation policy" "$completion_text" "policy:\$policy"
assert_contains "completion native signature gate" "$completion_text" "--require-github-verification"
assert_contains "completion runner event path" "$completion_text" 'GITHUB_EVENT_PATH'
assert_not_contains "completion context event path" "$completion_text" 'github.event_path'
assert_contains "completion current main check" "$completion_text" "validate_current_main"
assert_contains "completion trusted dispatch ref" "$completion_text" 'test "${GITHUB_REF}" = "refs/heads/main"'
assert_contains "completion check boundary input" "$completion_text" "checks_since:"
assert_contains "completion check boundary environment" "$completion_text" "CHECKS_SINCE"
assert_contains "completion check boundary filter" "$completion_text" "if checks_since and observed_at < checks_since:"
assert_contains "completion exact check publisher" "$completion_text" "publish-pr-check.py"
assert_contains "completion pull request head publisher" "$completion_text" "github.event_name != 'merge_group'"
assert_not_contains "completion label authority" "$completion_text" ".labels"
assert_not_contains "completion label gate workflow" "$completion_text" "label-gate.yml"
assert_not_contains "completion intent trailers" "$completion_text" "Release-Intent"

reconciliation_text="$(<"$root_dir/.github/workflows/release-reconciliation.yml")"
assert_contains "reconciliation workflow name" "$reconciliation_text" "name: Reconcile unpublished legacy release identity"
assert_contains "reconciliation dispatch trigger" "$reconciliation_text" "workflow_dispatch:"
assert_contains "reconciliation contents write" "$reconciliation_text" "contents: write"
assert_contains "reconciliation product tag absence" "$reconciliation_text" 'git/ref/tags/v${RELEASE_VERSION}'
assert_contains "reconciliation release absence" "$reconciliation_text" 'releases/tags/v${RELEASE_VERSION}'
assert_contains "reconciliation consumes absent status" "$reconciliation_text" 'gh api "$1" --include 2>/dev/null || true'
assert_contains "reconciliation consumed refs absence" "$reconciliation_text" 'release-consumed/v${RELEASE_VERSION}'
assert_contains "reconciliation artifact refs absence" "$reconciliation_text" 'release-artifact-attempts/v${RELEASE_VERSION}'
assert_contains "reconciliation run artifact check" "$reconciliation_text" 'actions/runs/${run_id}/artifacts'
assert_contains "reconciliation trusted release run" "$reconciliation_text" 'head_branch == "main"'
assert_contains "reconciliation trusted release workflow" "$reconciliation_text" 'path == ".github/workflows/release.yml"'
assert_contains "reconciliation retirement writer" "$reconciliation_text" "release_reservation.py retire"
assert_contains "reconciliation retirement namespace" "$reconciliation_text" "release-retired/v"
assert_contains "reconciliation refreshes retirement ref" "$reconciliation_text" 'git fetch --force origin "${retirement_ref_value}:${retirement_ref_value}"'
assert_not_contains "reconciliation no ref deletion" "$reconciliation_text" "git update-ref -d"
assert_not_contains "reconciliation no git push" "$reconciliation_text" "git push"
assert_not_contains "reconciliation no force push" "$reconciliation_text" "git push --force"

orphan_recovery_text="$(<"$root_dir/.github/workflows/release-orphan-recovery.yml")"
assert_contains "orphan recovery workflow name" "$orphan_recovery_text" "name: Recover unbound release reservation"
assert_contains "orphan recovery dispatch trigger" "$orphan_recovery_text" "workflow_dispatch:"
assert_contains "orphan recovery product tag absence" "$orphan_recovery_text" 'git/ref/tags/v${RELEASE_VERSION}'
assert_contains "orphan recovery release absence" "$orphan_recovery_text" 'releases/tags/v${RELEASE_VERSION}'
assert_contains "orphan recovery run artifact check" "$orphan_recovery_text" 'actions/runs/${run_id}/artifacts'
assert_contains "orphan recovery trusted main ancestry" "$orphan_recovery_text" 'git merge-base --is-ancestor'
assert_contains "orphan recovery writer" "$orphan_recovery_text" "release_reservation.py recover-orphan"
assert_contains "orphan recovery namespace" "$orphan_recovery_text" "release-orphaned/v"
assert_not_contains "orphan recovery no ref deletion" "$orphan_recovery_text" "git update-ref -d"
assert_not_contains "orphan recovery no git push" "$orphan_recovery_text" "git push"

release_text="$(<"$root_dir/.github/workflows/release.yml")"
assert_contains "release target output" "$release_text" "release_target"
assert_contains "release policy output" "$release_text" "policy"
assert_contains "release snapshot target" "$release_text" 'release_target:$release_target'
assert_contains "release snapshot policy" "$release_text" 'policy:$policy'
assert_contains "release reservation verification" "$release_text" "verify_github_reservation"
assert_contains "release sequence gate" "$release_text" "verify-release-sequence"
assert_contains "release immutable product tag" "$release_text" "Create or verify immutable product tag"
assert_contains "release identity receipt" "$release_text" "--state consumed"
assert_contains "recovery artifact reuse job" "$release_text" "reuse_release_assets"
assert_contains "recovery artifact receipt" "$release_text" "verify-artifact-receipt"
assert_contains "recovery artifact exact run" "$release_text" "actions/runs/\${candidate_run_id}/artifacts"
assert_contains "recovery artifact source run" "$release_text" "actions/runs/\${recovery_run_id}"
assert_contains "recovery fail-closed artifact" "$release_text" "same-identity recovery requires an unexpired prior release-assets artifact"
assert_contains "recovery missing pending receipt" "$release_text" "same-identity recovery cannot prove whether release-assets was ever recorded"
assert_contains "recovery trusted branch" "$release_text" '.head_branch == "main"'
assert_contains "recovery trusted workflow event" "$release_text" '.event == "workflow_run" or .event == "workflow_dispatch"'
assert_contains "recovery trusted workflow path" "$release_text" '.path == ".github/workflows/release.yml"'
assert_contains "recovery trusted repository" "$release_text" '.repository.full_name == $repository'
assert_contains "recovery trusted main ancestry" "$release_text" 'git merge-base --is-ancestor "${recovery_head_sha}" "${main_sha}"'
assert_contains "recovery artifact archive digest" "$release_text" "Verify immutable release-assets archive digest"
assert_contains "recovery packaging policy identity" "$release_text" "packaging_policy_sha"
assert_contains "recovery execution policy identity" "$release_text" "execution_policy_commit_sha"
assert_contains "recovery pre-artifact repair" "$release_text" "rebuilding pre-artifact output"
assert_contains "recovery no rebuild" "$release_text" "reuse_artifacts != 'true'"
assert_contains "recovery direct archive consumption" "$release_text" 'unzip -q "${archive}"'
assert_contains "release artifact attempt naming" "$release_text" 'source_artifact_name="release-assets-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"'
assert_contains "release artifact receipt name binding" "$release_text" 'artifact_name "${candidate_artifact_name}"'
assert_contains "release artifact rerun receipt reuse" "$release_text" "Check immutable release artifact receipt"
assert_contains "release artifact rerun upload guard" "$release_text" "steps.release-artifact-name.outputs.reuse != 'true'"
reuse_text="${release_text#*reuse_release_assets:}"
reuse_text="${reuse_text%%  publish:*}"
assert_not_contains "recovery second artifact download" "$reuse_text" "actions/download-artifact"
assert_not_contains "recovery global run history scan" "$release_text" "actions/workflows/release.yml/runs?per_page=100"
assert_not_contains "release label authority" "$release_text" "labels_json"
assert_not_contains "release label authority environment" "$release_text" "RELEASE_TYPE"
assert_not_contains "release intent trailers" "$release_text" "Release-Intent"
assert_contains "release publish dependencies" "$release_text" "needs: [resolve, assemble, reuse_release_assets]"
assert_not_contains "stable RC acceptance job" "$release_text" "macos-acceptance:"
assert_not_contains "stable RC acceptance environment" "$release_text" "macos-release-acceptance"
assert_not_contains "stable RC acceptance dependency" "$release_text" "needs.macos-acceptance.result"

homebrew_update_text="$(<"$root_dir/.github/workflows/homebrew-cask-update.yml")"
assert_contains "homebrew legacy label bridge dispatch" "$homebrew_update_text" "gh workflow run legacy-label-gate-bridge.yml"
assert_contains "homebrew legacy bridge head binding" "$homebrew_update_text" 'head_sha=${HEAD_SHA}'

notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
assert_contains "notifier target output" "$notify_text" "release_target:"
assert_contains "notifier target validation" "$notify_text" "release target is not a supported manual target"
assert_contains "notifier policy validation" "$notify_text" "televybackup-manual-semver-v1"
assert_contains "notifier immutable reservation validation" "$notify_text" "verify_reservation"
assert_contains "notifier immutable bound receipt validation" "$notify_text" "verify_bound_receipt"
assert_contains "notifier reclaimed reservation validation" "$notify_text" "release-reservation-reclaimed"
assert_contains "notifier reclaimed decision validation" "$notify_text" "release-decision-reclaimed"
assert_not_contains "notifier label authority" "$notify_text" "labels_json"
assert_not_contains "notifier intent trailers" "$notify_text" "Release-Intent"

merge_group_text="$(<"$root_dir/.github/scripts/merge-group-release-gate.sh")"
assert_contains "merge-group target gate requirement" "$merge_group_text" 'required=("Release target gate"'
assert_contains "merge-group target fields" "$merge_group_text" "releaseTarget"
assert_contains "merge-group completion verifier" "$merge_group_text" "release_completion.py"
assert_contains "merge-group current main lock" "$merge_group_text" "ensure_current_main"
assert_contains "merge-group base lock" "$merge_group_text" "merge_group_base_sha"
assert_not_contains "merge-group label authority" "$merge_group_text" ".labels"
assert_not_contains "merge-group label mode" "$merge_group_text" "labels"

python3 - "$root_dir" <<'PY'
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
contract = json.loads((root / ".github/release-contract.json").read_text(encoding="utf-8"))
assert contract["schema_version"] == 4
assert contract["source_of_truth"].startswith("one owner-supplied release_target")
policy = contract["version_policy"]
assert policy["id"] == "televybackup-manual-semver-v1"
assert policy["input"] == "release_target"
assert policy["symbolic_targets"] == ["patch", "minor", "major"]
assert policy["successor_on_collision"] is False
assert policy["reservation_key"] == "pull_request_number+release_target"
assert contract["workflows"]["target_gate"] == "Release target gate"
assert contract["workflows"]["reconciliation"] == "Reconcile unpublished legacy release identity"
assert contract["workflows"]["orphan_recovery"] == "Recover unbound release reservation"
assert contract["identity_refs"]["orphaned"] == "refs/tags/release-orphaned/v<version>/<reservation-id>"
assert contract["recovery"]["orphan_unbound"]["append_only"] is True
assert contract["preparation"]["reservation_before_version"] is True
assert contract["release_sequence"]["successor_on_collision"] is False
assert contract["release_sequence"]["successor_on_recovery"] is False
retired = json.loads((root / ".github/pr-label-release.json").read_text(encoding="utf-8"))
assert retired["status"] == "retired"
assert retired["source_of_truth"] == "none"
assert retired["successor"] == "manual-version-release-delivery"
quality = json.loads((root / ".github/quality-gates.json").read_text(encoding="utf-8"))
assert "Release target gate" in quality["required_checks"]
assert "Release intent label gate" not in quality["required_checks"]
PY

echo "release workflow contract tests passed"
