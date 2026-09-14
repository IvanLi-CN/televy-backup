#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
text="$(<"$root_dir/.github/workflows/notify-release-failure.yml")"
python3 "$root_dir/.github/scripts/test-release-failure-context.py"
for marker in identity_status no-identity resolver-error merge_sha merge_commit_sha preparation_commit_sha recovery_candidate reservation_ref pull_request source_sha head_sha tag_status tag_target_sha tag_owner reservation_id reservation_owner reservation_state reservation_merge_commit_sha provenance_verified signature_verified branch_head artifact_names recovery_instruction; do
  [[ "$text" == *"$marker"* ]] || { echo "missing failure context marker: $marker" >&2; exit 1; }
done
[[ "$text" == *"not-applicable: superseded_by_product_tag"* ]]
[[ "$text" != *"contents/VERSION"* ]]
[[ "$text" != *"base64.b64decode"* ]]
[[ "$text" == *"no version or tag was inferred"* ]]
[[ "$text" == *"release-intent artifact"* ]]
[[ "$text" == *'target_sha: ${{ needs.resolve_release_context.outputs.merge_sha }}'* ]]
[[ "$text" == *'merge_commit_sha: ${{ needs.resolve_release_context.outputs.merge_commit_sha }}'* ]]
[[ "$text" == *'preparation_commit_sha: ${{ needs.resolve_release_context.outputs.preparation_commit_sha }}'* ]]
[[ "$text" == *'contents: read'* ]]
[[ "$text" == *'pull-requests: read'* ]]
[[ "$text" == *'preparation commit GitHub signature is not verified'* ]]
[[ "$text" == *'declared pull request is not associated with the merge SHA'* ]]
[[ "$text" == *'reservation commit provenance does not match source SHA'* ]]
[[ "$text" == *'bound receipt provenance does not match merge SHA'* ]]
[[ "$text" == *'product_tag_target'* ]]
[[ "$text" == *"product tag: {tag_status}"* ]]
[[ "$text" != *'outputs.merge_sha || needs.resolve_release_context.outputs.head_sha'* ]]
echo "release failure context contract tests passed"
