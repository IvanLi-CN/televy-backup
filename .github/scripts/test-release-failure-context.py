#!/usr/bin/env python3
"""Exercise the embedded failure-context resolver with a deterministic GitHub API mock."""

from __future__ import annotations

import json
import os
import tempfile
import textwrap
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from io import BytesIO
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/notify-release-failure.yml"
SOURCE = "a" * 40
PREPARATION = "b" * 40
MERGE = "c" * 40
RESERVATION_COMMIT = "d" * 40
BOUND_COMMIT = "e" * 40
SOURCE_TREE = "1" * 40
MERGE_TREE = "2" * 40
TAG_OBJECT = "3" * 40


class Response:
    def __init__(self, value: bytes):
        self.value = value

    def __enter__(self) -> "Response":
        return self

    def __exit__(self, *_args: object) -> None:
        return None

    def read(self) -> bytes:
        return self.value


def json_response(value: object) -> Response:
    return Response(json.dumps(value).encode("utf-8"))


def intent(declared_pull_request: str = "7", release_type: str = "type:patch") -> dict[str, object]:
    return {
        "schema_version": 1,
        "pull_request": declared_pull_request,
        "source_sha": SOURCE,
        "preparation_commit_sha": PREPARATION,
        "merge_sha": MERGE,
        "merge_commit_sha": MERGE,
        "head_sha": PREPARATION,
        "branch_head": MERGE,
        "release_mode": "normal",
        "covered_merge_sha": "",
        "labels": [release_type, "channel:rc"],
        "type": release_type,
        "channel": "rc",
        "components": ["component:app"],
        "version": "1.2.3-rc.1",
        "tag": "v1.2.3-rc.1",
        "tag_target_sha": MERGE,
        "tag_owner": "protected-release-automation",
        "reservation": {
            "id": "res-1",
            "ref": "refs/tags/release-reservation/v1.2.3-rc.1",
            "owner": "github-actions[bot]",
            "claim_key": "pr:7:source:a:type:patch:channel:rc",
            "boundary_token": "bnd-1",
            "state": "bound",
            "merge_commit_sha": MERGE,
        },
        "provenance": {
            "signature": "github-native-verified",
            "signature_verified": True,
            "provenance_verified": True,
            "tag_owner": "protected-release-automation",
            "identity": "release_chain_and_reservation_refs",
        },
        "artifact_names": ["release-package-arm64", "release-package-x86_64", "release-assets"],
        "run_url": "https://github.example/run/7",
        "recovery_instruction": f"workflow_dispatch operation=recover commit_sha={MERGE} helper_mode=bootstrap",
    }


def commit_payload(sha: str) -> dict[str, object]:
    if sha == SOURCE:
        parents: list[str] = []
        tree = SOURCE_TREE
        message = "source\n"
        verification = {"verified": False}
    elif sha == PREPARATION:
        parents = [SOURCE]
        tree = "4" * 40
        message = f"""chore(release): v1.2.3-rc.1

Release-Source-SHA: {SOURCE}
Product-Version: 1.2.3-rc.1
Release-Intent-Type: type:patch
Release-Intent-Channel: channel:rc
Release-Mode: normal
Release-Reservation-Id: res-1
Release-Reservation-Ref: refs/tags/release-reservation/v1.2.3-rc.1
Release-Reservation-Owner: github-actions[bot]
Release-Claim-Key: pr:7:source:a:type:patch:channel:rc
Release-Boundary-Token: bnd-1
Release-Provenance: github-native-verified
"""
        verification = {"verified": True}
    elif sha == MERGE:
        parents = ["f" * 40, PREPARATION]
        tree = MERGE_TREE
        message = "Merge pull request #7\n"
        verification = {"verified": True}
    elif sha == "f" * 40:
        parents = []
        tree = MERGE_TREE
        message = "mainline\n"
        verification = {"verified": True}
    elif sha == RESERVATION_COMMIT:
        parents = [SOURCE]
        tree = SOURCE_TREE
        message = """release: reserve product identity

Reservation-Id: res-1
Reservation-Owner: github-actions[bot]
Reservation-Claim-Key: pr:7:source:a:type:patch:channel:rc
Reservation-Boundary-Token: bnd-1
Release-Version: 1.2.3-rc.1
Release-Channel: rc
Reservation-State: claimed
"""
        verification = {"verified": False}
    elif sha == BOUND_COMMIT:
        parents = [MERGE]
        tree = MERGE_TREE
        message = f"""release: immutable identity receipt

Receipt-State: bound
Release-Version: 1.2.3-rc.1
Release-Merge-SHA: {MERGE}
Release-Reservation-Id: res-1
Release-Owner: github-actions[bot]
Release-Claim-Key: pr:7:source:a:type:patch:channel:rc
Release-Boundary-Token: bnd-1
Release-Reservation-Ref: refs/tags/release-reservation/v1.2.3-rc.1
Receipt-Provenance: immutable-receipt
"""
        verification = {"verified": False}
    else:
        raise AssertionError(f"unexpected commit {sha}")
    return {
        "commit": {
            "tree": {"sha": tree},
            "message": message,
            "verification": verification,
        },
        "parents": [{"sha": parent} for parent in parents],
    }


