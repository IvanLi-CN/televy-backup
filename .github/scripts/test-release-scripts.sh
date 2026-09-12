#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

bash -n "$root_dir/.github/scripts/label-gate.sh"
python3 -m py_compile \
  "$root_dir/.github/scripts/release_chain.py" \
  "$root_dir/.github/scripts/release_preparation.py" \
  "$root_dir/.github/scripts/release_completion.py" \
  "$root_dir/.github/scripts/verify-macos-rc-acceptance.py"

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
assert contract["recovery"]["sequence_guard"] == "candidate must not be below the highest remote product tag"
assert contract["release_sequence"]["source"] == "remote product tags"
assert contract["release_states"]["published"] == "idempotent-success-without-build-or-overwrite"

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
assert 'bootstrap_release_tag' in release_workflow
assert 'TELEVYBACKUP_SNAPSHOT_ACCESS_SOURCE=rc1-universal-artifact' in release_workflow
assert 'source_is_prerelease' in release_workflow
assert '--pattern "BUILD-MANIFEST.json"' in release_workflow
assert 'SHA256SUMS" --dir' in release_workflow
assert '--manifest "$RUNNER_TEMP/snapshot-helper/BUILD-MANIFEST.json"' in release_workflow
assert 'source_tag_commit="$(git rev-list -n 1 "${HELPER_SOURCE_TAG}^{commit}")"' in release_workflow
assert 'manifest["source_commit"] == sys.argv[5]' in release_workflow
assert "macos-release-acceptance" in release_workflow
assert "TELEVYBACKUP_MACOS_RC_ACCEPTANCE_EVIDENCE" in release_workflow
assert "verify-macos-rc-acceptance.py" in release_workflow
assert "actions/download-artifact@v4" in release_workflow
assert 'rc1_json="$(gh release view "$rc1_tag"' in release_workflow
assert 'rc2_json="$(gh release view "$rc2_tag"' in release_workflow
assert "fda_regrant_requested" in (root / ".github/scripts/verify-macos-rc-acceptance.py").read_text(encoding="utf-8")
assert "artifact_sha256" in (root / ".github/scripts/verify-macos-rc-acceptance.py").read_text(encoding="utf-8")
assert "needs.macos-acceptance.result == 'success'" in release_workflow
assert "needs.assemble.result == 'success'" in release_workflow
assert "final assembly" in release_workflow
PY

python3 - "$root_dir" <<'PY'
import hashlib
import json
import subprocess
import sys
import tempfile
from pathlib import Path

root = Path(sys.argv[1])
verifier = root / ".github/scripts/verify-macos-rc-acceptance.py"
identity = {
    "sha256": "helper-sha",
    "artifact_sha256": "helper-artifact",
    "cdhash": "helper-cdhash",
    "designated_requirement": "helper-requirement",
}

def manifest(version, source_commit, dmg_name, dmg_digest):
    return {
        "release_version": version,
        "source_commit": source_commit,
        "components": {"snapshot_access": identity.copy()},
        "assets": [{"name": dmg_name, "sha256": dmg_digest, "bytes": 4}],
    }

evidence = {
    "schema_version": 1,
    "product": "TelevyBackup",
    "stable_version": "1.0.0",
    "rc1_tag": "v1.0.0-rc.1",
    "rc2_tag": "v1.0.0-rc.2",
    "legacy_registration_migrated": True,
    "strict_backup_rc1": True,
    "strict_backup_rc2": True,
    "fda_grants": 1,
    "fda_regrant_requested": False,
    "root_mount_helper_unchanged": True,
    "snapshot_access": identity.copy(),
    "root_mount_helper": {
        "install_path": "/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper",
        "component_version": "0.1.0",
        "protocol_version": 1,
        "rc1": identity.copy(),
        "rc2": identity.copy(),
    },
}

with tempfile.TemporaryDirectory() as directory:
    temp = Path(directory)
    stable_manifest = {
        "release_version": "1.0.0",
        "source_commit": "stable-source",
        "components": {
            "snapshot_access": identity.copy(),
            "snapshot_mount_helper": {
                "install_path": evidence["root_mount_helper"]["install_path"],
                "component_version": "0.1.0",
                "compatible_component_versions": ["0.1.0", "0.9.8"],
                "protocol_version": 1,
                **identity,
            },
        },
    }
    stable_path = temp / "stable.json"
    stable_path.write_text(json.dumps(stable_manifest), encoding="utf-8")
    rc_args = []
    for number, source in ((1, "rc1-source"), (2, "rc2-source")):
        version = f"1.0.0-rc.{number}"
        name = f"TelevyBackup-{version}.dmg"
        dmg = temp / name
        dmg.write_bytes(b"dmg\n")
        digest = hashlib.sha256(dmg.read_bytes()).hexdigest()
        (temp / f"rc{number}.json").write_text(
            json.dumps(manifest(version, source, name, digest)), encoding="utf-8"
        )
        (temp / f"rc{number}.sums").write_text(f"{digest}  {name}\n", encoding="utf-8")
        rc_args.extend([
            f"--rc{number}-manifest", str(temp / f"rc{number}.json"),
            f"--rc{number}-checksums", str(temp / f"rc{number}.sums"),
            f"--rc{number}-dmg", str(dmg),
            f"--rc{number}-source-commit", source,
        ])
    common = [
        sys.executable, str(verifier), "--evidence", json.dumps(evidence),
        "--manifest", str(stable_path), "--stable-version", "1.0.0",
        "--rc1-tag", "v1.0.0-rc.1", "--rc2-tag", "v1.0.0-rc.2",
        "--stable-source-commit", "stable-source", *rc_args,
    ]
    assert subprocess.run(common, capture_output=True, text=True).returncode == 0
    stale = temp / "rc2.json"
    stale.write_text(stale.read_text(encoding="utf-8").replace("helper-sha", "stale-sha"), encoding="utf-8")
    assert subprocess.run(common, capture_output=True, text=True).returncode != 0
    stable_manifest["components"]["snapshot_mount_helper"]["cdhash"] = "stale-cdhash"
    stable_path.write_text(json.dumps(stable_manifest), encoding="utf-8")
    assert subprocess.run(common, capture_output=True, text=True).returncode != 0
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
