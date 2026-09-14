#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

python3 - "$root_dir" <<'PY'
import importlib.util
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("release_helper", root / ".github/scripts/release_helper.py")
assert spec and spec.loader
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)

lock = json.loads((root / "packaging/macos/snapshot-components.lock.json").read_text(encoding="utf-8"))
version = "0.9.9-rc.38"

# A merged product identity may have a reservation and bound receipt while no
# helper Release exists. Its product identity must remain unchanged.
reservation_only_identity = {
    "merge_sha": "d" * 40,
    "version": version,
    "reservation_ref": "refs/tags/release-reservation/v0.9.9-rc.38",
    "release_state": "missing",
}
resolved = helper.resolve_helper(version=reservation_only_identity["version"], requested_mode="bootstrap")
assert resolved == {
    "version": version,
    "mode": "bootstrap",
    "source_tag": "",
    "reason": "no-approved-helper-release",
}
try:
    helper.resolve_helper(version=version, requested_mode="auto")
except helper.HelperResolutionError as error:
    assert "helper_mode=bootstrap" in str(error)
else:
    raise AssertionError("automatic bootstrap was accepted without an explicit recovery mode")

releases = [
    {"tagName": "v0.9.7-rc.2", "isDraft": False, "isPrerelease": True, "publishedAt": "2026-09-08T00:00:00Z"},
    {"tagName": "v0.9.8", "isDraft": False, "isPrerelease": False, "publishedAt": "2026-09-11T00:00:00Z"},
]
candidates = helper.candidate_tags(version, lock["bootstrap_release_tag"], releases)
assert candidates[:2] == ["v0.9.9-rc.1", "v0.9.8-rc.1"]
assert "v0.9.7-rc.2" in candidates

identity = {
    "sha256": "a" * 64,
    "artifact_sha256": "b" * 64,
    "cdhash": "c" * 40,
    "designated_requirement": '# designated => identifier "com.ivan.televybackup.snapshot-access"',
}
manifest = {
    "product": "TelevyBackup",
    "signing": "ad-hoc",
    "release_version": "0.9.7-rc.2",
    "components": {"snapshot_access": {
        "bundle_id": lock["components"]["snapshot_access"]["bundle_id"],
        "relative_path": lock["components"]["snapshot_access"]["relative_path"],
        "binary": lock["components"]["snapshot_access"]["binary"],
        "component_version": lock["components"]["snapshot_access"]["component_version"],
        "protocol_version": lock["components"]["snapshot_access"]["protocol_version"],
        "reuse_policy": lock["components"]["snapshot_access"]["reuse_policy"],
        **identity,
    }},
}
assert helper.validate_source_manifest(manifest, lock, "v0.9.7-rc.2") == identity
manifest["components"]["snapshot_access"]["cdhash"] = "invalid"
try:
    helper.validate_source_manifest(manifest, lock, "v0.9.7-rc.2")
except helper.HelperResolutionError:
    pass
else:
    raise AssertionError("invalid helper CodeDirectory identity was accepted")
print("release helper resolver fixture tests passed")
PY
