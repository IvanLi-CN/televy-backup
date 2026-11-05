#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

python3 - "$root_dir" "$tmp_dir" <<'PY'
from pathlib import Path
import argparse
import importlib.util
import subprocess
import sys

root = Path(sys.argv[1])
tmp = Path(sys.argv[2])
repo = tmp / "repo"
repo.mkdir()
subprocess.run(["git", "init", "-q", str(repo)], check=True)
for key, value in (("user.name", "fixture"), ("user.email", "fixture@example.com")):
    subprocess.run(["git", "-C", str(repo), "config", key, value], check=True)
(repo / "VERSION").write_text("0.9.2\n", encoding="utf-8")
(repo / "README").write_text("fixture\n", encoding="utf-8")
subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
subprocess.run(["git", "-C", str(repo), "commit", "-qm", "fixture source"], check=True)
source = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()


def product_tag(name: str, target: str) -> None:
    subprocess.run(
        [
            "git", "-C", str(repo), "-c", "user.name=github-actions[bot]",
            "-c", "user.email=41898282+github-actions[bot]@users.noreply.github.com",
            "tag", "-a", "-m", name, name, target,
        ],
        check=True,
    )


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


chain = load("release_chain", root / ".github/scripts/release_chain.py")
reservation = load("release_reservation", root / ".github/scripts/release_reservation.py")
chain.ROOT = repo
product_tag("v0.9.3", source)

assert chain.parse_release_target("patch")["kind"] == "symbolic"
assert chain.parse_release_target("0.9.5-rc.1")["channel"] == "rc"
for invalid in (" patch", "v0.9.5", "type:patch", ""):
    try:
        chain.parse_release_target(invalid)
    except chain.ReleaseChainError:
        pass
    else:
        raise AssertionError(f"invalid release target accepted: {invalid!r}")

resolved = chain.resolve_release_target("patch", chain.product_tags())
assert resolved["baselineVersion"] == "0.9.3"
assert resolved["version"] == "0.9.4"
assert resolved["policy"] == chain.VERSION_POLICY_ID
explicit = chain.resolve_release_target("0.9.5-rc.1", chain.product_tags())
assert explicit["version"] == "0.9.5-rc.1"
try:
    chain.resolve_release_target("0.9.3", chain.product_tags())
except chain.ReleaseChainError as error:
    assert "above trusted final baseline" in str(error)
else:
    raise AssertionError("explicit target at the final baseline was accepted")

assert chain.manual_claim_key(7, "patch") == chain.manual_claim_key(7, "patch")
assert chain.manual_claim_key(7, "patch") != chain.manual_claim_key(8, "patch")
claim_key = chain.manual_claim_key(7, "patch")
identity = reservation.deterministic_identity("fixture", claim_key)
reservation_value = {
    "ref": reservation.reservation_ref(resolved["version"]),
    "sourceSha": source,
    "version": resolved["version"],
    "channel": "prod",
    "releaseTarget": "patch",
    "policy": chain.VERSION_POLICY_ID,
    "reservationId": identity["reservationId"],
    "Reservation-Owner": "fixture",
    "Reservation-Claim-Key": claim_key,
    "Reservation-Boundary-Token": identity["boundaryToken"],
}
assert reservation.create_local_reservation(reservation_value, repo)["ref"] == reservation_value["ref"]

chain.stage(argparse.Namespace(
    source_sha=source,
    version=resolved["version"],
    release_target="patch",
    release_policy=chain.VERSION_POLICY_ID,
    release_mode="normal",
    reservation_id=reservation_value["reservationId"],
    reservation_ref=reservation_value["ref"],
    reservation_owner=reservation_value["Reservation-Owner"],
    claim_key=reservation_value["Reservation-Claim-Key"],
    boundary_token=reservation_value["Reservation-Boundary-Token"],
    covered_merge_sha="",
    provenance="fixture-verified",
))
prepared_sha = chain.git("rev-parse", "HEAD")
prepared = chain.verify_prepared(prepared_sha, source, expected_target="patch")
assert prepared["version"] == "0.9.4"
assert prepared["releaseTarget"] == "patch"
assert prepared["policy"] == chain.VERSION_POLICY_ID
assert chain.trailers(prepared_sha)["Release-Channel"] == "prod"
assert chain.diff_names(prepared_sha) == ["VERSION"]

(repo / "POST_PREPARATION").write_text("source update\n", encoding="utf-8")
subprocess.run(["git", "-C", str(repo), "add", "POST_PREPARATION"], check=True)
subprocess.run(["git", "-C", str(repo), "commit", "-qm", "source update after preparation"], check=True)
post_preparation = chain.git("rev-parse", "HEAD")
found = chain.find_prepared(post_preparation, source)
assert found["preparationSha"] == prepared_sha
assert found["sourceSha"] == source

occupied = chain.occupied_identity_versions()
assert "0.9.4" in occupied
for target in ("patch", "0.9.4"):
    try:
        chain.resolve_release_target(target, chain.product_tags(), occupied)
    except chain.ReleaseChainError as error:
        assert "refusing to allocate a successor" in str(error)
    else:
        raise AssertionError(f"occupied target {target} was silently advanced")
