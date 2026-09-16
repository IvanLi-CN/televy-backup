#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

bash -n "$root_dir/.github/scripts/label-gate.sh"
bash -n "$root_dir/.github/scripts/merge-group-release-gate.sh"
python3 -m py_compile \
  "$root_dir/.github/scripts/release_chain.py" \
  "$root_dir/.github/scripts/release_reservation.py" \
  "$root_dir/.github/scripts/release_preparation.py" \
  "$root_dir/.github/scripts/release_completion.py" \
  "$root_dir/.github/scripts/release_helper.py" \
  "$root_dir/.github/scripts/verify-macos-rc-acceptance.py"

python3 "$root_dir/scripts/test-product-version.py"
bash "$root_dir/.github/scripts/test-release-helper.sh"
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
assert "append-only repository identity refs" in contract["source_of_truth"]
assert contract["preparation"]["write_api"] == "createCommitOnBranch"
assert contract["preparation"]["expected_head_oid"] is True
assert contract["preparation"]["no_gpg_secrets"] is True
assert contract["execution_authority"]["release_policy"] == "trusted-main-checkout"
assert contract["execution_authority"]["write_capable_product_checkout"] is False
assert contract["recovery"]["historical_backfill"] is False
assert contract["release_sequence"]["final_baseline"] == "highest final vX.Y.Z only"
assert contract["release_states"]["published"] == "idempotent-success-without-build-or-overwrite"
assert contract["helper_bootstrap"]["bootstrap_preserves_identity"] is True
assert contract["helper_bootstrap"]["reuse_policy"] == "byte-identical-no-rebuild-no-lipo-no-resign"
assert contract["helper_bootstrap"]["identity_fields"] == ["sha256", "artifact_sha256", "cdhash", "designated_requirement"]
assert contract["helper_bootstrap"]["invalid_candidate_policy"] == "fallback-to-next-candidate"
assert contract["helper_bootstrap"]["terminal_states_before_resolution"] == ["published-release", "consumed-receipt"]
assert contract["helper_bootstrap"]["immutable_source_artifact"]["name"] == "snapshot-helper-source"
assert contract["helper_bootstrap"]["immutable_source_artifact"]["release_redownload_after_resolve"] is False
assert contract["identity_refs"]["write_policy"] == "append-only-create"
assert contract["identity_refs"]["state_order"] == "decision-before-bound-or-released;bound-before-consumed;released-only-when-unbound"
assert contract["identity_refs"]["receipt_validation"] == "independently-verify-reservation-provenance"
assert contract["identity_refs"]["automation"] == "default-github-actions-token"
assert contract["identity_refs"]["server_protection"] == "product-tags-only"
assert contract["identity_refs"]["ruleset_product_pattern"] == "refs/tags/v*"
assert contract["identity_refs"]["ruleset_excluded_identity_patterns"] == [
    "refs/tags/release-reservation/*",
    "refs/tags/release-decision/*",
    "refs/tags/release-bound/*",
    "refs/tags/release-consumed/*",
    "refs/tags/release-released/*",
]
assert contract["identity_refs"]["no_additional_ci_credentials"] is True
assert contract["recovery"]["dispatch_requires_existing_bound"] is False
assert contract["recovery"]["dispatch_bound_repair"] == "append-only-after-same-sha-provenance"
for gate in ("label_gate", "completion"):
    scheduling = contract["required_gate_scheduling"][gate]
    assert scheduling["queue"] == "max"
    assert scheduling["cancel_in_progress"] is False

workflow_text = "\n".join(
    (root / ".github/workflows" / name).read_text(encoding="utf-8")
    for name in ("release-preparation.yml", "release-completion.yml", "release.yml")
)
for forbidden in (
    "GPG",
    "release-backfill",
    "backfill",
    "create-github-app-token",
    "TELEVYBACKUP_RELEASE_APP_ID",
    "TELEVYBACKUP_RELEASE_APP_PRIVATE_KEY",
    "RELEASE_REF_TOKEN",
):
    assert forbidden not in workflow_text, forbidden
