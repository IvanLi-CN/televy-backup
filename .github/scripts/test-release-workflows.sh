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

for workflow in ci-pr.yml ci-main.yml legacy-label-gate-bridge.yml release-target-gate.yml package-ci.yml release-preparation.yml release-completion.yml release.yml notify-release-failure.yml homebrew-cask.yml homebrew-cask-update.yml; do
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
assert_contains "preparation CI dispatch" "$preparation_text" "gh workflow run ci-pr.yml"
assert_contains "preparation package dispatch" "$preparation_text" "gh workflow run package-ci.yml"
assert_contains "preparation manual target gate dispatch" "$preparation_text" "gh workflow run release-target-gate.yml"
assert_contains "preparation manual completion dispatch" "$preparation_text" "gh workflow run release-completion.yml"
assert_contains "preparation dispatch head binding" "$preparation_text" 'head_sha=${prepared_head_sha}'
assert_contains "preparation descendant head binding" "$preparation_text" "git merge-base --is-ancestor"
assert_not_contains "preparation label input" "$preparation_text" "labels-json"
assert_not_contains "preparation intent trailers" "$preparation_text" "Release-Intent"

legacy_label_bridge_text="$(<"$root_dir/.github/workflows/legacy-label-gate-bridge.yml")"
assert_contains "legacy label bridge workflow" "$legacy_label_bridge_text" "name: Label Gate"
assert_contains "legacy label bridge check" "$legacy_label_bridge_text" "name: Release intent label gate"
assert_contains "legacy label bridge explanation" "$legacy_label_bridge_text" "release labels are not release authority"
assert_not_contains "legacy label bridge label parsing" "$legacy_label_bridge_text" ".labels"

target_gate_text="$(<"$root_dir/.github/workflows/release-target-gate.yml")"
assert_contains "target gate workflow name" "$target_gate_text" "name: Release target gate"
assert_contains "target gate pull request trigger" "$target_gate_text" "pull_request_target:"
assert_contains "target gate trusted main resolution" "$target_gate_text" "git/ref/heads/main"
assert_contains "target gate trusted policy checkout" "$target_gate_text" 'ref: ${{ steps.trusted-main.outputs.sha }}'
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
assert_contains "completion current main check" "$completion_text" "validate_current_main"
assert_contains "completion trusted dispatch ref" "$completion_text" 'test "${GITHUB_REF}" = "refs/heads/main"'
assert_contains "completion exact check publisher" "$completion_text" "publish-pr-check.py"
assert_contains "completion pull request head publisher" "$completion_text" "github.event_name != 'merge_group'"
assert_not_contains "completion label authority" "$completion_text" ".labels"
assert_not_contains "completion label gate workflow" "$completion_text" "label-gate.yml"
assert_not_contains "completion intent trailers" "$completion_text" "Release-Intent"

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
assert_contains "recovery artifact source run" "$release_text" "run-id: \${{ needs.resolve.outputs.recovery_run_id }}"
assert_contains "recovery fail-closed artifact" "$release_text" "same-identity recovery requires an unexpired prior release-assets artifact"
assert_contains "recovery trusted branch" "$release_text" '"${candidate_branch}" == main'
assert_contains "recovery trusted workflow event" "$release_text" '"${candidate_event}" == workflow_run || "${candidate_event}" == workflow_dispatch'
assert_contains "recovery trusted workflow path" "$release_text" '"${candidate_workflow_path}" == .github/workflows/release.yml'
assert_contains "recovery trusted repository" "$release_text" '"${candidate_head_repository}" == "${GITHUB_REPOSITORY}"'
assert_contains "recovery trusted main ancestry" "$release_text" 'git merge-base --is-ancestor "${candidate_head_sha}" "${main_sha}"'
assert_contains "recovery artifact archive digest" "$release_text" "Verify immutable release-assets archive digest"
assert_contains "recovery packaging policy identity" "$release_text" "packaging_policy_sha"
assert_contains "recovery execution policy identity" "$release_text" "execution_policy_commit_sha"
assert_contains "recovery pre-artifact repair" "$release_text" "rebuilding pre-artifact output"
assert_contains "recovery no rebuild" "$release_text" "reuse_artifacts != 'true'"
assert_not_contains "release label authority" "$release_text" "labels_json"
assert_not_contains "release label authority environment" "$release_text" "RELEASE_TYPE"
assert_not_contains "release intent trailers" "$release_text" "Release-Intent"

notify_text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
assert_contains "notifier target output" "$notify_text" "release_target:"
assert_contains "notifier target validation" "$notify_text" "release target is not a supported manual target"
assert_contains "notifier policy validation" "$notify_text" "televybackup-manual-semver-v1"
assert_contains "notifier immutable reservation validation" "$notify_text" "verify_reservation"
assert_contains "notifier immutable bound receipt validation" "$notify_text" "verify_bound_receipt"
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
assert contract["schema_version"] == 3
assert contract["source_of_truth"].startswith("one owner-supplied release_target")
policy = contract["version_policy"]
assert policy["id"] == "televybackup-manual-semver-v1"
assert policy["input"] == "release_target"
assert policy["symbolic_targets"] == ["patch", "minor", "major"]
assert policy["successor_on_collision"] is False
assert policy["reservation_key"] == "pull_request_number+release_target"
assert contract["workflows"]["target_gate"] == "Release target gate"
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
