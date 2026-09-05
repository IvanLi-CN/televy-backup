#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

bash -n \
  "$root_dir/.github/scripts/label-gate.sh" \
  "$root_dir/.github/scripts/release_chain.py" \
  "$root_dir/.github/scripts/release_preparation.py" \
  "$root_dir/.github/scripts/release_completion.py"

python3 "$root_dir/scripts/test-product-version.py"

out="$(LABELS_JSON='[{"name":"type:patch"},{"name":"channel:stable"}]' \
  "$root_dir/.github/scripts/label-gate.sh")"
[[ "$out" == *"Intent label OK: type:patch"* ]]
[[ "$out" == *"release_channel=stable"* ]]
if LABELS_JSON='[{"name":"type:patch"}]' "$root_dir/.github/scripts/label-gate.sh" >/dev/null 2>&1; then
  echo "label gate accepted a missing channel" >&2
  exit 1
fi
if LABELS_JSON='[{"name":"type:patch"},{"name":"channel:stable"},{"name":"channel:rc"}]' "$root_dir/.github/scripts/label-gate.sh" >/dev/null 2>&1; then
  echo "label gate accepted duplicate channels" >&2
  exit 1
fi

python3 - "$root_dir" <<'PY'
from pathlib import Path
import json
import sys

root = Path(sys.argv[1])
contract = json.loads((root / ".github/release-contract.json").read_text(encoding="utf-8"))
assert contract["source_of_truth"] == "VERSION"
assert contract["preparation"]["write_api"] == "createCommitOnBranch"
assert contract["preparation"]["expected_head_oid"] is True
assert contract["preparation"]["no_gpg_secrets"] is True
assert contract["recovery"]["backfill"] is False

workflow_text = "\n".join(
    (root / ".github/workflows" / name).read_text(encoding="utf-8")
    for name in ("release-preparation.yml", "release-completion.yml", "release.yml")
)
for forbidden in ("GPG", "release-backfill", "backfill", "snapshot", "queue"):
    assert forbidden not in workflow_text, forbidden
assert "createCommitOnBranch" in workflow_text
assert "expectedHeadOid" in workflow_text
assert ".commit.verification.verified" in workflow_text
release_workflow = (root / ".github/workflows/release.yml").read_text(encoding="utf-8")
assert "options: [recover]" in release_workflow
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