bound = chain.resolve_release_target(
    "0.9.4", chain.product_tags(), occupied, allow_occupied_version="0.9.4"
)
assert bound["version"] == "0.9.4"

# A verified unpublished legacy claim releases only its old source refs. The
# next reservation must still use a new reclaimed identity path.
legacy_claim = "pr:125:target:0.9.5"
legacy_identity = reservation.deterministic_identity("github-actions[bot]", legacy_claim)
legacy_reservation = reservation.expected_reservation(
    source_sha=source,
    version="0.9.5",
    channel="prod",
    release_target="0.9.5",
    policy=chain.VERSION_POLICY_ID,
    owner="github-actions[bot]",
    claim_key=legacy_claim,
    reservation_id=legacy_identity["reservationId"],
    boundary_token=legacy_identity["boundaryToken"],
)
legacy_reservation["ref"] = reservation.reservation_ref("0.9.5")
reservation.create_local_reservation(legacy_reservation, repo)
reservation.create_local_receipt(
    state="released", version="0.9.5", merge_sha=source,
    reservation_id=legacy_identity["reservationId"], owner="github-actions[bot]",
    claim_key=legacy_claim, boundary_token=legacy_identity["boundaryToken"],
    reservation_ref_value=legacy_reservation["ref"], cwd=repo,
)
evidence = {
    "schema_version": 1,
    "version": "0.9.5",
    "main_sha": source,
    "product_tag": {"ref": "v0.9.5", "status": "absent"},
    "github_release": {"tag": "v0.9.5", "status": "absent"},
    "consumed_refs": [],
    "artifact_refs": [],
    "release_runs": [{"id": "1", "status": "completed", "artifacts": []}],
}
reservation.create_local_retirement(
    version="0.9.5", reservation_id=legacy_identity["reservationId"], evidence=evidence, cwd=repo,
)
retired_occupied = chain.occupied_identity_versions()
assert "0.9.5" not in retired_occupied
reclaimed = chain.resolve_release_target("0.9.5", chain.product_tags(), retired_occupied)
assert reclaimed["version"] == "0.9.5"
reclaimed_identity = reservation.deterministic_identity("github-actions[bot]", "pr:204:target:0.9.5")
active_reclaimed = dict(legacy_reservation)
active_reclaimed.update({
    "ref": reservation.reclaimed_reservation_ref("0.9.5", reclaimed_identity["reservationId"]),
    "reservationId": reclaimed_identity["reservationId"],
    "Reservation-Claim-Key": "pr:204:target:0.9.5",
    "Reservation-Boundary-Token": reclaimed_identity["boundaryToken"],
})
reservation.create_local_reservation(active_reclaimed, repo)
assert "0.9.5" in chain.occupied_identity_versions()

product_tag("v0.9.4", source)
assert chain.verify_release_sequence("0.9.4", source)["status"] == "matching"

# The merge verifier accepts a normal merge only when the merged PR head has
# the exact VERSION preparation tree and the full history is available.
topology = tmp / "topology"
topology.mkdir()
subprocess.run(["git", "init", "-q", str(topology)], check=True)
for key, value in (("user.name", "fixture"), ("user.email", "fixture@example.com")):
    subprocess.run(["git", "-C", str(topology), "config", key, value], check=True)
(topology / "VERSION").write_text("0.9.2\n", encoding="utf-8")
(topology / "README").write_text("topology\n", encoding="utf-8")
subprocess.run(["git", "-C", str(topology), "add", "."], check=True)
subprocess.run(["git", "-C", str(topology), "commit", "-qm", "topology base"], check=True)
base = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
subprocess.run(["git", "-C", str(topology), "switch", "-q", "-c", "source"], check=True)
(topology / "feature").write_text("feature\n", encoding="utf-8")
subprocess.run(["git", "-C", str(topology), "add", "feature"], check=True)
subprocess.run(["git", "-C", str(topology), "commit", "-qm", "feature"], check=True)
topology_source = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
(topology / "VERSION").write_text("0.9.3\n", encoding="utf-8")
subprocess.run(["git", "-C", str(topology), "add", "VERSION"], check=True)
message = f"""chore(release): v0.9.3

Release-Source-SHA: {topology_source}
Release-Target: patch
Release-Target-Kind: symbolic
Release-Policy: {chain.VERSION_POLICY_ID}
Product-Version: 0.9.3
Release-Channel: prod
Release-Mode: normal
Release-Reservation-Id: res-topology
Release-Reservation-Ref: refs/tags/release-reservation/v0.9.3
Release-Reservation-Owner: fixture
Release-Claim-Key: topology-claim
Release-Boundary-Token: topology-boundary
Release-Provenance: fixture-verified
"""
subprocess.run(["git", "-C", str(topology), "commit", "-qm", "chore(release): v0.9.3", "-m", message], check=True)
preparation = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
subprocess.run(["git", "-C", str(topology), "switch", "-q", "-c", "mainline", base], check=True)
subprocess.run(["git", "-C", str(topology), "merge", "--no-ff", "-m", "fixture product merge", preparation], check=True)
merge = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
chain.ROOT = topology
assert chain.verify_merged(merge)["prepared"] == "true"
assert chain.verify_merged(merge, "0" * 40)["reason"] == "pr_head_mismatch"

print("release chain fixture tests passed")
PY
