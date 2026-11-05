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
        return module.IDENTITY_METADATA_TREE_SHA

    def create_ref(self, ref, sha):
        if ref in self.refs:
            raise module.ReservationError("already exists")
        self.refs[ref] = sha
        return sha


module.GitHubRefClient = FakeGitHubRefClient
claim_key = "pr:7:target:1.0.0-beta.1"
identity = module.deterministic_identity("fixture", claim_key)
expected = module.expected_reservation(
    source_sha=source,
    version="1.0.0-beta.1",
    channel="beta",
    release_target="1.0.0-beta.1",
    policy=module.RELEASE_POLICY_ID,
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
Release-Target: 1.0.0-beta.1
Release-Policy: {module.RELEASE_POLICY_ID}
Release-Channel: beta
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
squash_merge = "8" * 40
FakeGitHubRefClient.commits[squash_merge] = {
    "sha": squash_merge,
    "tree": {"sha": tree},
    "parents": [{"sha": source}],
    "message": "squash merge pull request #7\n",
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
module.verify_github_merge_identity(
    client=FakeGitHubRefClient("fixture/repo", "token", "https://fixture.invalid"),
    merge_sha=squash_merge, source_sha=source, version="1.0.0-beta.1", channel="beta",
    reservation=expected, pr_head_sha=preparation,
)
try:
    module.verify_github_merge_identity(
        client=FakeGitHubRefClient("fixture/repo", "token", "https://fixture.invalid"),
        merge_sha=squash_merge, source_sha=source, version="1.0.0-beta.1", channel="beta",
        reservation=expected,
    )
except module.ReservationError:
    pass
else:
    raise AssertionError("squash merge identity was accepted without a PR head")

# A retry may reuse a reservation created from an older source while the
# VERSION preparation commit is based on a newer descendant. The product
# source and reservation source are distinct identities and must both verify.
retry_reservation_source = "9" * 40
retry_preparation = "a" * 40
retry_merge = "b" * 40
retry_claim = "pr:8:target:1.0.0-beta.2"
retry_identity = module.deterministic_identity("fixture", retry_claim)
retry_ref = module.reservation_ref("1.0.0-beta.2")
retry_reservation = module.expected_reservation(
    source_sha=retry_reservation_source,
    version="1.0.0-beta.2",
    channel="beta",
    release_target="1.0.0-beta.2",
    policy=module.RELEASE_POLICY_ID,
    owner="fixture",
    claim_key=retry_claim,
    reservation_id=retry_identity["reservationId"],
    boundary_token=retry_identity["boundaryToken"],
)
retry_reservation["ref"] = retry_ref
FakeGitHubRefClient.commits[retry_preparation] = {
    "sha": retry_preparation,
    "tree": {"sha": tree},
    "parents": [{"sha": source}],
    "message": f"""chore(release): v1.0.0-beta.2

Release-Source-SHA: {source}
Release-Reservation-Source-SHA: {retry_reservation_source}
Product-Version: 1.0.0-beta.2
Release-Target: 1.0.0-beta.2
Release-Policy: {module.RELEASE_POLICY_ID}
Release-Channel: beta
Release-Reservation-Id: {retry_identity["reservationId"]}
Release-Reservation-Ref: {retry_ref}
Release-Reservation-Owner: fixture
Release-Claim-Key: {retry_claim}
Release-Boundary-Token: {retry_identity["boundaryToken"]}
""",
}
FakeGitHubRefClient.commits[retry_merge] = {
    "sha": retry_merge,
    "tree": {"sha": tree},
    "parents": [{"sha": source}, {"sha": retry_preparation}],
    "message": "Merge pull request #8\n",
}
module.verify_github_merge_identity(
    client=FakeGitHubRefClient("fixture/repo", "token", "https://fixture.invalid"),
    merge_sha=retry_merge,
    source_sha=retry_reservation_source,
    version="1.0.0-beta.2",
    channel="beta",
    reservation=retry_reservation,
)

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

legacy_source = "9" * 40
legacy_reservation_sha = "a" * 40
legacy_decision_sha = "b" * 40
legacy_merge = "c" * 40
legacy_released_sha = "d" * 40
reclaimed_reservation_sha = "e" * 40
retirement_sha = "f" * 40
legacy_claim = "pr:125:target:0.9.9"
legacy_identity = module.deterministic_identity("github-actions[bot]", legacy_claim)


class RetirementFakeGitHubRefClient(FakeGitHubRefClient):
    repository = "fixture/repo"
    refs = {
        module.reservation_ref("0.9.9"): legacy_reservation_sha,
        module.decision_ref("0.9.9"): legacy_decision_sha,
        module.receipt_ref("released", "0.9.9", legacy_identity["reservationId"]): legacy_released_sha,
    }
    commits = {
        legacy_source: {"sha": legacy_source, "tree": {"sha": tree}, "parents": [], "message": "legacy source\n"},
        legacy_reservation_sha: {
            "sha": legacy_reservation_sha, "tree": {"sha": tree}, "parents": [{"sha": legacy_source}],
            "message": f"""release: reserve product identity

Reservation-Id: {legacy_identity["reservationId"]}
Reservation-Owner: github-actions[bot]
Reservation-Claim-Key: {legacy_claim}
Reservation-Boundary-Token: {legacy_identity["boundaryToken"]}
Release-Version: 0.9.9
Release-Channel: prod
Reservation-State: claimed
""",
        },
        legacy_decision_sha: {
            "sha": legacy_decision_sha, "tree": {"sha": tree}, "parents": [{"sha": legacy_reservation_sha}],
            "message": f"""release: immutable identity decision

Decision-State: released
Release-Version: 0.9.9
Release-Merge-SHA: {legacy_merge}
Release-Reservation-Id: {legacy_identity["reservationId"]}
Release-Owner: github-actions[bot]
Release-Claim-Key: {legacy_claim}
Release-Boundary-Token: {legacy_identity["boundaryToken"]}
Release-Reservation-Ref: {module.reservation_ref("0.9.9")}
Decision-Provenance: immutable-decision
""",
        },
        legacy_merge: {"sha": legacy_merge, "tree": {"sha": tree}, "parents": [{"sha": legacy_source}], "message": "legacy preparation\n"},
        legacy_released_sha: {
            "sha": legacy_released_sha, "tree": {"sha": tree}, "parents": [{"sha": legacy_merge}],
            "message": f"""release: immutable identity receipt

Receipt-State: released
Release-Version: 0.9.9
Release-Merge-SHA: {legacy_merge}
Release-Reservation-Id: {legacy_identity["reservationId"]}
Release-Owner: github-actions[bot]
Release-Claim-Key: {legacy_claim}
Release-Boundary-Token: {legacy_identity["boundaryToken"]}
Release-Reservation-Ref: {module.reservation_ref("0.9.9")}
Receipt-Provenance: immutable-receipt
""",
        },
    }
    next_commits = iter((retirement_sha, reclaimed_reservation_sha))

    def request_json(self, method, path, payload=None):
        if method == "GET" and (
            path.endswith("/git/ref/tags/v0.9.9")
            or path.endswith("/releases/tags/v0.9.9")
            or "/git/matching-refs/tags/" in path
        ):
            return (404 if "/git/matching-refs/" not in path else 200), ([] if "/git/matching-refs/" in path else {})
        raise AssertionError((method, path, payload))


module.GitHubRefClient = RetirementFakeGitHubRefClient
retirement_evidence = {
    "schema_version": 1,
    "version": "0.9.9",
    "main_sha": legacy_source,
    "product_tag": {"ref": "v0.9.9", "status": "absent"},
    "github_release": {"tag": "v0.9.9", "status": "absent"},
    "consumed_refs": [],
    "artifact_refs": [],
    "release_runs": [{"id": "35336297939", "status": "completed", "artifacts": []}],
}
retired = module.create_github_retirement(
    version="0.9.9", reservation_id=legacy_identity["reservationId"], evidence=retirement_evidence,
    repository="fixture/repo", token="token", api_root="https://fixture.invalid",
)
assert retired["ref"] == module.retirement_ref("0.9.9", legacy_identity["reservationId"])
assert RetirementFakeGitHubRefClient.refs[module.reservation_ref("0.9.9")] == legacy_reservation_sha
assert RetirementFakeGitHubRefClient.refs[module.decision_ref("0.9.9")] == legacy_decision_sha

later_evidence = dict(retirement_evidence, main_sha="8" * 40)
rechecked = module.create_github_retirement(
    version="0.9.9", reservation_id=legacy_identity["reservationId"], evidence=later_evidence,
    repository="fixture/repo", token="token", api_root="https://fixture.invalid",
)
assert rechecked["target"] == retired["target"]

reclaimed_claim = "pr:204:target:0.9.9"
reclaimed_identity = module.deterministic_identity("github-actions[bot]", reclaimed_claim)
reclaimed = module.expected_reservation(
    source_sha=legacy_source, version="0.9.9", channel="prod", release_target="0.9.9",
    policy=module.RELEASE_POLICY_ID, owner="github-actions[bot]", claim_key=reclaimed_claim,
    reservation_id=reclaimed_identity["reservationId"], boundary_token=reclaimed_identity["boundaryToken"],
)
reclaimed["ref"] = module.reclaimed_reservation_ref("0.9.9", reclaimed_identity["reservationId"])
created_reclaimed = module.create_github_reservation(
    reclaimed, repository="fixture/repo", token="token", api_root="https://fixture.invalid",
)
assert created_reclaimed["ref"] == reclaimed["ref"]
assert module.decision_ref("0.9.9", reclaimed["ref"]).endswith(
    f"release-decision-reclaimed/v0.9.9/{reclaimed_identity['reservationId']}"
)

print("release GitHub API mock tests passed")
PY
