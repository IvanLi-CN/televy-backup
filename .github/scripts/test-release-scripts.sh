#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

bash -n "$root_dir/.github/scripts/label-gate.sh"
python3 -m py_compile \
  "$root_dir/.github/scripts/release_chain.py" \
  "$root_dir/.github/scripts/release_reservation.py" \
  "$root_dir/.github/scripts/release_preparation.py" \
  "$root_dir/.github/scripts/release_completion.py"

python3 "$root_dir/scripts/test-product-version.py"
bash "$root_dir/.github/scripts/test-release-failure-context.sh"
bash "$root_dir/.github/scripts/test-release-github-api.sh"

out="$(LABELS_JSON='[{"name":"type:patch"},{"name":"channel:prod"}]' \
  "$root_dir/.github/scripts/label-gate.sh")"
[[ "$out" == *"Intent label OK: type:patch"* ]]
[[ "$out" == *"release_channel=prod"* ]]
if LABELS_JSON='[{"name":"type:patch"}]' "$root_dir/.github/scripts/label-gate.sh" >/dev/null 2>&1; then
  echo "label gate accepted a missing channel" >&2
  exit 1
fi
if LABELS_JSON='[{"name":"type:patch"},{"name":"channel:prod"},{"name":"channel:rc"}]' "$root_dir/.github/scripts/label-gate.sh" >/dev/null 2>&1; then
  echo "label gate accepted duplicate channels" >&2
  exit 1
fi

python3 - "$root_dir" <<'PY'
from pathlib import Path
import json
import sys

root = Path(sys.argv[1])
contract = json.loads((root / ".github/release-contract.json").read_text(encoding="utf-8"))
assert "immutable repository identity refs" in contract["source_of_truth"]
assert contract["preparation"]["write_api"] == "createCommitOnBranch"
assert contract["preparation"]["expected_head_oid"] is True
assert contract["preparation"]["no_gpg_secrets"] is True
assert contract["recovery"]["historical_backfill"] is False
assert contract["release_sequence"]["final_baseline"] == "highest final vX.Y.Z only"
assert contract["release_states"]["published"] == "idempotent-success-without-build-or-overwrite"
assert contract["identity_refs"]["write_policy"] == "append-only-create"
assert contract["identity_refs"]["state_order"] == "bound-before-consumed;released-only-when-unbound"
assert contract["identity_refs"]["receipt_validation"] == "independently-verify-reservation-provenance"
assert contract["recovery"]["dispatch_requires_existing_bound"] is True

workflow_text = "\n".join(
    (root / ".github/workflows" / name).read_text(encoding="utf-8")
    for name in ("release-preparation.yml", "release-completion.yml", "release.yml")
)
for forbidden in ("GPG", "release-backfill", "backfill", "queue"):
    assert forbidden not in workflow_text, forbidden
assert "createCommitOnBranch" in workflow_text
assert "expectedHeadOid" in workflow_text
assert ".commit.verification.verified" in workflow_text
assert "verify-release-sequence" in workflow_text
release_workflow = (root / ".github/workflows/release.yml").read_text(encoding="utf-8")
assert "options: [recover]" in release_workflow
assert "helper_source_tag" in release_workflow
assert "hdiutil attach" in release_workflow
assert "verify-component-identity.sh" in release_workflow
assert "stable release requires a previously published RC" in release_workflow
assert 'helper_source_tag="v${core_version}-rc.1"' in release_workflow
assert 'source_is_prerelease' in release_workflow
assert 'BUILD-MANIFEST.json" --dir' in release_workflow
assert '--manifest "$RUNNER_TEMP/snapshot-helper/BUILD-MANIFEST.json"' in release_workflow
assert 'source_tag_commit="$(git rev-list -n 1 "${HELPER_SOURCE_TAG}^{commit}")"' in release_workflow
assert 'manifest["source_commit"] == sys.argv[3]' in release_workflow
assert "final assembly" in release_workflow
PY

ruby -ryaml -e 'ARGV.each { |path| YAML.load_file(path) }' \
  "$root_dir/.github/workflows/ci-pr.yml" \
  "$root_dir/.github/workflows/ci-main.yml" \
  "$root_dir/.github/workflows/label-gate.yml" \
  "$root_dir/.github/workflows/package-ci.yml" \
  "$root_dir/.github/workflows/release-preparation.yml" \
  "$root_dir/.github/workflows/release-completion.yml" \
  "$root_dir/.github/workflows/release.yml" \
  "$root_dir/.github/workflows/notify-release-failure.yml"

echo "release script contract tests passed"
