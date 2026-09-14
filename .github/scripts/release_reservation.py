#!/usr/bin/env python3
"""Create and verify append-only release reservation and receipt refs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any
from urllib import error, request

ROOT = Path(__file__).resolve().parents[2]
SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")
REF_RE = re.compile(r"^refs/tags/[A-Za-z0-9._/-]+$")
VERSION_RE = re.compile(r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:beta|rc|dev)\.[1-9]\d*)?$")
CHANNELS = {"prod", "beta", "rc", "dev"}
IDENTITY_SEGMENT_RE = re.compile(r"^[A-Za-z0-9._-]+$")


class ReservationError(RuntimeError):
    """Raised when an immutable release claim cannot be proven."""


def normalize_sha(value: str, label: str = "SHA") -> str:
    if not SHA_RE.fullmatch(value):
        raise ReservationError(f"invalid {label}: {value!r}")
    return value.lower()


def validate_version(version: str) -> None:
    if not VERSION_RE.fullmatch(version):
        raise ReservationError(f"invalid release version: {version!r}")


def validate_identity(version: str, channel: str) -> None:
    validate_version(version)
    if channel not in CHANNELS:
        raise ReservationError(f"invalid release channel: {channel!r}")
    version_channel = version.rsplit("-", 1)[-1].split(".", 1)[0] if "-" in version else "prod"
    if version_channel != channel:
        raise ReservationError(f"release version {version!r} does not match channel {channel!r}")


def reservation_ref(version: str) -> str:
    validate_version(version)
    return f"refs/tags/release-reservation/v{version}"


def receipt_ref(state: str, version: str, identity: str) -> str:
    if state not in {"bound", "consumed", "released"}:
        raise ReservationError(f"unsupported receipt state: {state}")
    validate_version(version)
    if not IDENTITY_SEGMENT_RE.fullmatch(identity):
        raise ReservationError(f"invalid receipt identity: {identity!r}")
    if state == "released":
        return f"refs/tags/release-released/v{version}/{identity}"
    normalize_sha(identity, "merge SHA")
    return f"refs/tags/release-{state}/v{version}/{identity}"


def decision_ref(version: str) -> str:
    validate_version(version)
    return f"refs/tags/release-decision/v{version}"


def deterministic_identity(owner: str, claim_key: str) -> dict[str, str]:
    if not owner.strip() or not claim_key.strip():
        raise ReservationError("reservation owner and claim key are required")
    digest = hashlib.sha256(f"{owner}\0{claim_key}".encode("utf-8")).hexdigest()
    claim_digest = hashlib.sha256(claim_key.encode("utf-8")).hexdigest()
    return {
        "reservationId": f"res-{digest[:24]}",
        "boundaryToken": f"bnd-{claim_digest[:32]}",
    }


def reservation_metadata(
    *, reservation_id: str, owner: str, claim_key: str, boundary_token: str, version: str, channel: str, state: str
) -> dict[str, str]:
    if state != "claimed":
        raise ReservationError("new reservations must start in claimed state")
    return {
        "Reservation-Id": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
        "Release-Version": version,
        "Release-Channel": channel,
        "Reservation-State": state,
    }


def trailers_from_message(message: str) -> dict[str, str]:
    result: dict[str, str] = {}
    for line in message.splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            if key.strip() and value.strip():
                result[key.strip()] = value.strip()
    return result


def _git(*args: str, cwd: Path = ROOT, check: bool = True) -> str:
    result = subprocess.run(["git", *args], cwd=cwd, text=True, capture_output=True)
    if check and result.returncode != 0:
        raise ReservationError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout.strip()


def _git_raw(*args: str, cwd: Path = ROOT) -> str:
    result = subprocess.run(["git", *args], cwd=cwd, text=True, capture_output=True)
    if result.returncode != 0:
        raise ReservationError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout


def local_commit_info(commit: str, cwd: Path = ROOT) -> dict[str, Any]:
    normalized = _git("rev-parse", f"{commit}^{{commit}}", cwd=cwd)
    raw = _git_raw("cat-file", "-p", normalized, cwd=cwd)
    lines = raw.splitlines()
    tree = next((line.split(" ", 1)[1] for line in lines if line.startswith("tree ")), "")
    parents = [line.split(" ", 1)[1] for line in lines if line.startswith("parent ")]
    message = raw.split("\n\n", 1)[1] if "\n\n" in raw else ""
    return {"sha": normalized, "tree": tree, "parents": parents, "message": message}


def expected_reservation(
    *, source_sha: str, version: str, channel: str, owner: str, claim_key: str, reservation_id: str, boundary_token: str
) -> dict[str, Any]:
    source = normalize_sha(source_sha, "source SHA")
    validate_identity(version, channel)
    metadata = reservation_metadata(
        reservation_id=reservation_id,
        owner=owner,
        claim_key=claim_key,
        boundary_token=boundary_token,
        version=version,
        channel=channel,
        state="claimed",
    )
    return {
        "sourceSha": source,
        "version": version,
        "channel": channel,
        "reservationId": reservation_id,
        **metadata,
    }


def validate_expected_reservation(expected: dict[str, Any]) -> None:
    version = str(expected["version"])
    channel = str(expected["channel"])
    validate_identity(version, channel)
    if str(expected["ref"]) != reservation_ref(version):
        raise ReservationError("reservation ref does not match the release version")
    normalize_sha(str(expected["sourceSha"]), "source SHA")
    for key in (
        "reservationId",
        "Reservation-Owner",
        "Reservation-Claim-Key",
        "Reservation-Boundary-Token",
    ):
        if not str(expected.get(key, "")).strip():
            raise ReservationError(f"reservation is missing {key}")


def verify_reservation_commit(ref_target: str, expected: dict[str, Any], cwd: Path = ROOT) -> dict[str, Any]:
    validate_expected_reservation(expected)
    info = local_commit_info(ref_target, cwd)
    source = normalize_sha(str(expected["sourceSha"]), "source SHA")
    if [normalize_sha(parent, "reservation parent") for parent in info["parents"]] != [source]:
        raise ReservationError("reservation must have source SHA as its only parent")
    source_info = local_commit_info(source, cwd)
    if info["tree"] != source_info["tree"]:
        raise ReservationError("reservation tree must equal the source tree")
    trailers = trailers_from_message(info["message"])
    expected_trailers = {
        "Reservation-Id": expected["reservationId"],
        "Reservation-Owner": expected["Reservation-Owner"],
        "Reservation-Claim-Key": expected["Reservation-Claim-Key"],
        "Reservation-Boundary-Token": expected["Reservation-Boundary-Token"],
        "Release-Version": expected["version"],
        "Release-Channel": expected["channel"],
        "Reservation-State": "claimed",
    }
    for key, value in expected_trailers.items():
        if trailers.get(key) != value:
            raise ReservationError(f"reservation trailer {key} does not match")
    return {"ref": expected["ref"], "target": info["sha"], **expected}


def _reservation_message(metadata: dict[str, str]) -> str:
    lines = ["release: reserve product identity", ""]
    lines.extend(f"{key}: {value}" for key, value in metadata.items())
    return "\n".join(lines) + "\n"


def local_ref_target(ref: str, cwd: Path = ROOT) -> str | None:
    if not REF_RE.fullmatch(ref):
        raise ReservationError(f"invalid repository ref: {ref}")
    result = subprocess.run(["git", "show-ref", "--verify", "--quiet", ref], cwd=cwd)
    if result.returncode != 0:
        return None
    return _git("rev-parse", f"{ref}^{{commit}}", cwd=cwd)


def create_local_reservation(expected: dict[str, Any], cwd: Path = ROOT) -> dict[str, Any]:
    validate_expected_reservation(expected)
    ref = str(expected["ref"])
    existing = local_ref_target(ref, cwd)
    if existing:
        return verify_reservation_commit(existing, expected, cwd)
    source_info = local_commit_info(str(expected["sourceSha"]), cwd)
    metadata = reservation_metadata(
        reservation_id=str(expected["reservationId"]),
        owner=str(expected["Reservation-Owner"]),
        claim_key=str(expected["Reservation-Claim-Key"]),
        boundary_token=str(expected["Reservation-Boundary-Token"]),
        version=str(expected["version"]),
        channel=str(expected["channel"]),
        state="claimed",
    )
    commit = _git(
        "commit-tree", source_info["tree"], "-p", source_info["sha"], "-m", _reservation_message(metadata), cwd=cwd
    )
    update = subprocess.run(
        ["git", "update-ref", ref, commit, "0" * 40], cwd=cwd, text=True, capture_output=True
    )
    if update.returncode != 0:
        existing = local_ref_target(ref, cwd)
        if existing:
            return verify_reservation_commit(existing, expected, cwd)
        raise ReservationError(update.stderr.strip() or "reservation ref creation failed")
    return verify_reservation_commit(commit, expected, cwd)


class GitHubRefClient:
    """Small GitHub Git Database client used only for append-only ref creation."""

    def __init__(self, repository: str, token: str, api_root: str = "https://api.github.com") -> None:
        self.repository = repository
        self.token = token
        self.api_root = api_root.rstrip("/")

    def request_json(self, method: str, path: str, payload: dict[str, Any] | None = None) -> tuple[int, dict[str, Any]]:
        url = path if path.startswith("http") else f"{self.api_root}{path}"
        body = json.dumps(payload).encode("utf-8") if payload is not None else None
        req = request.Request(
            url,
            data=body,
            method=method,
            headers={
                "Authorization": f"Bearer {self.token}",
                "Accept": "application/vnd.github+json",
                "Content-Type": "application/json",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "televy-backup-release-reservation",
            },
        )
        try:
            with request.urlopen(req) as response:
                raw = response.read().decode("utf-8")
                return response.status, json.loads(raw) if raw else {}
        except error.HTTPError as exc:
            raw = exc.read().decode("utf-8", errors="replace")
            try:
                payload_value = json.loads(raw)
            except json.JSONDecodeError:
                payload_value = {"message": raw}
            if exc.code == 404:
                return 404, payload_value
            raise ReservationError(f"GitHub API {method} {path} returned {exc.code}: {payload_value}") from exc

    def ref_target(self, ref: str) -> str | None:
        status, payload = self.request_json("GET", f"/repos/{self.repository}/git/ref/{ref.removeprefix('refs/')}")
        if status == 404:
            return None
        value = payload.get("object", {}).get("sha")
        return normalize_sha(value, "remote ref target") if isinstance(value, str) else None

    def commit_info(self, sha: str) -> dict[str, Any]:
        status, payload = self.request_json("GET", f"/repos/{self.repository}/git/commits/{normalize_sha(sha)}")
        if status != 200:
            raise ReservationError("GitHub commit provenance is unavailable")
        return payload

    def create_commit(self, *, tree: str, parent: str, message: str) -> str:
        status, payload = self.request_json(
            "POST",
            f"/repos/{self.repository}/git/commits",
            {"message": message, "tree": tree, "parents": [parent]},
        )
        if status not in {200, 201} or not isinstance(payload.get("sha"), str):
            raise ReservationError("GitHub did not return a reservation commit")
        return normalize_sha(payload["sha"], "reservation commit")

    def create_ref(self, ref: str, sha: str) -> str:
        status, payload = self.request_json(
            "POST", f"/repos/{self.repository}/git/refs", {"ref": ref, "sha": normalize_sha(sha)}
        )
        if status not in {200, 201}:
            raise ReservationError(f"GitHub did not create append-only ref {ref}")
        value = payload.get("object", {}).get("sha", sha)
        return normalize_sha(value, "created ref target")


def verify_github_decision(
    *, client: GitHubRefClient, fields: dict[str, str], reservation: dict[str, Any]
) -> dict[str, Any]:
    ref = decision_ref(str(reservation["version"]))
    target = client.ref_target(ref)
    if not target:
        raise ReservationError(f"decision ref is missing: {ref}")
    info = client.commit_info(target)
    reservation_info = client.commit_info(str(reservation["target"]))
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    if parents != [reservation_info.get("sha")] or info.get("tree", {}).get("sha") != reservation_info.get("tree", {}).get("sha"):
        raise ReservationError("remote decision provenance does not match the reservation")
    actual = trailers_from_message(str(info.get("message", "")))
    if any(actual.get(key) != value for key, value in fields.items()):
        raise ReservationError("remote decision does not match the requested identity")
    return {"ref": ref, "target": target, **fields}


def create_github_decision(
    *, state: str, merge_sha: str, reservation: dict[str, Any], client: GitHubRefClient
) -> dict[str, str]:
    fields = decision_fields(
        state=state, version=str(reservation["version"]), merge_sha=merge_sha, reservation=reservation
    )
    ref = decision_ref(str(reservation["version"]))
    existing = client.ref_target(ref)
    if existing:
        return {"ref": ref, **verify_github_decision(client=client, fields=fields, reservation=reservation)}
    reservation_info = client.commit_info(str(reservation["target"]))
    tree = reservation_info.get("tree", {}).get("sha")
    if not isinstance(tree, str):
        raise ReservationError("reservation tree provenance is unavailable")
    commit = client.create_commit(tree=tree, parent=str(reservation["target"]), message=decision_message(fields))
    try:
        target = client.create_ref(ref, commit)
    except ReservationError:
        target = client.ref_target(ref)
        if not target:
            raise
    if target != commit:
        return {"ref": ref, **verify_github_decision(client=client, fields=fields, reservation=reservation)}
    return {"ref": ref, **verify_github_decision(client=client, fields=fields, reservation=reservation)}


def create_github_reservation(
    expected: dict[str, Any], *, repository: str, token: str, api_root: str = "https://api.github.com"
) -> dict[str, Any]:
    validate_expected_reservation(expected)
    client = GitHubRefClient(repository, token, api_root)
    ref = str(expected["ref"])
    existing = client.ref_target(ref)
    if existing:
        info = client.commit_info(existing)
        parents = [parent.get("sha") for parent in info.get("parents", [])]
        tree = info.get("tree", {}).get("sha")
        source_info = client.commit_info(str(expected["sourceSha"]))
        if parents != [expected["sourceSha"]] or tree != source_info.get("tree", {}).get("sha"):
            raise ReservationError("existing reservation provenance does not match the claim")
        trailers = trailers_from_message(str(info.get("message", "")))
        for key, value in {
            "Reservation-Id": expected["reservationId"],
            "Reservation-Owner": expected["Reservation-Owner"],
            "Reservation-Claim-Key": expected["Reservation-Claim-Key"],
            "Reservation-Boundary-Token": expected["Reservation-Boundary-Token"],
            "Release-Version": expected["version"],
            "Release-Channel": expected["channel"],
            "Reservation-State": "claimed",
        }.items():
            if trailers.get(key) != value:
                raise ReservationError(f"existing reservation trailer {key} does not match")
        return {"ref": ref, "target": existing, **expected}
    source_info = client.commit_info(str(expected["sourceSha"]))
    tree = source_info.get("tree", {}).get("sha")
    if not isinstance(tree, str):
        raise ReservationError("source tree provenance is unavailable")
    metadata = reservation_metadata(
        reservation_id=str(expected["reservationId"]),
        owner=str(expected["Reservation-Owner"]),
        claim_key=str(expected["Reservation-Claim-Key"]),
        boundary_token=str(expected["Reservation-Boundary-Token"]),
        version=str(expected["version"]),
        channel=str(expected["channel"]),
        state="claimed",
    )
    commit = client.create_commit(tree=tree, parent=str(expected["sourceSha"]), message=_reservation_message(metadata))
    try:
        target = client.create_ref(ref, commit)
    except ReservationError:
        target = client.ref_target(ref)
        if not target:
            raise
    if target != commit:
        # Another creator won the race; only an exactly matching claim may continue.
        return create_github_reservation(expected, repository=repository, token=token, api_root=api_root)
    return {"ref": ref, "target": target, **expected}


def verify_github_reservation(expected: dict[str, Any], *, repository: str, token: str, api_root: str = "https://api.github.com") -> dict[str, Any]:
    validate_expected_reservation(expected)
    client = GitHubRefClient(repository, token, api_root)
    ref = str(expected["ref"])
    target = client.ref_target(ref)
    if not target:
        raise ReservationError(f"reservation ref is missing: {ref}")
    info = client.commit_info(target)
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    source = normalize_sha(str(expected["sourceSha"]), "source SHA")
    source_info = client.commit_info(source)
    if parents != [source] or info.get("tree", {}).get("sha") != source_info.get("tree", {}).get("sha"):
        raise ReservationError("remote reservation provenance does not match the claim")
    trailers = trailers_from_message(str(info.get("message", "")))
    for key, value in {
        "Reservation-Id": expected["reservationId"],
        "Reservation-Owner": expected["Reservation-Owner"],
        "Reservation-Claim-Key": expected["Reservation-Claim-Key"],
        "Reservation-Boundary-Token": expected["Reservation-Boundary-Token"],
        "Release-Version": expected["version"],
        "Release-Channel": expected["channel"],
        "Reservation-State": "claimed",
    }.items():
        if trailers.get(key) != value:
            raise ReservationError(f"remote reservation trailer {key} does not match")
    return {"ref": ref, "target": target, **expected}


def verify_local_reservation_claim(
    *, reservation_ref_value: str, version: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, cwd: Path = ROOT
) -> dict[str, Any]:
    validate_version(version)
    if reservation_ref_value != reservation_ref(version):
        raise ReservationError("receipt reservation ref does not match the release version")
    target = local_ref_target(reservation_ref_value, cwd)
    if not target:
        raise ReservationError(f"reservation ref is missing: {reservation_ref_value}")
    info = local_commit_info(target, cwd)
    if len(info["parents"]) != 1:
        raise ReservationError("reservation must have one source parent")
    trailers = trailers_from_message(info["message"])
    channel = trailers.get("Release-Channel", "")
    expected = {
        "ref": reservation_ref_value,
        "sourceSha": info["parents"][0],
        "version": version,
        "channel": channel,
        "reservationId": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
    }
    return verify_reservation_commit(target, expected, cwd)


def verify_github_reservation_claim(
    *, reservation_ref_value: str, version: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, repository: str, token: str, api_root: str = "https://api.github.com"
) -> dict[str, Any]:
    client = GitHubRefClient(repository, token, api_root)
    validate_version(version)
    if reservation_ref_value != reservation_ref(version):
        raise ReservationError("receipt reservation ref does not match the release version")
    target = client.ref_target(reservation_ref_value)
    if not target:
        raise ReservationError(f"reservation ref is missing: {reservation_ref_value}")
    info = client.commit_info(target)
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    if len(parents) != 1 or not isinstance(parents[0], str):
        raise ReservationError("remote reservation must have one source parent")
    source = normalize_sha(parents[0], "reservation parent")
    source_info = client.commit_info(source)
    if info.get("tree", {}).get("sha") != source_info.get("tree", {}).get("sha"):
        raise ReservationError("remote reservation tree does not match its source")
    trailers = trailers_from_message(str(info.get("message", "")))
    channel = trailers.get("Release-Channel", "")
    expected = {
        "ref": reservation_ref_value,
        "sourceSha": source,
        "version": version,
        "channel": channel,
        "reservationId": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
    }
    validate_identity(version, channel)
    for key, value in {
        "Reservation-Id": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
        "Release-Version": version,
        "Release-Channel": channel,
        "Reservation-State": "claimed",
    }.items():
        if trailers.get(key) != value:
            raise ReservationError(f"remote reservation trailer {key} does not match")
    return {"ref": reservation_ref_value, "target": target, **expected}


def receipt_message(fields: dict[str, str]) -> str:
    lines = ["release: immutable identity receipt", ""]
    lines.extend(f"{key}: {value}" for key, value in fields.items())
    return "\n".join(lines) + "\n"


def decision_fields(*, state: str, version: str, merge_sha: str, reservation: dict[str, Any]) -> dict[str, str]:
    if state not in {"bound", "released"}:
        raise ReservationError(f"unsupported decision state: {state}")
    return {
        "Decision-State": state,
        "Release-Version": version,
        "Release-Merge-SHA": normalize_sha(merge_sha, "decision merge SHA"),
        "Release-Reservation-Id": reservation["reservationId"],
        "Release-Owner": reservation["Reservation-Owner"],
        "Release-Claim-Key": reservation["Reservation-Claim-Key"],
        "Release-Boundary-Token": reservation["Reservation-Boundary-Token"],
        "Release-Reservation-Ref": reservation["ref"],
        "Decision-Provenance": "immutable-decision",
    }


def decision_message(fields: dict[str, str]) -> str:
    lines = ["release: immutable identity decision", ""]
    lines.extend(f"{key}: {value}" for key, value in fields.items())
    return "\n".join(lines) + "\n"


def verify_local_decision_state(
    *, state: str, merge_sha: str, reservation: dict[str, Any], cwd: Path = ROOT
) -> dict[str, Any]:
    fields = decision_fields(
        state=state, version=str(reservation["version"]), merge_sha=merge_sha, reservation=reservation
    )
    ref = decision_ref(str(reservation["version"]))
    target = local_ref_target(ref, cwd)
    if not target:
        raise ReservationError(f"decision ref is missing: {ref}")
    return verify_decision_commit(target, fields, reservation, cwd)


def verify_github_decision_state(
    *, state: str, merge_sha: str, reservation: dict[str, Any], client: GitHubRefClient
) -> dict[str, Any]:
    fields = decision_fields(
        state=state, version=str(reservation["version"]), merge_sha=merge_sha, reservation=reservation
    )
    return verify_github_decision(client=client, fields=fields, reservation=reservation)


def verify_decision_commit(
    ref_target: str, fields: dict[str, str], reservation: dict[str, Any], cwd: Path = ROOT
) -> dict[str, Any]:
    info = local_commit_info(ref_target, cwd)
    reservation_info = local_commit_info(str(reservation["target"]), cwd)
    if info["parents"] != [reservation_info["sha"]] or info["tree"] != reservation_info["tree"]:
        raise ReservationError("decision provenance does not match the reservation")
    actual = trailers_from_message(info["message"])
    if any(actual.get(key) != value for key, value in fields.items()):
        raise ReservationError("existing decision does not match the requested identity")
    return {"target": info["sha"], **fields}


def create_local_decision(
    *, state: str, merge_sha: str, reservation: dict[str, Any], cwd: Path = ROOT
) -> dict[str, str]:
    fields = decision_fields(
        state=state, version=str(reservation["version"]), merge_sha=merge_sha, reservation=reservation
    )
    ref = decision_ref(str(reservation["version"]))
    existing = local_ref_target(ref, cwd)
    if existing:
        return {"ref": ref, **verify_decision_commit(existing, fields, reservation, cwd)}
    reservation_info = local_commit_info(str(reservation["target"]), cwd)
    commit = _git(
        "commit-tree", reservation_info["tree"], "-p", reservation_info["sha"],
        "-m", decision_message(fields), cwd=cwd
    )
    update = subprocess.run(
        ["git", "update-ref", ref, commit, "0" * 40], cwd=cwd, text=True, capture_output=True
    )
    if update.returncode != 0:
        existing = local_ref_target(ref, cwd)
        if existing:
            return {"ref": ref, **verify_decision_commit(existing, fields, reservation, cwd)}
        raise ReservationError(update.stderr.strip() or "decision ref creation failed")
    return {"ref": ref, **verify_decision_commit(commit, fields, reservation, cwd)}


def receipt_fields(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str
) -> dict[str, str]:
    if state not in {"bound", "consumed", "released"}:
        raise ReservationError(f"unsupported receipt state: {state}")
    validate_version(version)
    return {
        "Receipt-State": state,
        "Release-Version": version,
        "Release-Merge-SHA": normalize_sha(merge_sha, "merge SHA"),
        "Release-Reservation-Id": reservation_id,
        "Release-Owner": owner,
        "Release-Claim-Key": claim_key,
        "Release-Boundary-Token": boundary_token,
        "Release-Reservation-Ref": reservation_ref_value,
        "Receipt-Provenance": "immutable-receipt",
    }


def verify_receipt_commit(ref_target: str, fields: dict[str, str], cwd: Path = ROOT) -> dict[str, Any]:
    info = local_commit_info(ref_target, cwd)
    merge_sha = normalize_sha(fields["Release-Merge-SHA"], "merge SHA")
    merge_info = local_commit_info(merge_sha, cwd)
    if info["parents"] != [merge_sha] or info["tree"] != merge_info["tree"]:
        raise ReservationError("receipt provenance does not match its merge SHA")
    actual = trailers_from_message(info["message"])
    if any(actual.get(key) != value for key, value in fields.items()):
        raise ReservationError("existing receipt does not match the requested identity")
    return {"target": info["sha"], **fields}


def verify_local_merge_identity(
    *, merge_sha: str, source_sha: str, version: str, channel: str,
    reservation: dict[str, Any], cwd: Path = ROOT
) -> None:
    merge = local_commit_info(merge_sha, cwd)
    if len(merge["parents"]) != 2:
        raise ReservationError("bound receipt merge SHA must be a two-parent merge commit")
    expected = {
        "Release-Source-SHA": source_sha,
        "Product-Version": version,
        "Release-Intent-Channel": f"channel:{channel}",
        "Release-Reservation-Id": reservation["reservationId"],
        "Release-Reservation-Ref": reservation["ref"],
        "Release-Reservation-Owner": reservation["Reservation-Owner"],
        "Release-Claim-Key": reservation["Reservation-Claim-Key"],
        "Release-Boundary-Token": reservation["Reservation-Boundary-Token"],
    }
    for preparation_sha in merge["parents"]:
        preparation = local_commit_info(preparation_sha, cwd)
        if preparation["parents"] != [source_sha] or preparation["tree"] != merge["tree"]:
            continue
        values = trailers_from_message(preparation["message"])
        if all(values.get(key) == value for key, value in expected.items()):
            return
    raise ReservationError("merge SHA does not contain the matching prepared release identity")


def verify_github_merge_identity(
    *, client: GitHubRefClient, merge_sha: str, source_sha: str, version: str, channel: str,
    reservation: dict[str, Any]
) -> None:
    merge = client.commit_info(merge_sha)
    parents = [parent.get("sha") for parent in merge.get("parents", [])]
    if len(parents) != 2 or any(not isinstance(parent, str) for parent in parents):
        raise ReservationError("bound receipt merge SHA must be a two-parent merge commit")
    expected = {
        "Release-Source-SHA": source_sha,
        "Product-Version": version,
        "Release-Intent-Channel": f"channel:{channel}",
        "Release-Reservation-Id": reservation["reservationId"],
        "Release-Reservation-Ref": reservation["ref"],
        "Release-Reservation-Owner": reservation["Reservation-Owner"],
        "Release-Claim-Key": reservation["Reservation-Claim-Key"],
        "Release-Boundary-Token": reservation["Reservation-Boundary-Token"],
    }
    merge_tree = merge.get("tree", {}).get("sha")
    for preparation_sha in parents:
        preparation = client.commit_info(preparation_sha)
        preparation_parents = [parent.get("sha") for parent in preparation.get("parents", [])]
        if preparation_parents != [source_sha] or preparation.get("tree", {}).get("sha") != merge_tree:
            continue
        values = trailers_from_message(str(preparation.get("message", "")))
        if all(values.get(key) == value for key, value in expected.items()):
            return
    raise ReservationError("merge SHA does not contain the matching prepared release identity")


def create_local_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, cwd: Path = ROOT
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
    )
    reservation = verify_local_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, cwd=cwd,
    )
    bound_ref = receipt_ref("bound", version, merge)
    consumed_ref = receipt_ref("consumed", version, merge)
    bound_target = local_ref_target(bound_ref, cwd)
    consumed_target = local_ref_target(consumed_ref, cwd)
    released_target = local_ref_target(receipt_ref("released", version, reservation_id), cwd)
    if state in {"bound", "consumed"} and released_target:
        raise ReservationError("release claim is already marked released")
    if state == "bound" and consumed_target:
        raise ReservationError("consumed receipt exists before bound receipt")
    if state == "consumed":
        if not bound_target:
            raise ReservationError("consumed receipt requires a matching bound receipt")
        verify_receipt_commit(bound_target, {**fields, "Receipt-State": "bound"}, cwd)
        verify_local_decision_state(state="bound", merge_sha=merge, reservation=reservation, cwd=cwd)
    if state == "released" and (bound_target or consumed_target):
        raise ReservationError("released receipt is only valid for an unbound claim")
    if state == "bound":
        verify_local_merge_identity(
            merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation, cwd=cwd,
        )
        create_local_decision(state="bound", merge_sha=merge, reservation=reservation, cwd=cwd)
    elif state == "consumed":
        verify_local_merge_identity(
            merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation, cwd=cwd,
        )
    elif state == "released":
        create_local_decision(state="released", merge_sha=merge, reservation=reservation, cwd=cwd)
    existing = local_ref_target(ref, cwd)
    if existing:
        return {"ref": ref, **verify_receipt_commit(existing, fields, cwd)}
    merge_info = local_commit_info(merge, cwd)
    commit = _git("commit-tree", merge_info["tree"], "-p", merge, "-m", receipt_message(fields), cwd=cwd)
    update = subprocess.run(["git", "update-ref", ref, commit, "0" * 40], cwd=cwd, text=True, capture_output=True)
    if update.returncode != 0:
        existing = local_ref_target(ref, cwd)
        if existing:
            return create_local_receipt(
                state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
                claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value, cwd=cwd
            )
        raise ReservationError(update.stderr.strip() or "receipt ref creation failed")
    return {"ref": ref, "target": commit, **fields}


def create_github_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, repository: str, token: str,
    api_root: str = "https://api.github.com"
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
    )
    client = GitHubRefClient(repository, token, api_root)
    reservation = verify_github_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, repository=repository,
        token=token, api_root=api_root,
    )
    bound_ref = receipt_ref("bound", version, merge)
    consumed_ref = receipt_ref("consumed", version, merge)
    bound_target = client.ref_target(bound_ref)
    consumed_target = client.ref_target(consumed_ref)
    released_target = client.ref_target(receipt_ref("released", version, reservation_id))
    if state in {"bound", "consumed"} and released_target:
        raise ReservationError("release claim is already marked released")
    if state == "bound" and consumed_target:
        raise ReservationError("consumed receipt exists before bound receipt")
    if state == "consumed":
        if not bound_target:
            raise ReservationError("consumed receipt requires a matching bound receipt")
        verify_github_receipt(
            state="bound", version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
            claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
            repository=repository, token=token, api_root=api_root,
        )
        verify_github_decision_state(state="bound", merge_sha=merge, reservation=reservation, client=client)
    if state == "released" and (bound_target or consumed_target):
        raise ReservationError("released receipt is only valid for an unbound claim")
    if state == "bound":
        verify_github_merge_identity(
            client=client, merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation,
        )
        create_github_decision(state="bound", merge_sha=merge, reservation=reservation, client=client)
    elif state == "consumed":
        verify_github_merge_identity(
            client=client, merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation,
        )
    elif state == "released":
        create_github_decision(state="released", merge_sha=merge, reservation=reservation, client=client)
    existing = client.ref_target(ref)
    if existing:
        info = client.commit_info(existing)
        merge_info = client.commit_info(merge)
        parents = [parent.get("sha") for parent in info.get("parents", [])]
        if parents != [merge] or info.get("tree", {}).get("sha") != merge_info.get("tree", {}).get("sha"):
            raise ReservationError("existing remote receipt provenance does not match its merge SHA")
        actual = trailers_from_message(str(info.get("message", "")))
        if any(actual.get(key) != value for key, value in fields.items()):
            raise ReservationError("existing remote receipt does not match the requested identity")
        return {"ref": ref, "target": existing, **fields}
    merge_info = client.commit_info(merge)
    tree = merge_info.get("tree", {}).get("sha")
    if not isinstance(tree, str):
        raise ReservationError("merge tree provenance is unavailable")
    commit = client.create_commit(tree=tree, parent=merge, message=receipt_message(fields))
    try:
        target = client.create_ref(ref, commit)
    except ReservationError:
        target = client.ref_target(ref)
        if not target:
            raise
    if target != commit:
        return create_github_receipt(
            state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
            claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
            repository=repository, token=token, api_root=api_root
        )
    return {"ref": ref, "target": target, **fields}


def verify_local_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, cwd: Path = ROOT
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
    )
    reservation = verify_local_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, cwd=cwd,
    )
    target = local_ref_target(ref, cwd)
    if not target:
        raise ReservationError(f"receipt ref is missing: {ref}")
    bound_ref = receipt_ref("bound", version, merge)
    consumed_ref = receipt_ref("consumed", version, merge)
    bound_target = local_ref_target(bound_ref, cwd)
    consumed_target = local_ref_target(consumed_ref, cwd)
    if state in {"bound", "consumed"}:
        verify_local_merge_identity(
            merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation, cwd=cwd,
        )
    if state == "bound":
        verify_local_decision_state(state="bound", merge_sha=merge, reservation=reservation, cwd=cwd)
    elif state == "consumed":
        if not bound_target:
            raise ReservationError("consumed receipt requires a matching bound receipt")
        verify_receipt_commit(bound_target, {**fields, "Receipt-State": "bound"}, cwd)
        verify_local_decision_state(state="bound", merge_sha=merge, reservation=reservation, cwd=cwd)
    elif state == "released":
        if bound_target or consumed_target:
            raise ReservationError("released receipt is only valid for an unbound claim")
        verify_local_decision_state(state="released", merge_sha=merge, reservation=reservation, cwd=cwd)
    return {"ref": ref, **verify_receipt_commit(target, fields, cwd)}


def verify_github_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, repository: str, token: str,
    api_root: str = "https://api.github.com"
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
    )
    reservation = verify_github_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, repository=repository,
        token=token, api_root=api_root,
    )
    client = GitHubRefClient(repository, token, api_root)
    target = client.ref_target(ref)
    if not target:
        raise ReservationError(f"receipt ref is missing: {ref}")
    bound_ref = receipt_ref("bound", version, merge)
    consumed_ref = receipt_ref("consumed", version, merge)
    bound_target = client.ref_target(bound_ref)
    consumed_target = client.ref_target(consumed_ref)
    if state in {"bound", "consumed"}:
        verify_github_merge_identity(
            client=client, merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation,
        )
    if state == "bound":
        verify_github_decision_state(state="bound", merge_sha=merge, reservation=reservation, client=client)
    elif state == "consumed":
        if not bound_target:
            raise ReservationError("consumed receipt requires a matching bound receipt")
        verify_github_receipt(
            state="bound", version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
            claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
            repository=repository, token=token, api_root=api_root,
        )
        verify_github_decision_state(state="bound", merge_sha=merge, reservation=reservation, client=client)
    elif state == "released":
        if bound_target or consumed_target:
            raise ReservationError("released receipt is only valid for an unbound claim")
        verify_github_decision_state(state="released", merge_sha=merge, reservation=reservation, client=client)
    info = client.commit_info(target)
    merge_info = client.commit_info(merge)
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    if parents != [merge] or info.get("tree", {}).get("sha") != merge_info.get("tree", {}).get("sha"):
        raise ReservationError("remote receipt provenance does not match its merge SHA")
    actual = trailers_from_message(str(info.get("message", "")))
    if any(actual.get(key) != value for key, value in fields.items()):
        raise ReservationError("remote receipt does not match the requested identity")
    return {"ref": ref, "target": target, **fields}


def intent_snapshot(**values: Any) -> dict[str, Any]:
    required = {"version", "channel", "merge_sha", "source_sha", "release_mode", "reservation"}
    missing = sorted(required - values.keys())
    if missing:
        raise ReservationError(f"release intent is missing fields: {', '.join(missing)}")
    normalize_sha(str(values["source_sha"]), "source SHA")
    normalize_sha(str(values["merge_sha"]), "merge SHA")
    version = str(values["version"])
    snapshot = {
        "schema_version": 1,
        "pull_request": values.get("pull_request"),
        "source_sha": values["source_sha"],
        "merge_sha": values["merge_sha"],
        "release_mode": values["release_mode"],
        "covered_merge_sha": values.get("covered_merge_sha", ""),
        "type": values.get("type", ""),
        "channel": values["channel"],
        "version": version,
        "tag": f"v{version}",
        "reservation": values["reservation"],
        "provenance": values.get("provenance", {}),
        "artifact_names": values.get("artifact_names", []),
        "run_url": values.get("run_url", ""),
        "recovery_instruction": values.get(
            "recovery_instruction",
            f"workflow_dispatch operation=recover commit_sha={values['merge_sha']} "
            "precondition=verify-bound-identity-and-same-sha",
        ),
    }
    return snapshot


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    reserve = sub.add_parser("reserve")
    reserve.add_argument("--source-sha", required=True)
    reserve.add_argument("--version", required=True)
    reserve.add_argument("--channel", required=True)
    reserve.add_argument("--owner", required=True)
    reserve.add_argument("--claim-key", required=True)
    reserve.add_argument("--reservation-id")
    reserve.add_argument("--boundary-token")
    reserve.add_argument("--repository")
    reserve.add_argument("--token")
    reserve.add_argument("--api-root", default="https://api.github.com")
    reserve.add_argument("--local-root", type=Path)
    reserve.add_argument("--output", type=Path)
    receipt = sub.add_parser("receipt")
    for option in ("state", "version", "merge-sha", "reservation-id", "owner", "claim-key", "boundary-token", "reservation-ref"):
        receipt.add_argument(f"--{option}", required=True)
    receipt.add_argument("--maintainer-confirmed", action="store_true")
    receipt.add_argument("--local-root", type=Path)
    receipt.add_argument("--repository")
    receipt.add_argument("--token")
    receipt.add_argument("--api-root", default="https://api.github.com")
    receipt.add_argument("--output", type=Path)
    snapshot = sub.add_parser("snapshot")
    snapshot.add_argument("--input", type=Path, required=True)
    snapshot.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "reserve":
            generated = deterministic_identity(args.owner, args.claim_key)
            expected = {
                "ref": reservation_ref(args.version),
                "sourceSha": normalize_sha(args.source_sha, "source SHA"),
                "version": args.version,
                "channel": args.channel,
                "reservationId": args.reservation_id or generated["reservationId"],
                "Reservation-Owner": args.owner,
                "Reservation-Claim-Key": args.claim_key,
                "Reservation-Boundary-Token": args.boundary_token or generated["boundaryToken"],
            }
            if args.repository and args.token:
                result = create_github_reservation(expected, repository=args.repository, token=args.token, api_root=args.api_root)
            else:
                result = create_local_reservation(expected, args.local_root or ROOT)
        elif args.command == "receipt":
            if args.state == "released" and not args.maintainer_confirmed:
                raise ReservationError("released receipt requires explicit maintainer confirmation")
            if args.repository and args.token:
                result = create_github_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha, reservation_id=args.reservation_id,
                    owner=args.owner, claim_key=args.claim_key, boundary_token=args.boundary_token,
                    reservation_ref_value=args.reservation_ref, repository=args.repository, token=args.token,
                    api_root=args.api_root,
                )
            else:
                result = create_local_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha, reservation_id=args.reservation_id,
                    owner=args.owner, claim_key=args.claim_key, boundary_token=args.boundary_token,
                    reservation_ref_value=args.reservation_ref, cwd=args.local_root or ROOT
                )
        else:
            value = json.loads(args.input.read_text(encoding="utf-8"))
            write_json(args.output, intent_snapshot(**value))
            result = intent_snapshot(**value)
        if getattr(args, "output", None) and args.command != "snapshot":
            write_json(args.output, result)
        print(json.dumps(result, sort_keys=True))
        return 0
    except (ReservationError, OSError, json.JSONDecodeError) as exc:
        print(f"release_reservation.py: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