class GitHubMock:
    def __init__(self, tag_mode: str, declared_pull_request: str, artifact_available: bool, release_type: str):
        self.tag_mode = tag_mode
        self.declared_pull_request = declared_pull_request
        self.artifact_available = artifact_available
        archive = BytesIO()
        with zipfile.ZipFile(archive, "w") as bundle:
            bundle.writestr("release-intent.json", json.dumps(intent(declared_pull_request, release_type)))
        self.archive = archive.getvalue()

    def __call__(self, req: urllib.request.Request) -> Response:
        parsed = urllib.parse.urlparse(req.full_url)
        path = parsed.path
        if path.endswith("/actions/runs/7/artifacts"):
            artifacts = []
            if self.artifact_available:
                artifacts = [{"name": "release-intent", "expired": False, "archive_download_url": "https://fixture/archive"}]
            return json_response({"artifacts": artifacts})
        if path == "/archive":
            return Response(self.archive)
        if path.endswith("/actions/runs/7/jobs/1") or path.endswith("/actions/runs/7/jobs"):
            return json_response({"jobs": [{"id": 1}]})
        if path.endswith("/actions/jobs/1/logs"):
            return Response(b"RELEASE_IDENTITY status=resolved version=1.2.3-rc.1 channel=rc merge_sha=" + MERGE.encode() + b" tag=v1.2.3-rc.1")
        if path.endswith(f"/commits/{MERGE}/pulls"):
            return json_response([{"number": 7, "merge_commit_sha": MERGE}])
        if path.endswith("/pulls/7"):
            return json_response({"number": 7, "merge_commit_sha": MERGE, "head": {"sha": PREPARATION}})
        if "/commits/" in path:
            return json_response(commit_payload(path.rsplit("/", 1)[-1]))
        if path.endswith("release-reservation/v1.2.3-rc.1"):
            return json_response({"object": {"sha": RESERVATION_COMMIT, "type": "commit"}})
        if path.endswith("release-bound/v1.2.3-rc.1/" + MERGE):
            return json_response({"object": {"sha": BOUND_COMMIT, "type": "commit"}})
        if path.endswith("/git/ref/tags/v1.2.3-rc.1"):
            if self.tag_mode == "missing":
                raise urllib.error.HTTPError(req.full_url, 404, "missing", {}, BytesIO())
            return json_response({"object": {"sha": TAG_OBJECT, "type": "tag"}})
        if path.endswith(f"/git/tags/{TAG_OBJECT}"):
            return json_response({"object": {"sha": MERGE, "type": "commit"}})
        raise AssertionError(f"unexpected API request: {path}")


def run_case(tag_mode: str, declared_pull_request: str, artifact_available: bool = True, release_type: str = "type:patch") -> str:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    source = workflow.split("          python3 - <<'PYCODE'\n", 1)[1].split("\n          PYCODE", 1)[0]
    output = tempfile.NamedTemporaryFile(delete=False)
    output.close()
    env = {
        "GH_TOKEN": "fixture",
        "REPOSITORY": "fixture/repo",
        "RUN_ID": "7",
        "RUN_ATTEMPT": "1",
        "HEAD_BRANCH": "main",
        "HEAD_SHA": MERGE,
        "TRIGGERING_ACTOR": "fixture",
        "GITHUB_API_URL": "https://api.fixture",
        "GITHUB_OUTPUT": output.name,
    }
    old_env = os.environ.copy()
    os.environ.update(env)
    try:
        from unittest.mock import patch

        with patch("urllib.request.urlopen", side_effect=GitHubMock(tag_mode, declared_pull_request, artifact_available, release_type)):
            exec(compile(textwrap.dedent(source), str(WORKFLOW), "exec"), {"__name__": "__main__"})
        return Path(output.name).read_text(encoding="utf-8")
    finally:
        os.environ.clear()
        os.environ.update(old_env)
        Path(output.name).unlink(missing_ok=True)


missing = run_case("missing", "7")
assert "identity_status=resolved" in missing
assert "tag_status=not-created" in missing
reconstructed = run_case("missing", "7", artifact_available=False)
assert "identity_status=resolved" in reconstructed
assert "artifact_names=reconstructed-from-immutable-refs" in reconstructed
annotated = run_case("annotated", "7")
assert "identity_status=resolved" in annotated
assert "tag_status=present" in annotated
mismatched_pr = run_case("missing", "99")
assert "identity_status=resolver-error" in mismatched_pr
assert "merge_sha=\n" in mismatched_pr
invalid_type = run_case("missing", "7", release_type="type:invalid")
assert "identity_status=resolver-error" in invalid_type
print("release failure resolver mock tests passed")
