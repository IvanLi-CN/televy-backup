#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
python3 - "$root_dir" <<'PY'
import importlib.util
import sys
from pathlib import Path

root = Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("release_reservation", root / ".github/scripts/release_reservation.py")
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

source = "1" * 40
fake_reservation = "2" * 40
fake_bound = "3" * 40
fake_consumed = "4" * 40
fake_decision = "7" * 40
preparation = "5" * 40
merge = "6" * 40
tree = "a" * 40


class FakeGitHubRefClient:
    refs = {}
    commits = {
        source: {"sha": source, "tree": {"sha": tree}, "parents": [], "message": "source\n"},
    }
    next_commits = iter((fake_reservation, fake_decision, fake_bound, fake_consumed))

    def __init__(self, repository, token, api_root):
        assert repository == "fixture/repo"
        assert token == "token"
        assert api_root == "https://fixture.invalid"

    def ref_target(self, ref):
        return self.refs.get(ref)

    def commit_info(self, sha):
        return self.commits[sha]

    def create_commit(self, *, tree, parent, message):
        sha = next(self.next_commits)
        self.commits[sha] = {"sha": sha, "tree": {"sha": tree}, "parents": [{"sha": parent}], "message": message}
        return sha

    def create_tree(self):
        return module.EMPTY_TREE_SHA

    def create_ref(self, ref, sha):
        if ref in self.refs:
            raise module.ReservationError("already exists")
        self.refs[ref] = sha
        return sha


module.GitHubRefClient = FakeGitHubRefClient
claim_key = "pr:7:source:" + source
identity = module.deterministic_identity("fixture", claim_key)
expected = module.expected_reservation(
    source_sha=source,
    version="1.0.0-beta.1",
    channel="beta",
    owner="fixture",
    claim_key=claim_key,
    reservation_id=identity["reservationId"],
    boundary_token=identity["boundaryToken"],
)
expected["ref"] = module.reservation_ref("1.0.0-beta.1")
expected["reservationId"] = identity["reservationId"]
FakeGitHubRefClient.commits[preparation] = {
    "sha": preparation,
    "tree": {"sha": tree},
    "parents": [{"sha": source}],
    "message": f"""chore(release): v1.0.0-beta.1

Release-Source-SHA: {source}
Product-Version: 1.0.0-beta.1
Release-Intent-Type: type:patch
Release-Intent-Channel: channel:beta
Release-Reservation-Id: {identity["reservationId"]}
Release-Reservation-Ref: {expected["ref"]}
Release-Reservation-Owner: fixture
Release-Claim-Key: {claim_key}
Release-Boundary-Token: {identity["boundaryToken"]}
""",
}
FakeGitHubRefClient.commits[merge] = {
    "sha": merge,
    "tree": {"sha": tree},
    "parents": [{"sha": source}, {"sha": preparation}],
    "message": "Merge pull request #7\n",
}
created = module.create_github_reservation(expected, repository="fixture/repo", token="token", api_root="https://fixture.invalid")
assert created["target"] == fake_reservation
assert module.create_github_reservation(expected, repository="fixture/repo", token="token", api_root="https://fixture.invalid")["target"] == fake_reservation

class MissingSourceGitHubRefClient(FakeGitHubRefClient):
    def commit_info(self, sha):
        if sha == source:
            raise module.ReservationError(
                "GitHub API GET /repos/fixture/repo/git/commits/" + source
                + " returned 422: {'message': 'No commit found for SHA: " + source + "'}"
            )
        return super().commit_info(sha)

module.GitHubRefClient = MissingSourceGitHubRefClient
expected_with_tree = dict(expected, sourceTreeSha=tree)
verified = module.verify_github_reservation(
    expected_with_tree, repository="fixture/repo", token="token", api_root="https://fixture.invalid"
)
assert verified["target"] == fake_reservation

module.GitHubRefClient = FakeGitHubRefClient

bound = module.create_github_receipt(
    state="bound", version="1.0.0-beta.1", merge_sha=merge,
    reservation_id=identity["reservationId"], owner="fixture", claim_key=claim_key,
    boundary_token=identity["boundaryToken"], reservation_ref_value=expected["ref"],
    repository="fixture/repo", token="token", api_root="https://fixture.invalid",
)
consumed = module.create_github_receipt(
    state="consumed", version="1.0.0-beta.1", merge_sha=merge,
    reservation_id=identity["reservationId"], owner="fixture", claim_key=claim_key,
    boundary_token=identity["boundaryToken"], reservation_ref_value=expected["ref"],
    repository="fixture/repo", token="token", api_root="https://fixture.invalid",
)
assert bound["target"] == fake_bound
assert consumed["target"] == fake_consumed
module.verify_github_receipt(
    state="bound", version="1.0.0-beta.1", merge_sha=merge,
    reservation_id=identity["reservationId"], owner="fixture", claim_key=claim_key,
    boundary_token=identity["boundaryToken"], reservation_ref_value=expected["ref"],
    repository="fixture/repo", token="token", api_root="https://fixture.invalid",
)

def assert_consumed_rejected():
    try:
        module.verify_github_receipt(
            state="consumed", version="1.0.0-beta.1", merge_sha=merge,
            reservation_id=identity["reservationId"], owner="fixture", claim_key=claim_key,
            boundary_token=identity["boundaryToken"], reservation_ref_value=expected["ref"],
            repository="fixture/repo", token="token", api_root="https://fixture.invalid",
        )
    except module.ReservationError:
        return
    raise AssertionError("consumed receipt bypassed immutable state ordering")

bound_ref = module.receipt_ref("bound", "1.0.0-beta.1", merge)
saved_bound = FakeGitHubRefClient.refs.pop(bound_ref)
try:
    assert_consumed_rejected()
finally:
    FakeGitHubRefClient.refs[bound_ref] = saved_bound

decision_ref = module.decision_ref("1.0.0-beta.1")
saved_decision = FakeGitHubRefClient.refs.pop(decision_ref)
try:
    assert_consumed_rejected()
finally:
    FakeGitHubRefClient.refs[decision_ref] = saved_decision

print("release GitHub API mock tests passed")
PY