assert "createCommitOnBranch" in workflow_text
assert "expectedHeadOid" in workflow_text
assert ".commit.verification.verified" in workflow_text
assert "verify-release-sequence" in workflow_text
assert '--expected-source-commit "${{ needs.resolve.outputs.merge_sha }}"' in (root / ".github/workflows/release.yml").read_text(encoding="utf-8")
release_workflow = (root / ".github/workflows/release.yml").read_text(encoding="utf-8")
assert "options: [recover]" in release_workflow
assert "helper_source_mode" in release_workflow
assert "helper_source_tag" in release_workflow
assert "hdiutil attach" in release_workflow
assert "verify-component-identity.sh" in release_workflow
assert "stable release requires a previously published RC" in release_workflow
assert "gh release list" in release_workflow
assert "helper_mode=bootstrap" in release_workflow
assert "one-time-bootstrap-universal-build" in release_workflow
assert 'helper_source_tag="v${core_version}-rc.1"' not in release_workflow
assert 'bootstrap_release_tag' in release_workflow
assert 'TELEVYBACKUP_SNAPSHOT_ACCESS_SOURCE=rc1-universal-artifact' in release_workflow
assert 'source_is_prerelease' in release_workflow
assert '--pattern "BUILD-MANIFEST.json"' in release_workflow
assert '--pattern "SHA256SUMS"' in release_workflow
assert '--manifest "$RUNNER_TEMP/snapshot-helper/BUILD-MANIFEST.json"' in release_workflow
assert 'source_tag_commit="$(git rev-list -n 1 "${candidate}^{commit}"' in release_workflow
assert "release_helper.py verify-assets" in release_workflow
assert "Snapshot Access source component contract mismatch" in (root / ".github/scripts/release_helper.py").read_text(encoding="utf-8")
assert 'source_artifact_dir="${RUNNER_TEMP}/snapshot-helper-source"' in release_workflow
assert "Upload immutable helper source" in release_workflow
assert "name: snapshot-helper-source" in release_workflow
assert "skipping helper candidate with incompatible immutable assets" in release_workflow
assert "preferred helper source is missing" not in release_workflow
assert "preferred helper source manifest failed" not in release_workflow
assert "Keep the trusted main checkout for release policy scripts" in release_workflow
assert 'git checkout --detach "${TARGET_INPUT}"' not in release_workflow
assert 'policy_sha: ${{ steps.release.outputs.policy_sha }}' in release_workflow
assert 'policy_sha="$(git rev-parse HEAD)"' in release_workflow
assert 'test "${policy_sha}" = "${main_sha}"' in release_workflow
assert 'echo "policy_sha=${policy_sha}" >> "$GITHUB_OUTPUT"' in release_workflow
assert 'git cat-file -e "${POLICY_SHA}^{commit}"' in release_workflow
assert 'git worktree add --detach "${policy_checkout}" "${POLICY_SHA}"' in release_workflow
assert 'cp "$GITHUB_WORKSPACE/VERSION" "${policy_checkout}/VERSION"' in release_workflow
assert 'policy_verify_release_assets="${policy_checkout}/scripts/macos/verify-release-assets.sh"' in release_workflow
assert 'cd "${policy_checkout}"' in release_workflow
assert '"${policy_verify_release_assets}" --mode release --asset-dir "$GITHUB_WORKSPACE/dist/final" --expected-source-commit "${{ needs.resolve.outputs.merge_sha }}" --expected-packaging-commit "${POLICY_SHA}"' in release_workflow
assert 'git show "${POLICY_SHA}:scripts/macos/verify-release-assets.sh"' not in release_workflow
assert 'bash scripts/macos/verify-release-assets.sh --mode release --asset-dir dist/final' not in release_workflow
build_and_assembly = release_workflow.split("  build-arm64:", 1)[1].split("  macos-acceptance:", 1)[0]
assert "gh release download" not in build_and_assembly
assert build_and_assembly.count("name: snapshot-helper-source") == 3
assert release_workflow.index("release_state=missing") < release_workflow.index("helper_candidates_json")
assert release_workflow.index("consumed_state=missing") < release_workflow.index("helper_candidates_json")
assert release_workflow.index("RELEASE_TERMINAL state=consumed") < release_workflow.index("helper_candidates_json")
assert "consumed receipt exists without a matching product tag" in release_workflow
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
assert "Assemble and validate final assets" in release_workflow
PY

python3 - "$root_dir" <<'PY'
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

root = Path(sys.argv[1])
version = (root / "VERSION").read_text(encoding="utf-8").strip()
validator_source = root / "scripts/macos/verify-release-assets.sh"

with tempfile.TemporaryDirectory() as directory:
    temp = Path(directory)
    policy_repo = temp / "policy-repo"
    asset_dir = temp / "dist"
    (policy_repo / "scripts/macos").mkdir(parents=True)
    (policy_repo / "packaging/macos").mkdir(parents=True)
    shutil.copytree(
        root / "assets/brand/macos/dmg",
        policy_repo / "assets/brand/macos/dmg",
    )
    asset_dir.mkdir()
    shutil.copy2(validator_source, policy_repo / "scripts/macos/verify-release-assets.sh")
    shutil.copy2(
        root / "scripts/macos/generate-release-manifest.sh",
        policy_repo / "scripts/macos/generate-release-manifest.sh",
    )
    shutil.copy2(root / "scripts/product-version.py", policy_repo / "scripts/product-version.py")
    (policy_repo / "VERSION").write_text("0.9.9-rc.37\n", encoding="utf-8")
    shutil.copy2(
        root / "packaging/macos/snapshot-components.lock.json",
        policy_repo / "packaging/macos/snapshot-components.lock.json",
    )
    subprocess.run(["git", "-C", str(policy_repo), "init", "-q"], check=True)
    subprocess.run(["git", "-C", str(policy_repo), "config", "user.name", "fixture"], check=True)
    subprocess.run(
        ["git", "-C", str(policy_repo), "config", "user.email", "fixture@example.com"],
        check=True,
    )

    asset_names = [
        f"TelevyBackup-{version}.dmg",
        f"TelevyBackup-{version}-arm64.dmg",
        f"TelevyBackup-{version}-x86_64.dmg",
        f"televybackup-tools-{version}-arm64.tar.gz",
        f"televybackup-tools-{version}-x86_64.tar.gz",
    ]
    for name in asset_names:
        (asset_dir / name).write_bytes(f"bootstrap fixture: {name}\n".encode())
    subprocess.run(
        ["git", "-C", str(policy_repo), "add", "."],
        check=True,
    )
    subprocess.run(
        ["git", "-C", str(policy_repo), "commit", "-qm", "trusted policy fixture"],
        check=True,
    )
    policy_sha = subprocess.check_output(
        ["git", "-C", str(policy_repo), "rev-parse", "HEAD"], text=True
    ).strip()
    policy_checkout = temp / "policy-checkout"
    subprocess.run(
        [
            "git",
            "-C",
            str(policy_repo),
            "worktree",
            "add",
            "--detach",
            str(policy_checkout),
            policy_sha,
        ],
        check=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        assert (
            subprocess.check_output(
                ["git", "-C", str(policy_checkout), "rev-parse", "HEAD"], text=True
            ).strip()
            == policy_sha
        )
        shutil.copy2(root / "VERSION", policy_checkout / "VERSION")
        generation_env = dict(os.environ)
        generation_env["TELEVYBACKUP_SNAPSHOT_ACCESS_SOURCE"] = (
            "one-time-bootstrap-universal-build"
        )
        generation = subprocess.run(
            [
                str(policy_checkout / "scripts/macos/generate-release-manifest.sh"),
                "--mode",
                "release",
                "--asset-dir",
                str(asset_dir),
                "--source-commit",
                "1" * 40,
                "--packaging-commit",
                "2" * 40,
                "--output",
                str(asset_dir / "BUILD-MANIFEST.json"),
            ],
            cwd=policy_checkout,
            env=generation_env,
            capture_output=True,
            text=True,
        )
        assert generation.returncode == 0, generation.stdout + generation.stderr
        generated_manifest = json.loads(
            (asset_dir / "BUILD-MANIFEST.json").read_text(encoding="utf-8")
        )
        assert generated_manifest["release_version"] == version
        assert (
            generated_manifest["components"]["snapshot_access"]["source"]
            == "one-time-bootstrap-universal-build"
        )
        result = subprocess.run(
            [
                str(policy_checkout / "scripts/macos/verify-release-assets.sh"),
                "--mode",
                "release",
                "--asset-dir",
                str(asset_dir),
                "--expected-source-commit",
                "1" * 40,
                "--expected-packaging-commit",
                "2" * 40,
                "--skip-bundle-checks",
            ],
            cwd=policy_checkout,
            capture_output=True,
            text=True,
        )
        assert result.returncode == 0, result.stdout + result.stderr
    finally:
        subprocess.run(
            ["git", "-C", str(policy_repo), "worktree", "remove", "--force", str(policy_checkout)],
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
PY

python3 - "$root_dir" <<'PY'
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

root = Path(sys.argv[1])
verifier = root / ".github/scripts/verify-macos-rc-acceptance.py"
identity = {
    "sha256": "",
    "artifact_sha256": "",
    "cdhash": "1111111111111111111111111111111111111111",
    "designated_requirement": "designated => identifier \"com.ivan.televybackup.snapshot-access\" and (cdhash H\"1111111111111111111111111111111111111111\" or cdhash H\"2222222222222222222222222222222222222222\")",
    "bundle_id": "com.ivan.televybackup.snapshot-access",
    "relative_path": "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
    "binary": "Contents/MacOS/televybackup-snapshot-access",
    "component_version": "0.2.0",
    "protocol_version": 2,
    "reuse_policy": "byte-identical-no-rebuild-no-lipo-no-resign",
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
    fake_bin = temp / "bin"
    fake_bin.mkdir()
    hdiutil_path = fake_bin / "hdiutil"
    hdiutil_path.write_text(
        """#!/bin/sh
set -eu
if [ "$1" = attach ]; then
  shift
  mount_point=""
  source=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      -mountpoint) mount_point="$2"; shift 2 ;;
      *) source="$1"; shift ;;
    esac
  done
  mkdir -p "$mount_point"
  cp -R "${source}.tree/TelevyBackup.app" "$mount_point/"
  chmod 600 "$mount_point/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/Info.plist"
elif [ "$1" = detach ]; then
  rm -rf "$2/TelevyBackup.app"
else
  exit 2
fi
""",
        encoding="utf-8",
    )
    hdiutil_path.chmod(0o755)
    lipo_path = fake_bin / "lipo"
    lipo_path.write_text(
        """#!/bin/sh
set -eu
if [ "$1" = -info ]; then
  echo "Architectures in the fat file: $2 are: arm64 x86_64"
else
  exit 2
fi
""",
        encoding="utf-8",
    )
    lipo_path.chmod(0o755)
    codesign_path = fake_bin / "codesign"
    codesign_path.write_text(
        """#!/bin/sh
set -eu
if [ "$1" = -dvvv ]; then
  echo 'Signature=adhoc' >&2
  echo 'CDHash=2222222222222222222222222222222222222222' >&2
elif [ "$1" = -d ] && [ "$2" = -r- ]; then
  echo 'designated => identifier "com.ivan.televybackup.snapshot-access" and (cdhash H"1111111111111111111111111111111111111111" or cdhash H"2222222222222222222222222222222222222222")' >&2
else
  exit 2
fi
""",
        encoding="utf-8",
    )
    codesign_path.chmod(0o755)

    def artifact_sha256(path):
        digest = hashlib.sha256()
        for root_path, directories, files in os.walk(path, followlinks=False):
            directories.sort()
            files.sort()
            relative_root = os.path.relpath(root_path, path)
            if relative_root == ".":
                relative_root = ""
            for entry_name in directories + files:
                entry = Path(root_path) / entry_name
                relative = os.path.join(relative_root, entry_name)
                stat = os.lstat(entry)
                digest.update(b"entry\0" + relative.encode() + b"\0")
                digest.update(str(stat.st_mode).encode() + b"\0")
                if entry.is_file():
                    digest.update(b"file\0" + entry.read_bytes())
                else:
                    digest.update(b"other\0")
        return digest.hexdigest()

    helper_binary_bytes = b"#!/bin/sh\nprintf '%s\\n' '{\"bundleId\":\"com.ivan.televybackup.snapshot-access\",\"relativePath\":\"Contents/Library/LoginItems/TelevyBackup Snapshot Access.app\",\"componentVersion\":\"0.2.0\",\"protocolVersion\":2}'\n"
    helper_paths = []
    for number in (1, 2):
        tree = temp / f"TelevyBackup-1.0.0-rc.{number}.dmg.tree"
        helper = tree / "TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"
        binary = helper / "Contents/MacOS/televybackup-snapshot-access"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(helper_binary_bytes)
        binary.chmod(0o755)
        main_binary = tree / "TelevyBackup.app/Contents/MacOS/TelevyBackup"
        main_binary.parent.mkdir(parents=True, exist_ok=True)
        main_binary.write_bytes(b"universal-main-fixture")
        (helper / "Contents/Info.plist").write_text("fixture", encoding="utf-8")
        helper_paths.append(helper)
    identity["sha256"] = hashlib.sha256(
        (helper_paths[0] / "Contents/MacOS/televybackup-snapshot-access").read_bytes()
    ).hexdigest()
    identity["artifact_sha256"] = artifact_sha256(helper_paths[0])
    evidence["snapshot_access"] = identity.copy()
    evidence["root_mount_helper"]["rc1"] = identity.copy()
    evidence["root_mount_helper"]["rc2"] = identity.copy()
    root_identity = {
        field: identity[field]
        for field in ("sha256", "artifact_sha256", "cdhash", "designated_requirement")
    }

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
                **root_identity,
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
    old_path = os.environ.get("PATH", "")
    os.environ["PATH"] = f"{fake_bin}{os.pathsep}{old_path}"
    common = [
        sys.executable, str(verifier), "--evidence", json.dumps(evidence),
        "--manifest", str(stable_path), "--stable-version", "1.0.0",
        "--rc1-tag", "v1.0.0-rc.1", "--rc2-tag", "v1.0.0-rc.2",
        "--stable-source-commit", "stable-source", *rc_args,
    ]
    result = subprocess.run(common, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    evidence["root_mount_helper"]["rc2"]["cdhash"] = "2222222222222222222222222222222222222222"
    common[3] = json.dumps(evidence)
    result = subprocess.run(common, capture_output=True, text=True)
    assert result.returncode != 0, result.stdout + result.stderr
    evidence["root_mount_helper"]["rc2"] = identity.copy()
    common[3] = json.dumps(evidence)
    info_path = helper_paths[0] / "Contents/Info.plist"
    original_mode = info_path.stat().st_mode & 0o777
    info_path.chmod(0o600)
    legacy_artifact = artifact_sha256(helper_paths[0])
    info_path.chmod(original_mode)
    stable_manifest["components"]["snapshot_access"]["artifact_sha256"] = legacy_artifact
    evidence["snapshot_access"]["artifact_sha256"] = legacy_artifact
    stable_path.write_text(json.dumps(stable_manifest), encoding="utf-8")
    common[3] = json.dumps(evidence)
    result = subprocess.run(common, capture_output=True, text=True)
    assert result.returncode == 0, result.stdout + result.stderr
    lipo_path.write_text(
        """#!/bin/sh
set -eu
echo "Non-fat file: $2 is architecture: arm64"
""",
        encoding="utf-8",
    )
    lipo_path.chmod(0o755)
    assert subprocess.run(common, capture_output=True, text=True).returncode != 0
    lipo_path.write_text(
        """#!/bin/sh
set -eu
if [ "$1" = -info ]; then
  echo "Architectures in the fat file: $2 are: arm64 x86_64"
else
  exit 2
fi
""",
        encoding="utf-8",
    )
    lipo_path.chmod(0o755)
    tampered_binary = helper_paths[1] / "Contents/MacOS/televybackup-snapshot-access"
    tampered_binary.write_bytes(helper_binary_bytes + b"tampered")
    assert subprocess.run(common, capture_output=True, text=True).returncode != 0
    tampered_binary.write_bytes(helper_binary_bytes)
    stale = temp / "rc2.json"
    stale.write_text(stale.read_text(encoding="utf-8").replace(identity["sha256"], "stale-sha"), encoding="utf-8")
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
