#!/usr/bin/env python3
"""Create and verify append-only release reservation and receipt refs."""

from __future__ import annotations

import argparse
import base64
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
RELEASE_TARGET_RE = re.compile(
    r"^(?:patch|minor|major|(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:beta|rc|dev)\.[1-9]\d*)?)$"
)
RELEASE_POLICY_ID = "televybackup-manual-semver-v1"
CHANNELS = {"prod", "beta", "rc", "dev"}
IDENTITY_SEGMENT_RE = re.compile(r"^[A-Za-z0-9._-]+$")
ARTIFACT_STATE_RE = re.compile(r"^(?:pending|attempt|finalized)$")
POSITIVE_INTEGER_RE = re.compile(r"^[1-9][0-9]*$")
ARTIFACT_DIGEST_HEX_RE = re.compile(r"^[0-9a-fA-F]{64}$")
ARTIFACT_NAME_RE = re.compile(r"^release-assets(?:-[1-9][0-9]*-[1-9][0-9]*)?$")
IDENTITY_METADATA_CONTENT = "televy-backup release identity\n"
IDENTITY_METADATA_TREE_SHA = "b1612a1f4eed800eeef62ae225399baf8d0ff6fb"


class ReservationError(RuntimeError):
    """Raised when an immutable release claim cannot be proven."""


class ArtifactReceiptMissing(ReservationError):
    """Raised when a durable artifact receipt does not exist yet."""


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


def validate_release_target(release_target: str, policy: str) -> None:
    if not RELEASE_TARGET_RE.fullmatch(release_target):
        raise ReservationError(f"invalid release target: {release_target!r}")
    if policy != RELEASE_POLICY_ID:
        raise ReservationError(f"unsupported release policy: {policy!r}")


def validate_target_identity(release_target: str, version: str, channel: str) -> None:
    """Ensure the durable target maps to the exact reserved version/channel."""
    validate_release_target(release_target, RELEASE_POLICY_ID)
    if release_target in {"patch", "minor", "major"}:
        if channel != "prod" or "-" in version:
            raise ReservationError("symbolic release targets must reserve a final product version")
    elif release_target != version:
        raise ReservationError("explicit release target must match the reserved version")


def reservation_ref(version: str) -> str:
    validate_version(version)
    return f"refs/tags/release-reservation/v{version}"


def reclaimed_reservation_ref(version: str, reservation_id: str) -> str:
    validate_version(version)
    if not IDENTITY_SEGMENT_RE.fullmatch(reservation_id):
        raise ReservationError(f"invalid reservation identity: {reservation_id!r}")
    return f"refs/tags/release-reservation-reclaimed/v{version}/{reservation_id}"


def validate_reservation_ref(version: str, value: str) -> None:
    expected = reservation_ref(version)
    if value == expected:
        return
    prefix = f"refs/tags/release-reservation-reclaimed/v{version}/"
    if value.startswith(prefix) and IDENTITY_SEGMENT_RE.fullmatch(value.removeprefix(prefix)):
        return
    raise ReservationError("reservation ref does not match the release version")


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


def normalize_artifact_digest(value: str) -> str:
    digest = str(value).strip()
    if digest.startswith("sha256:"):
        digest = digest.removeprefix("sha256:")
    if not ARTIFACT_DIGEST_HEX_RE.fullmatch(digest):
        raise ReservationError("artifact receipt has an invalid digest")
    return f"sha256:{digest.lower()}"


def artifact_receipt_ref(
    state: str, version: str, merge_sha: str, run_id: str = "", run_attempt: str = ""
) -> str:
    if not ARTIFACT_STATE_RE.fullmatch(state):
        raise ReservationError(f"unsupported artifact receipt state: {state}")
    validate_version(version)
    merge = normalize_sha(merge_sha, "merge SHA")
    if state == "attempt":
        if not POSITIVE_INTEGER_RE.fullmatch(str(run_id)) or not POSITIVE_INTEGER_RE.fullmatch(str(run_attempt)):
            raise ReservationError("artifact attempt receipt requires positive run identity")
        return f"refs/tags/release-artifact-attempts/v{version}/{merge}/{run_id}/{run_attempt}"
    if run_id or run_attempt:
        raise ReservationError("only artifact attempt receipts accept a run identity in the ref")
    namespace = "release-artifact-pending" if state == "pending" else "release-artifacts"
    return f"refs/tags/{namespace}/v{version}/{merge}"


def decision_ref(version: str, reservation_ref_value: str | None = None) -> str:
    validate_version(version)
    if reservation_ref_value is None or reservation_ref_value == reservation_ref(version):
        return f"refs/tags/release-decision/v{version}"
    validate_reservation_ref(version, reservation_ref_value)
    reservation_id = reservation_ref_value.rsplit("/", 1)[-1]
    return f"refs/tags/release-decision-reclaimed/v{version}/{reservation_id}"


def retirement_ref(version: str, reservation_id: str) -> str:
    validate_version(version)
    if not IDENTITY_SEGMENT_RE.fullmatch(reservation_id):
        raise ReservationError(f"invalid retirement identity: {reservation_id!r}")
    return f"refs/tags/release-retired/v{version}/{reservation_id}"


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
    *, reservation_id: str, owner: str, claim_key: str, boundary_token: str, version: str,
    channel: str, release_target: str, policy: str, state: str
) -> dict[str, str]:
    if state != "claimed":
        raise ReservationError("new reservations must start in claimed state")
    validate_release_target(release_target, policy)
    return {
        "Reservation-Id": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
        "Release-Version": version,
        "Release-Channel": channel,
        "Release-Target": release_target,
        "Release-Policy": policy,
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
    *, source_sha: str, version: str, channel: str, release_target: str, policy: str,
    owner: str, claim_key: str, reservation_id: str, boundary_token: str
) -> dict[str, Any]:
    source = normalize_sha(source_sha, "source SHA")
    validate_identity(version, channel)
    validate_target_identity(release_target, version, channel)
    metadata = reservation_metadata(
        reservation_id=reservation_id,
        owner=owner,
        claim_key=claim_key,
        boundary_token=boundary_token,
        version=version,
        channel=channel,
        release_target=release_target,
        policy=policy,
        state="claimed",
    )
    return {
        "sourceSha": source,
        "version": version,
        "channel": channel,
        "releaseTarget": release_target,
        "policy": policy,
        "reservationId": reservation_id,
        **metadata,
    }


def validate_expected_reservation(expected: dict[str, Any]) -> None:
    version = str(expected["version"])
    channel = str(expected["channel"])
    release_target = str(expected.get("releaseTarget", ""))
    policy = str(expected.get("policy", ""))
    validate_identity(version, channel)
    validate_release_target(release_target, policy)
    if policy == RELEASE_POLICY_ID:
        validate_target_identity(release_target, version, channel)
    validate_reservation_ref(version, str(expected["ref"]))
    normalize_sha(str(expected["sourceSha"]), "source SHA")
    for key in (
        "reservationId",
        "Reservation-Owner",
        "Reservation-Claim-Key",
        "Reservation-Boundary-Token",
    ):
        if not str(expected.get(key, "")).strip():
            raise ReservationError(f"reservation is missing {key}")
    if expected.get("sourceTreeSha"):
        normalize_sha(str(expected["sourceTreeSha"]), "source tree SHA")


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
        "Release-Target": expected["releaseTarget"],
        "Release-Policy": expected["policy"],
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
        release_target=str(expected["releaseTarget"]),
        policy=str(expected["policy"]),
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


def find_local_reservation_by_claim(
    *, claim_key: str, owner: str, release_target: str, policy: str = RELEASE_POLICY_ID, cwd: Path = ROOT
) -> dict[str, Any] | None:
    """Find and verify an immutable reservation before resolving a new target."""
    if not claim_key.strip() or not owner.strip():
        raise ReservationError("reservation owner and claim key are required")
    validate_release_target(release_target, policy)
    refs = _git(
        "for-each-ref", "--format=%(refname)",
        "refs/tags/release-reservation", "refs/tags/release-reservation-reclaimed", cwd=cwd,
    ).splitlines()
    matches: list[dict[str, Any]] = []
    for ref in refs:
        if not (
            ref.startswith("refs/tags/release-reservation/v")
            or ref.startswith("refs/tags/release-reservation-reclaimed/v")
        ):
            continue
        target = local_ref_target(ref, cwd)
        if not target:
            continue
        info = local_commit_info(target, cwd)
        trailers = trailers_from_message(info["message"])
        if trailers.get("Reservation-Claim-Key") != claim_key:
            continue
        if trailers.get("Reservation-Owner") != owner:
            raise ReservationError("existing reservation owner does not match the claim")
        if trailers.get("Release-Target") != release_target:
            raise ReservationError("existing reservation target does not match the claim")
        if trailers.get("Release-Policy") != policy:
            raise ReservationError("existing reservation policy does not match the claim")
        parents = info["parents"]
        if len(parents) != 1:
            raise ReservationError("existing reservation must have one source parent")
        source_info = local_commit_info(parents[0], cwd)
        expected = {
            "ref": ref,
            "sourceSha": parents[0],
            "sourceTreeSha": source_info["tree"],
            "version": trailers.get("Release-Version", ""),
            "channel": trailers.get("Release-Channel", ""),
            "releaseTarget": release_target,
            "policy": policy,
            "reservationId": trailers.get("Reservation-Id", ""),
            "Reservation-Owner": owner,
            "Reservation-Claim-Key": claim_key,
            "Reservation-Boundary-Token": trailers.get("Reservation-Boundary-Token", ""),
        }
        verified = verify_reservation_commit(target, expected, cwd)
        matches.append({**verified, "sourceTreeSha": source_info["tree"]})
    if len(matches) > 1:
        raise ReservationError("multiple reservations match the same claim")
    return matches[0] if matches else None


def reservation_ref_for_claim(
    *, version: str, reservation_id: str, owner: str = "", claim_key: str = "",
    release_target: str = "", repository: str | None = None, token: str | None = None,
    api_root: str = "https://api.github.com", cwd: Path = ROOT
) -> str:
    """Choose the immutable reservation path, reclaiming only a verified retired legacy claim."""
    default_ref = reservation_ref(version)
    if repository and token:
        client = GitHubRefClient(repository, token, api_root)
        if not client.ref_target(default_ref):
            return default_ref
        legacy_target = client.ref_target(default_ref)
        if not legacy_target:
            raise ReservationError("release reservation changed while resolving its immutable path")
        legacy_info = client.commit_info(legacy_target)
        legacy_trailers = trailers_from_message(str(legacy_info.get("message", "")))
        legacy_id = legacy_trailers.get("Reservation-Id", "")
        if not legacy_id:
            raise ReservationError("existing reservation has no immutable reservation id")
        if (
            legacy_id == reservation_id
            and legacy_trailers.get("Reservation-Owner") == owner
            and legacy_trailers.get("Reservation-Claim-Key") == claim_key
            and legacy_trailers.get("Release-Target") == release_target
        ):
            return default_ref
        verify_github_retirement(
            version=version, reservation_id=legacy_id, repository=repository, token=token,
            api_root=api_root,
        )
        return reclaimed_reservation_ref(version, reservation_id)
    if not local_ref_target(default_ref, cwd):
        return default_ref
    legacy_target = local_ref_target(default_ref, cwd)
    if not legacy_target:
        raise ReservationError("release reservation changed while resolving its immutable path")
    legacy_trailers = trailers_from_message(local_commit_info(legacy_target, cwd)["message"])
    legacy_id = legacy_trailers.get("Reservation-Id", "")
    if not legacy_id:
        raise ReservationError("existing reservation has no immutable reservation id")
    if (
        legacy_id == reservation_id
        and legacy_trailers.get("Reservation-Owner") == owner
        and legacy_trailers.get("Reservation-Claim-Key") == claim_key
        and legacy_trailers.get("Release-Target") == release_target
    ):
        return default_ref
    verify_local_retirement(version=version, reservation_id=legacy_id, cwd=cwd)
    return reclaimed_reservation_ref(version, reservation_id)


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
        if not isinstance(value, str):
            return None
        target = normalize_sha(value, "remote ref target")
        if payload.get("object", {}).get("type") != "tag":
            return target
        tag_status, tag_payload = self.request_json("GET", f"/repos/{self.repository}/git/tags/{target}")
        if tag_status != 200:
            raise ReservationError("annotated identity ref target is unavailable")
        commit = tag_payload.get("object", {}).get("sha")
        return normalize_sha(commit, "annotated identity ref commit") if isinstance(commit, str) else None

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

    def create_tree(self) -> str:
        blob_status, blob_payload = self.request_json(
            "POST",
            f"/repos/{self.repository}/git/blobs",
            {"content": IDENTITY_METADATA_CONTENT, "encoding": "utf-8"},
        )
        if blob_status not in {200, 201} or not isinstance(blob_payload.get("sha"), str):
            raise ReservationError("GitHub did not return an identity metadata blob")
        status, payload = self.request_json(
            "POST",
            f"/repos/{self.repository}/git/trees",
            {
                "tree": [
                    {
                        "path": ".televybackup-release-identity",
                        "mode": "100644",
                        "type": "blob",
                        "sha": normalize_sha(blob_payload["sha"], "identity metadata blob"),
                    }
                ]
            },
        )
        if status not in {200, 201} or not isinstance(payload.get("sha"), str):
            raise ReservationError("GitHub did not return an identity metadata tree")
        tree = normalize_sha(payload["sha"], "identity metadata tree")
        if tree != IDENTITY_METADATA_TREE_SHA:
            raise ReservationError("GitHub returned an unexpected identity metadata tree")
        return tree

    def create_annotated_tag(self, ref: str, sha: str) -> str:
        tag = ref.removeprefix("refs/tags/")
        status, payload = self.request_json(
            "POST",
            f"/repos/{self.repository}/git/tags",
            {"tag": tag, "message": f"release identity ref: {tag}", "object": normalize_sha(sha), "type": "commit"},
        )
        if status not in {200, 201} or not isinstance(payload.get("sha"), str):
            raise ReservationError(f"GitHub did not create an annotated identity tag: {ref}")
        return normalize_sha(payload["sha"], "annotated identity tag")

    def _git_env(self) -> dict[str, str]:
        encoded = base64.b64encode(f"x-access-token:{self.token}".encode("utf-8")).decode("ascii")
        env = os.environ.copy()
        env.update(
            {
                "GIT_CONFIG_COUNT": "1",
                "GIT_CONFIG_KEY_0": "http.https://github.com/.extraheader",
                "GIT_CONFIG_VALUE_0": f"AUTHORIZATION: basic {encoded}",
                "GIT_TERMINAL_PROMPT": "0",
            }
        )
        return env

    def _git(self, *args: str) -> str:
        result = subprocess.run(
            ["git", *args],
            cwd=ROOT,
            env=self._git_env(),
            text=True,
            capture_output=True,
        )
        if result.returncode != 0:
            raise ReservationError(result.stderr.strip() or f"git {' '.join(args)} failed")
        return result.stdout.strip()

    def _push_ref(self, ref: str, sha: str) -> str:
        existing = self.ref_target(ref)
        if existing:
            if existing != sha:
                raise ReservationError(f"existing remote ref does not match the requested identity: {ref}")
            return existing
        self._git("fetch", "--no-tags", "origin", sha)
        try:
            self._git("push", "origin", f"{sha}:{ref}")
        except ReservationError:
            existing = self.ref_target(ref)
            if existing == sha:
                return existing
            raise
        target = self.ref_target(ref)
        if target != sha:
            raise ReservationError(f"git push did not create the requested remote ref: {ref}")
        return target

    def create_ref(self, ref: str, sha: str) -> str:
        normalized = normalize_sha(sha)
        try:
            status, payload = self.request_json(
                "POST", f"/repos/{self.repository}/git/refs", {"ref": ref, "sha": normalized}
            )
        except ReservationError as error:
            if " returned 403:" not in str(error):
                raise
            tag_object = self.create_annotated_tag(ref, normalized)
            try:
                self.request_json(
                    "POST", f"/repos/{self.repository}/git/refs", {"ref": ref, "sha": tag_object}
                )
            except ReservationError:
                existing = self.ref_target(ref)
                if existing == normalized:
                    return existing
                raise
            target = self.ref_target(ref)
            if target != normalized:
                raise ReservationError(f"annotated identity ref does not resolve to the requested commit: {ref}")
            return target
        if status not in {200, 201}:
            raise ReservationError(f"GitHub did not create append-only ref {ref}")
        target = self.ref_target(ref)
        if target is None:
            value = payload.get("object", {}).get("sha", normalized)
            return normalize_sha(value, "created ref target")
        if target != normalized:
            raise ReservationError(f"created remote ref does not match the requested identity: {ref}")
        return target


def retirement_evidence_digest(evidence: dict[str, Any]) -> str:
    canonical = json.dumps(evidence, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return f"sha256:{hashlib.sha256(canonical.encode('utf-8')).hexdigest()}"


def validate_retirement_evidence(evidence: dict[str, Any], version: str) -> None:
    if not isinstance(evidence, dict) or evidence.get("version") != version:
        raise ReservationError("retirement evidence version does not match the release version")
    if evidence.get("schema_version") != 1:
        raise ReservationError("unsupported retirement evidence schema")
    if evidence.get("product_tag", {}).get("status") != "absent":
        raise ReservationError("retirement requires an absent product tag")
    if evidence.get("github_release", {}).get("status") != "absent":
        raise ReservationError("retirement requires an absent GitHub Release")
    if evidence.get("consumed_refs") != []:
        raise ReservationError("retirement requires no stable consumed refs")
    if evidence.get("artifact_refs") != []:
        raise ReservationError("retirement requires no stable artifact refs")
    runs = evidence.get("release_runs")
    if not isinstance(runs, list) or not runs:
        raise ReservationError("retirement evidence must list the checked Release Product runs")
    for run in runs:
        if not isinstance(run, dict) or not str(run.get("id", "")).strip():
            raise ReservationError("retirement evidence has an invalid Release Product run")
        if run.get("artifacts") != []:
            raise ReservationError("retirement requires checked Release Product runs with no artifacts")
    normalize_sha(str(evidence.get("main_sha", "")), "retirement evidence main SHA")


def _retirement_static_fields(
    *, version: str, reservation_id: str, reservation_ref_value: str, decision_ref_value: str,
    released_ref_value: str, reservation_target: str, decision_target: str, released_target: str,
    owner: str, reason: str
) -> dict[str, str]:
    return {
        "Retirement-State": "retired",
        "Release-Version": version,
        "Retirement-Reservation-Id": reservation_id,
        "Retirement-Source-Reservation-Ref": reservation_ref_value,
        "Retirement-Source-Reservation-SHA": reservation_target,
        "Retirement-Source-Decision-Ref": decision_ref_value,
        "Retirement-Source-Decision-SHA": decision_target,
        "Retirement-Source-Released-Ref": released_ref_value,
        "Retirement-Source-Released-SHA": released_target,
        "Retirement-Product-Tag": "absent",
        "Retirement-GitHub-Release": "absent",
        "Retirement-Consumed-Refs": "none",
        "Retirement-Artifact-Refs": "none",
        "Retirement-Owner": owner,
        "Retirement-Reason": reason,
        "Retirement-Provenance": "immutable-retirement-reconciliation",
    }


def retirement_fields(
    *, version: str, reservation_id: str, reservation_ref_value: str, decision_ref_value: str,
    released_ref_value: str, reservation_target: str, decision_target: str, released_target: str,
    owner: str, reason: str, evidence: dict[str, Any]
) -> dict[str, str]:
    validate_retirement_evidence(evidence, version)
    fields = _retirement_static_fields(
        version=version, reservation_id=reservation_id, reservation_ref_value=reservation_ref_value,
        decision_ref_value=decision_ref_value, released_ref_value=released_ref_value,
        reservation_target=reservation_target, decision_target=decision_target,
        released_target=released_target, owner=owner, reason=reason,
    )
    run_ids = [str(run["id"]) for run in evidence["release_runs"]]
    fields.update({
        "Retirement-Artifact-Runs": ",".join(run_ids),
        "Retirement-Evidence-Main-SHA": normalize_sha(str(evidence["main_sha"]), "retirement evidence main SHA"),
        "Retirement-Evidence-Digest": retirement_evidence_digest(evidence),
    })
    return fields


def retirement_message(fields: dict[str, str]) -> str:
    lines = ["release: immutable legacy identity retirement", ""]
    lines.extend(f"{key}: {value}" for key, value in fields.items())
    return "\n".join(lines) + "\n"


def verify_retirement_trailers(actual: dict[str, str], fields: dict[str, str], evidence: dict[str, Any] | None) -> None:
    if any(actual.get(key) != value for key, value in fields.items()):
        raise ReservationError("retirement receipt does not match the requested identity")
    if evidence is None:
        if not actual.get("Retirement-Artifact-Runs") or actual.get("Retirement-Artifact-Runs") == "none":
            raise ReservationError("retirement receipt is missing checked Release Product runs")
        normalize_sha(actual.get("Retirement-Evidence-Main-SHA", ""), "retirement evidence main SHA")
        if not re.fullmatch(r"sha256:[0-9a-fA-F]{64}", actual.get("Retirement-Evidence-Digest", "")):
            raise ReservationError("retirement receipt has an invalid evidence digest")
    else:
        expected = retirement_fields(
            version=fields["Release-Version"], reservation_id=fields["Retirement-Reservation-Id"],
            reservation_ref_value=fields["Retirement-Source-Reservation-Ref"],
            decision_ref_value=fields["Retirement-Source-Decision-Ref"],
            released_ref_value=fields["Retirement-Source-Released-Ref"],
            reservation_target=fields["Retirement-Source-Reservation-SHA"],
            decision_target=fields["Retirement-Source-Decision-SHA"],
            released_target=fields["Retirement-Source-Released-SHA"],
            owner=fields["Retirement-Owner"], reason=fields["Retirement-Reason"], evidence=evidence,
        )
        if any(actual.get(key) != value for key, value in expected.items()):
            raise ReservationError("retirement receipt does not match the supplied evidence")


def _verify_retirement_source_local(
    *, version: str, reservation_id: str, owner: str, cwd: Path = ROOT
) -> dict[str, Any]:
    reservation_ref_value = reservation_ref(version)
    decision_ref_value = decision_ref(version)
    released_ref_value = receipt_ref("released", version, reservation_id)
    reservation_target = local_ref_target(reservation_ref_value, cwd)
    decision_target = local_ref_target(decision_ref_value, cwd)
    released_target = local_ref_target(released_ref_value, cwd)
    if not reservation_target or not decision_target or not released_target:
        raise ReservationError("legacy retirement source refs are incomplete")
    reservation_info = local_commit_info(reservation_target, cwd)
    if len(reservation_info["parents"]) != 1:
        raise ReservationError("legacy reservation must have one source parent")
    source_info = local_commit_info(reservation_info["parents"][0], cwd)
    if reservation_info["tree"] != source_info["tree"]:
        raise ReservationError("legacy reservation tree does not match its source")
    reservation_trailers = trailers_from_message(reservation_info["message"])
    if any(reservation_trailers.get(key) != value for key, value in {
        "Reservation-Id": reservation_id,
        "Reservation-Owner": owner,
        "Release-Version": version,
        "Reservation-State": "claimed",
    }.items()):
        raise ReservationError("legacy reservation identity does not match the retirement request")
    decision_info = local_commit_info(decision_target, cwd)
    if decision_info["parents"] != [reservation_target] or decision_info["tree"] not in {reservation_info["tree"], IDENTITY_METADATA_TREE_SHA}:
        raise ReservationError("legacy decision provenance does not match the reservation")
    decision_trailers = trailers_from_message(decision_info["message"])
    for key, value in {
        "Decision-State": "released",
        "Release-Version": version,
        "Release-Reservation-Id": reservation_id,
        "Release-Owner": owner,
        "Release-Reservation-Ref": reservation_ref_value,
        "Decision-Provenance": "immutable-decision",
    }.items():
        if decision_trailers.get(key) != value:
            raise ReservationError(f"legacy decision trailer {key} does not match")
    merge_sha = normalize_sha(decision_trailers.get("Release-Merge-SHA", ""), "legacy release merge SHA")
    released_info = local_commit_info(released_target, cwd)
    merge_info = local_commit_info(merge_sha, cwd)
    if released_info["parents"] != [merge_sha] or released_info["tree"] not in {merge_info["tree"], IDENTITY_METADATA_TREE_SHA}:
        raise ReservationError("legacy released receipt provenance does not match its merge SHA")
    released_trailers = trailers_from_message(released_info["message"])
    for key, value in {
        "Receipt-State": "released",
        "Release-Version": version,
        "Release-Reservation-Id": reservation_id,
        "Release-Owner": owner,
        "Release-Reservation-Ref": reservation_ref_value,
        "Receipt-Provenance": "immutable-receipt",
    }.items():
        if released_trailers.get(key) != value:
            raise ReservationError(f"legacy released trailer {key} does not match")
    return {
        "reservationRef": reservation_ref_value,
        "decisionRef": decision_ref_value,
        "releasedRef": released_ref_value,
        "reservationTarget": reservation_target,
        "decisionTarget": decision_target,
        "releasedTarget": released_target,
        "mergeSha": merge_sha,
        "reservationTrailers": reservation_trailers,
        "decisionTrailers": decision_trailers,
        "releasedTrailers": released_trailers,
    }


def verify_local_retirement(
    *, version: str, reservation_id: str, owner: str = "github-actions[bot]",
    reason: str = "no-product-publication", evidence: dict[str, Any] | None = None,
    cwd: Path = ROOT
) -> dict[str, Any]:
    source = _verify_retirement_source_local(version=version, reservation_id=reservation_id, owner=owner, cwd=cwd)
    ref = retirement_ref(version, reservation_id)
    target = local_ref_target(ref, cwd)
    if not target:
        raise ReservationError(f"retirement ref is missing: {ref}")
    if evidence is None:
        fields = _retirement_static_fields(
            version=version, reservation_id=reservation_id, reservation_ref_value=source["reservationRef"],
            decision_ref_value=source["decisionRef"], released_ref_value=source["releasedRef"],
            reservation_target=source["reservationTarget"], decision_target=source["decisionTarget"],
            released_target=source["releasedTarget"], owner=owner, reason=reason,
        )
    else:
        fields = retirement_fields(
            version=version, reservation_id=reservation_id, reservation_ref_value=source["reservationRef"],
            decision_ref_value=source["decisionRef"], released_ref_value=source["releasedRef"],
            reservation_target=source["reservationTarget"], decision_target=source["decisionTarget"],
            released_target=source["releasedTarget"], owner=owner, reason=reason, evidence=evidence,
        )
    info = local_commit_info(target, cwd)
    released_info = local_commit_info(source["releasedTarget"], cwd)
    if info["parents"] != [source["releasedTarget"]] or info["tree"] not in {released_info["tree"], IDENTITY_METADATA_TREE_SHA}:
        raise ReservationError("retirement receipt provenance does not match the released receipt")
    actual = trailers_from_message(info["message"])
    verify_retirement_trailers(actual, fields, evidence)
    return {"ref": ref, "target": target, **fields}


def _verify_retirement_source_github(
    *, client: GitHubRefClient, version: str, reservation_id: str, owner: str
) -> dict[str, Any]:
    reservation_ref_value = reservation_ref(version)
    decision_ref_value = decision_ref(version)
    released_ref_value = receipt_ref("released", version, reservation_id)
    reservation_target = client.ref_target(reservation_ref_value)
    decision_target = client.ref_target(decision_ref_value)
    released_target = client.ref_target(released_ref_value)
    if not reservation_target or not decision_target or not released_target:
        raise ReservationError("legacy retirement source refs are incomplete")
    reservation_info = client.commit_info(reservation_target)
    reservation_parents = [parent.get("sha") for parent in reservation_info.get("parents", [])]
    if len(reservation_parents) != 1 or not isinstance(reservation_parents[0], str):
        raise ReservationError("legacy reservation must have one source parent")
    source_info = client.commit_info(reservation_parents[0])
    if reservation_info.get("tree", {}).get("sha") != source_info.get("tree", {}).get("sha"):
        raise ReservationError("legacy reservation tree does not match its source")
    reservation_trailers = trailers_from_message(str(reservation_info.get("message", "")))
    for key, value in {
        "Reservation-Id": reservation_id,
        "Reservation-Owner": owner,
        "Release-Version": version,
        "Reservation-State": "claimed",
    }.items():
        if reservation_trailers.get(key) != value:
            raise ReservationError(f"legacy reservation trailer {key} does not match")
    decision_info = client.commit_info(decision_target)
    decision_parents = [parent.get("sha") for parent in decision_info.get("parents", [])]
    if decision_parents != [reservation_target] or decision_info.get("tree", {}).get("sha") not in {
        reservation_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA
    }:
        raise ReservationError("legacy decision provenance does not match the reservation")
    decision_trailers = trailers_from_message(str(decision_info.get("message", "")))
    for key, value in {
        "Decision-State": "released",
        "Release-Version": version,
        "Release-Reservation-Id": reservation_id,
        "Release-Owner": owner,
        "Release-Reservation-Ref": reservation_ref_value,
        "Decision-Provenance": "immutable-decision",
    }.items():
        if decision_trailers.get(key) != value:
            raise ReservationError(f"legacy decision trailer {key} does not match")
    merge_sha = normalize_sha(decision_trailers.get("Release-Merge-SHA", ""), "legacy release merge SHA")
    released_info = client.commit_info(released_target)
    merge_info = client.commit_info(merge_sha)
    released_parents = [parent.get("sha") for parent in released_info.get("parents", [])]
    if released_parents != [merge_sha] or released_info.get("tree", {}).get("sha") not in {
        merge_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA
    }:
        raise ReservationError("legacy released receipt provenance does not match its merge SHA")
    released_trailers = trailers_from_message(str(released_info.get("message", "")))
    for key, value in {
        "Receipt-State": "released",
        "Release-Version": version,
        "Release-Reservation-Id": reservation_id,
        "Release-Owner": owner,
        "Release-Reservation-Ref": reservation_ref_value,
        "Receipt-Provenance": "immutable-receipt",
    }.items():
        if released_trailers.get(key) != value:
            raise ReservationError(f"legacy released trailer {key} does not match")
    return {
        "reservationRef": reservation_ref_value,
        "decisionRef": decision_ref_value,
        "releasedRef": released_ref_value,
        "reservationTarget": reservation_target,
        "decisionTarget": decision_target,
        "releasedTarget": released_target,
        "mergeSha": merge_sha,
        "reservationTrailers": reservation_trailers,
        "decisionTrailers": decision_trailers,
        "releasedTrailers": released_trailers,
    }


def verify_github_retirement(
    *, version: str, reservation_id: str, repository: str, token: str,
    api_root: str = "https://api.github.com", owner: str = "github-actions[bot]",
    reason: str = "no-product-publication", evidence: dict[str, Any] | None = None
) -> dict[str, Any]:
    client = GitHubRefClient(repository, token, api_root)
    source = _verify_retirement_source_github(
        client=client, version=version, reservation_id=reservation_id, owner=owner
    )
    ref = retirement_ref(version, reservation_id)
    target = client.ref_target(ref)
    if not target:
        raise ReservationError(f"retirement ref is missing: {ref}")
    if evidence is None:
        fields = _retirement_static_fields(
            version=version, reservation_id=reservation_id, reservation_ref_value=source["reservationRef"],
            decision_ref_value=source["decisionRef"], released_ref_value=source["releasedRef"],
            reservation_target=source["reservationTarget"], decision_target=source["decisionTarget"],
            released_target=source["releasedTarget"], owner=owner, reason=reason,
        )
    else:
        fields = retirement_fields(
            version=version, reservation_id=reservation_id, reservation_ref_value=source["reservationRef"],
            decision_ref_value=source["decisionRef"], released_ref_value=source["releasedRef"],
            reservation_target=source["reservationTarget"], decision_target=source["decisionTarget"],
            released_target=source["releasedTarget"], owner=owner, reason=reason, evidence=evidence,
        )
    info = client.commit_info(target)
    released_info = client.commit_info(source["releasedTarget"])
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    if parents != [source["releasedTarget"]] or info.get("tree", {}).get("sha") not in {
        released_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA
    }:
        raise ReservationError("retirement receipt provenance does not match the released receipt")
    actual = trailers_from_message(str(info.get("message", "")))
    verify_retirement_trailers(actual, fields, evidence)
    return {"ref": ref, "target": target, **fields}


def _stable_identity_refs_local(version: str, cwd: Path) -> list[str]:
    refs: list[str] = []
    for namespace in (
        "release-consumed",
        "release-artifact-pending",
        "release-artifact-attempts",
        "release-artifacts",
    ):
        refs.extend(_git("for-each-ref", "--format=%(refname)", f"refs/tags/{namespace}/v{version}", cwd=cwd).splitlines())
    return [ref for ref in refs if f"/v{version}/" in ref]


def ensure_local_retirement_unpublished(version: str, cwd: Path = ROOT) -> None:
    if local_ref_target(f"refs/tags/v{version}", cwd):
        raise ReservationError("cannot retire a version with a product tag")
    refs = _stable_identity_refs_local(version, cwd)
    if refs:
        raise ReservationError(f"cannot retire a version with stable identity refs: {', '.join(refs)}")


def _remote_matching_refs(client: GitHubRefClient, namespace: str, version: str) -> list[str]:
    status, payload = client.request_json(
        "GET", f"/repos/{client.repository}/git/matching-refs/tags/{namespace}/v{version}"
    )
    if status == 404:
        return []
    if not isinstance(payload, list):
        raise ReservationError(f"GitHub returned invalid matching refs for {namespace}")
    prefix = f"refs/tags/{namespace}/v{version}/"
    return [row.get("ref", "") for row in payload if isinstance(row, dict) and row.get("ref", "").startswith(prefix)]


def ensure_github_retirement_unpublished(client: GitHubRefClient, version: str) -> None:
    if client.ref_target(f"refs/tags/v{version}"):
        raise ReservationError("cannot retire a version with a product tag")
    status, _ = client.request_json("GET", f"/repos/{client.repository}/releases/tags/v{version}")
    if status != 404:
        raise ReservationError("cannot retire a version with a GitHub Release")
    refs: list[str] = []
    for namespace in (
        "release-consumed",
        "release-artifact-pending",
        "release-artifact-attempts",
        "release-artifacts",
    ):
        refs.extend(_remote_matching_refs(client, namespace, version))
    if refs:
        raise ReservationError(f"cannot retire a version with stable identity refs: {', '.join(refs)}")


def create_local_retirement(
    *, version: str, reservation_id: str, evidence: dict[str, Any], owner: str = "github-actions[bot]",
    reason: str = "no-product-publication", cwd: Path = ROOT
) -> dict[str, Any]:
    ensure_local_retirement_unpublished(version, cwd)
    source = _verify_retirement_source_local(version=version, reservation_id=reservation_id, owner=owner, cwd=cwd)
    ref = retirement_ref(version, reservation_id)
    existing = local_ref_target(ref, cwd)
    if existing:
        return verify_local_retirement(
            version=version, reservation_id=reservation_id, owner=owner, reason=reason,
            evidence=None, cwd=cwd,
        )
    fields = retirement_fields(
        version=version, reservation_id=reservation_id, reservation_ref_value=source["reservationRef"],
        decision_ref_value=source["decisionRef"], released_ref_value=source["releasedRef"],
        reservation_target=source["reservationTarget"], decision_target=source["decisionTarget"],
        released_target=source["releasedTarget"], owner=owner, reason=reason, evidence=evidence,
    )
    released_info = local_commit_info(source["releasedTarget"], cwd)
    commit = _git(
        "commit-tree", released_info["tree"], "-p", source["releasedTarget"],
        "-m", retirement_message(fields), cwd=cwd,
    )
    update = subprocess.run(["git", "update-ref", ref, commit, "0" * 40], cwd=cwd, text=True, capture_output=True)
    if update.returncode != 0:
        existing = local_ref_target(ref, cwd)
        if existing:
            return verify_local_retirement(
                version=version, reservation_id=reservation_id, owner=owner, reason=reason,
                evidence=evidence, cwd=cwd,
            )
        raise ReservationError(update.stderr.strip() or "retirement ref creation failed")
    return verify_local_retirement(
        version=version, reservation_id=reservation_id, owner=owner, reason=reason,
        evidence=evidence, cwd=cwd,
    )


def create_github_retirement(
    *, version: str, reservation_id: str, evidence: dict[str, Any], repository: str, token: str,
    api_root: str = "https://api.github.com", owner: str = "github-actions[bot]",
    reason: str = "no-product-publication"
) -> dict[str, Any]:
    client = GitHubRefClient(repository, token, api_root)
    ensure_github_retirement_unpublished(client, version)
    source = _verify_retirement_source_github(
        client=client, version=version, reservation_id=reservation_id, owner=owner
    )
    ref = retirement_ref(version, reservation_id)
    existing = client.ref_target(ref)
    if existing:
        return verify_github_retirement(
            version=version, reservation_id=reservation_id, repository=repository, token=token,
            api_root=api_root, owner=owner, reason=reason, evidence=None,
        )
    fields = retirement_fields(
        version=version, reservation_id=reservation_id, reservation_ref_value=source["reservationRef"],
        decision_ref_value=source["decisionRef"], released_ref_value=source["releasedRef"],
        reservation_target=source["reservationTarget"], decision_target=source["decisionTarget"],
        released_target=source["releasedTarget"], owner=owner, reason=reason, evidence=evidence,
    )
    commit = client.create_commit(
        tree=client.create_tree(), parent=source["releasedTarget"], message=retirement_message(fields)
    )
    try:
        target = client.create_ref(ref, commit)
    except ReservationError:
        target = client.ref_target(ref)
        if not target:
            raise
    if target != commit:
        return verify_github_retirement(
            version=version, reservation_id=reservation_id, repository=repository, token=token,
            api_root=api_root, owner=owner, reason=reason, evidence=evidence,
        )
    return verify_github_retirement(
        version=version, reservation_id=reservation_id, repository=repository, token=token,
        api_root=api_root, owner=owner, reason=reason, evidence=evidence,
    )


def verify_github_decision(
    *, client: GitHubRefClient, fields: dict[str, str], reservation: dict[str, Any]
) -> dict[str, Any]:
    ref = decision_ref(str(reservation["version"]), str(reservation["ref"]))
    target = client.ref_target(ref)
    if not target:
        raise ReservationError(f"decision ref is missing: {ref}")
    info = client.commit_info(target)
    reservation_info = client.commit_info(str(reservation["target"]))
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    allowed_trees = {reservation_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA}
    if parents != [reservation_info.get("sha")] or info.get("tree", {}).get("sha") not in allowed_trees:
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
    ref = decision_ref(str(reservation["version"]), str(reservation["ref"]))
    existing = client.ref_target(ref)
    if existing:
        return {"ref": ref, **verify_github_decision(client=client, fields=fields, reservation=reservation)}
    reservation_info = client.commit_info(str(reservation["target"]))
    tree = client.create_tree()
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
            "Release-Target": expected["releaseTarget"],
            "Release-Policy": expected["policy"],
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
        release_target=str(expected["releaseTarget"]),
        policy=str(expected["policy"]),
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
    source_tree = ""
    try:
        source_info = client.commit_info(source)
        source_tree = str(source_info.get("tree", {}).get("sha", ""))
    except ReservationError as error:
        # GitHub can retain a ref whose historical parent is no longer exposed
        # by the Git Database API. The trusted checkout supplies the same tree
        # digest captured before the ref was created; other API failures remain
        # fatal and cannot use this recovery path.
        message = str(error)
        if "returned 422" not in message or "No commit found for SHA" not in message:
            raise
        source_tree = str(expected.get("sourceTreeSha", ""))
    if parents != [source] or info.get("tree", {}).get("sha") != source_tree:
        raise ReservationError("remote reservation provenance does not match the claim")
    trailers = trailers_from_message(str(info.get("message", "")))
    for key, value in {
        "Reservation-Id": expected["reservationId"],
        "Reservation-Owner": expected["Reservation-Owner"],
        "Reservation-Claim-Key": expected["Reservation-Claim-Key"],
        "Reservation-Boundary-Token": expected["Reservation-Boundary-Token"],
        "Release-Version": expected["version"],
        "Release-Channel": expected["channel"],
        "Release-Target": expected["releaseTarget"],
        "Release-Policy": expected["policy"],
        "Reservation-State": "claimed",
    }.items():
        if trailers.get(key) != value:
            raise ReservationError(f"remote reservation trailer {key} does not match")
    return {"ref": ref, "target": target, **expected}


def verify_local_reservation_claim(
    *, reservation_ref_value: str, version: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, release_target: str | None = None, policy: str | None = None,
    cwd: Path = ROOT
) -> dict[str, Any]:
    validate_version(version)
    validate_reservation_ref(version, reservation_ref_value)
    target = local_ref_target(reservation_ref_value, cwd)
    if not target:
        raise ReservationError(f"reservation ref is missing: {reservation_ref_value}")
    info = local_commit_info(target, cwd)
    if len(info["parents"]) != 1:
        raise ReservationError("reservation must have one source parent")
    trailers = trailers_from_message(info["message"])
    channel = trailers.get("Release-Channel", "")
    recorded_target = trailers.get("Release-Target", "")
    recorded_policy = trailers.get("Release-Policy", "")
    expected = {
        "ref": reservation_ref_value,
        "sourceSha": info["parents"][0],
        "version": version,
        "channel": channel,
        "releaseTarget": release_target or recorded_target,
        "policy": policy or recorded_policy,
        "reservationId": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
    }
    if release_target and recorded_target != release_target:
        raise ReservationError("reservation release target does not match the claim")
    if policy and recorded_policy != policy:
        raise ReservationError("reservation release policy does not match the claim")
    return verify_reservation_commit(target, expected, cwd)


def verify_github_reservation_claim(
    *, reservation_ref_value: str, version: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, release_target: str | None = None, policy: str | None = None,
    repository: str, token: str, api_root: str = "https://api.github.com"
) -> dict[str, Any]:
    client = GitHubRefClient(repository, token, api_root)
    validate_version(version)
    validate_reservation_ref(version, reservation_ref_value)
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
    recorded_target = trailers.get("Release-Target", "")
    recorded_policy = trailers.get("Release-Policy", "")
    expected = {
        "ref": reservation_ref_value,
        "sourceSha": source,
        "version": version,
        "channel": channel,
        "releaseTarget": release_target or recorded_target,
        "policy": policy or recorded_policy,
        "reservationId": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
    }
    if release_target and recorded_target != release_target:
        raise ReservationError("remote reservation release target does not match the claim")
    if policy and recorded_policy != policy:
        raise ReservationError("remote reservation release policy does not match the claim")
    validate_identity(version, channel)
    validate_target_identity(expected["releaseTarget"], version, channel)
    for key, value in {
        "Reservation-Id": reservation_id,
        "Reservation-Owner": owner,
        "Reservation-Claim-Key": claim_key,
        "Reservation-Boundary-Token": boundary_token,
        "Release-Version": version,
        "Release-Channel": channel,
        "Release-Target": expected["releaseTarget"],
        "Release-Policy": expected["policy"],
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
        "Release-Target": reservation["releaseTarget"],
        "Release-Policy": reservation["policy"],
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
    ref = decision_ref(str(reservation["version"]), str(reservation["ref"]))
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
    ref = decision_ref(str(reservation["version"]), str(reservation["ref"]))
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
    boundary_token: str, reservation_ref_value: str, release_target: str | None = None,
    policy: str | None = None
) -> dict[str, str]:
    if state not in {"bound", "consumed", "released"}:
        raise ReservationError(f"unsupported receipt state: {state}")
    validate_version(version)
    fields = {
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
    if (release_target is None) != (policy is None):
        raise ReservationError("receipt release target and policy must be supplied together")
    if release_target is not None and policy is not None:
        validate_release_target(release_target, policy)
        fields["Release-Target"] = release_target
        fields["Release-Policy"] = policy
    return fields


def artifact_receipt_fields(
    *, state: str, version: str, merge_sha: str, run_id: str, run_attempt: str,
    packaging_policy_sha: str, artifact_id: str = "", artifact_digest: str = "",
    helper_mode: str = "pending", helper_tag: str = "", helper_commit: str = "",
    artifact_name: str = "release-assets"
) -> dict[str, str]:
    if not ARTIFACT_STATE_RE.fullmatch(state):
        raise ReservationError(f"unsupported artifact receipt state: {state}")
    validate_version(version)
    merge = normalize_sha(merge_sha, "merge SHA")
    if not POSITIVE_INTEGER_RE.fullmatch(str(run_id)):
        raise ReservationError("artifact receipt run id must be a positive integer")
    if not POSITIVE_INTEGER_RE.fullmatch(str(run_attempt)):
        raise ReservationError("artifact receipt run attempt must be a positive integer")
    if not ARTIFACT_NAME_RE.fullmatch(str(artifact_name)):
        raise ReservationError("artifact receipt has an invalid artifact name")
    packaging = normalize_sha(packaging_policy_sha, "packaging policy SHA")
    fields = {
        "Artifact-State": state,
        "Release-Version": version,
        "Release-Merge-SHA": merge,
        "Release-Run-Id": str(run_id),
        "Release-Run-Attempt": str(run_attempt),
        "Release-Packaging-Policy-SHA": packaging,
        "Release-Artifact-Name": str(artifact_name),
        "Artifact-Helper-Mode": helper_mode,
        "Artifact-Provenance": "immutable-release-artifact",
    }
    if helper_tag:
        fields["Artifact-Helper-Tag"] = helper_tag
    if helper_commit:
        fields["Artifact-Helper-Commit"] = helper_commit
    if state in {"pending", "attempt"}:
        if helper_mode != "pending" or artifact_id or artifact_digest:
            raise ReservationError("pending or attempt artifact receipt cannot contain finalized artifact fields")
    else:
        if helper_mode not in {"reuse", "bootstrap"}:
            raise ReservationError("finalized artifact receipt has an invalid helper mode")
        if not POSITIVE_INTEGER_RE.fullmatch(str(artifact_id)):
            raise ReservationError("finalized artifact receipt id must be a positive integer")
        normalized_digest = normalize_artifact_digest(str(artifact_digest))
        fields["Release-Artifact-Id"] = str(artifact_id)
        fields["Release-Artifact-Digest"] = normalized_digest
    return fields


def artifact_receipt_message(fields: dict[str, str]) -> str:
    lines = ["release: immutable release artifact receipt", ""]
    lines.extend(f"{key}: {value}" for key, value in fields.items())
    return "\n".join(lines) + "\n"


def verify_artifact_receipt_commit(
    ref_target: str, *, state: str, version: str, merge_sha: str, cwd: Path = ROOT
) -> dict[str, Any]:
    info = local_commit_info(ref_target, cwd)
    merge = normalize_sha(merge_sha, "merge SHA")
    merge_info = local_commit_info(merge, cwd)
    if info["parents"] != [merge] or info["tree"] not in {merge_info["tree"], IDENTITY_METADATA_TREE_SHA}:
        raise ReservationError("artifact receipt provenance does not match its merge SHA")
    actual = trailers_from_message(info["message"])
    expected = artifact_receipt_fields(
        state=state,
        version=version,
        merge_sha=merge,
        run_id=actual.get("Release-Run-Id", ""),
        run_attempt=actual.get("Release-Run-Attempt", ""),
        packaging_policy_sha=actual.get("Release-Packaging-Policy-SHA", ""),
        artifact_id=actual.get("Release-Artifact-Id", ""),
        artifact_digest=actual.get("Release-Artifact-Digest", ""),
        helper_mode=actual.get("Artifact-Helper-Mode", ""),
        helper_tag=actual.get("Artifact-Helper-Tag", ""),
        helper_commit=actual.get("Artifact-Helper-Commit", ""),
        artifact_name=actual.get("Release-Artifact-Name", ""),
    )
    if any(actual.get(key) != value for key, value in expected.items()):
        raise ReservationError("artifact receipt does not match its declared identity")
    return {
        "ref": artifact_receipt_ref(
            state,
            version,
            merge,
            actual.get("Release-Run-Id", "") if state == "attempt" else "",
            actual.get("Release-Run-Attempt", "") if state == "attempt" else "",
        ),
        "target": info["sha"],
        **actual,
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


def create_local_artifact_receipt(
    *, state: str, version: str, merge_sha: str, run_id: str, run_attempt: str,
    packaging_policy_sha: str, artifact_id: str = "", artifact_digest: str = "",
    helper_mode: str = "pending", helper_tag: str = "", helper_commit: str = "",
    artifact_name: str = "release-assets", cwd: Path = ROOT
) -> dict[str, Any]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = artifact_receipt_ref(
        state,
        version,
        merge,
        run_id if state == "attempt" else "",
        run_attempt if state == "attempt" else "",
    )
    fields = artifact_receipt_fields(
        state=state, version=version, merge_sha=merge, run_id=run_id, run_attempt=run_attempt,
        packaging_policy_sha=packaging_policy_sha, artifact_id=artifact_id, artifact_digest=artifact_digest,
        helper_mode=helper_mode, helper_tag=helper_tag, helper_commit=helper_commit,
        artifact_name=artifact_name,
    )
    existing = local_ref_target(ref, cwd)
    if existing:
        verified = verify_artifact_receipt_commit(existing, state=state, version=version, merge_sha=merge, cwd=cwd)
        if verified["ref"] != ref:
            raise ReservationError("existing artifact receipt ref does not match its declared identity")
        if any(verified.get(key) != value for key, value in fields.items()):
            raise ReservationError("existing artifact receipt does not match the requested identity")
        return verified
    merge_info = local_commit_info(merge, cwd)
    commit = _git(
        "commit-tree", merge_info["tree"], "-p", merge, "-m", artifact_receipt_message(fields), cwd=cwd
    )
    update = subprocess.run(
        ["git", "update-ref", ref, commit, "0" * 40], cwd=cwd, text=True, capture_output=True
    )
    if update.returncode != 0:
        existing = local_ref_target(ref, cwd)
        if existing:
            verified = verify_artifact_receipt_commit(existing, state=state, version=version, merge_sha=merge, cwd=cwd)
            if verified["ref"] != ref:
                raise ReservationError("existing artifact receipt ref does not match its declared identity")
            if any(verified.get(key) != value for key, value in fields.items()):
                raise ReservationError("existing artifact receipt does not match the requested identity")
            return verified
        raise ReservationError(update.stderr.strip() or "artifact receipt ref creation failed")
    verified = verify_artifact_receipt_commit(commit, state=state, version=version, merge_sha=merge, cwd=cwd)
    if verified["ref"] != ref:
        raise ReservationError("created artifact receipt ref does not match its declared identity")
    return verified


def create_github_artifact_receipt(
    *, state: str, version: str, merge_sha: str, run_id: str, run_attempt: str,
    packaging_policy_sha: str, repository: str, token: str, artifact_id: str = "",
    artifact_digest: str = "", helper_mode: str = "pending", helper_tag: str = "",
    helper_commit: str = "", artifact_name: str = "release-assets",
    api_root: str = "https://api.github.com"
) -> dict[str, Any]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = artifact_receipt_ref(
        state,
        version,
        merge,
        run_id if state == "attempt" else "",
        run_attempt if state == "attempt" else "",
    )
    fields = artifact_receipt_fields(
        state=state, version=version, merge_sha=merge, run_id=run_id, run_attempt=run_attempt,
        packaging_policy_sha=packaging_policy_sha, artifact_id=artifact_id, artifact_digest=artifact_digest,
        helper_mode=helper_mode, helper_tag=helper_tag, helper_commit=helper_commit,
        artifact_name=artifact_name,
    )
    client = GitHubRefClient(repository, token, api_root)
    existing = client.ref_target(ref)
    if existing:
        info = client.commit_info(existing)
        merge_info = client.commit_info(merge)
        parents = [parent.get("sha") for parent in info.get("parents", [])]
        allowed_trees = {merge_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA}
        if parents != [merge] or info.get("tree", {}).get("sha") not in allowed_trees:
            raise ReservationError("existing remote artifact receipt provenance does not match its merge SHA")
        actual = trailers_from_message(str(info.get("message", "")))
        if any(actual.get(key) != value for key, value in fields.items()):
            raise ReservationError("existing remote artifact receipt does not match the requested identity")
        actual_ref = artifact_receipt_ref(
            state,
            version,
            merge,
            actual.get("Release-Run-Id", "") if state == "attempt" else "",
            actual.get("Release-Run-Attempt", "") if state == "attempt" else "",
        )
        if actual_ref != ref:
            raise ReservationError("existing remote artifact receipt ref does not match its declared identity")
        return {"ref": ref, "target": existing, **actual}
    tree = client.create_tree()
    commit = client.create_commit(tree=tree, parent=merge, message=artifact_receipt_message(fields))
    try:
        target = client.create_ref(ref, commit)
    except ReservationError:
        target = client.ref_target(ref)
        if not target:
            raise
    if target != commit:
        return create_github_artifact_receipt(
            state=state, version=version, merge_sha=merge, run_id=run_id, run_attempt=run_attempt,
            packaging_policy_sha=packaging_policy_sha, repository=repository, token=token,
            artifact_id=artifact_id, artifact_digest=artifact_digest, helper_mode=helper_mode,
            helper_tag=helper_tag, helper_commit=helper_commit, artifact_name=artifact_name, api_root=api_root,
        )
    actual_ref = artifact_receipt_ref(
        state,
        version,
        merge,
        fields.get("Release-Run-Id", "") if state == "attempt" else "",
        fields.get("Release-Run-Attempt", "") if state == "attempt" else "",
    )
    if actual_ref != ref:
        raise ReservationError("created remote artifact receipt ref does not match its declared identity")
    return {"ref": ref, "target": target, **fields}


def verify_local_artifact_receipt(
    *, state: str, version: str, merge_sha: str, run_id: str = "", run_attempt: str = "", cwd: Path = ROOT
) -> dict[str, Any]:
    ref = artifact_receipt_ref(state, version, merge_sha, run_id, run_attempt)
    target = local_ref_target(ref, cwd)
    if not target:
        raise ArtifactReceiptMissing(f"artifact receipt ref is missing: {ref}")
    result = verify_artifact_receipt_commit(target, state=state, version=version, merge_sha=merge_sha, cwd=cwd)
    if result["ref"] != ref:
        raise ReservationError("local artifact receipt ref does not match its declared identity")
    return result


def verify_github_artifact_receipt(
    *, state: str, version: str, merge_sha: str, repository: str, token: str,
    api_root: str = "https://api.github.com", run_id: str = "", run_attempt: str = ""
) -> dict[str, Any]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = artifact_receipt_ref(state, version, merge, run_id, run_attempt)
    client = GitHubRefClient(repository, token, api_root)
    target = client.ref_target(ref)
    if not target:
        raise ArtifactReceiptMissing(f"artifact receipt ref is missing: {ref}")
    info = client.commit_info(target)
    merge_info = client.commit_info(merge)
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    allowed_trees = {merge_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA}
    if parents != [merge] or info.get("tree", {}).get("sha") not in allowed_trees:
        raise ReservationError("remote artifact receipt provenance does not match its merge SHA")
    actual = trailers_from_message(str(info.get("message", "")))
    expected = artifact_receipt_fields(
        state=state,
        version=version,
        merge_sha=merge,
        run_id=actual.get("Release-Run-Id", ""),
        run_attempt=actual.get("Release-Run-Attempt", ""),
        packaging_policy_sha=actual.get("Release-Packaging-Policy-SHA", ""),
        artifact_id=actual.get("Release-Artifact-Id", ""),
        artifact_digest=actual.get("Release-Artifact-Digest", ""),
        helper_mode=actual.get("Artifact-Helper-Mode", ""),
        helper_tag=actual.get("Artifact-Helper-Tag", ""),
        helper_commit=actual.get("Artifact-Helper-Commit", ""),
        artifact_name=actual.get("Release-Artifact-Name", ""),
    )
    if any(actual.get(key) != value for key, value in expected.items()):
        raise ReservationError("remote artifact receipt does not match its declared identity")
    expected_ref = artifact_receipt_ref(
        state,
        version,
        merge,
        actual.get("Release-Run-Id", "") if state == "attempt" else "",
        actual.get("Release-Run-Attempt", "") if state == "attempt" else "",
    )
    if expected_ref != ref:
        raise ReservationError("remote artifact receipt ref does not match its declared identity")
    return {"ref": expected_ref, "target": target, **actual}


def verify_local_merge_identity(
    *, merge_sha: str, source_sha: str, version: str, channel: str,
    reservation: dict[str, Any], pr_head_sha: str | None = None, cwd: Path = ROOT
) -> None:
    merge = local_commit_info(merge_sha, cwd)
    parents = merge["parents"]
    if len(parents) == 2:
        head_shas = parents
    elif len(parents) == 1 and pr_head_sha:
        candidate = normalize_sha(pr_head_sha, "PR head SHA")
        head = local_commit_info(candidate, cwd)
        if head["tree"] != merge["tree"]:
            raise ReservationError("PR head tree does not match the squash merge tree")
        head_shas = [candidate]
    else:
        raise ReservationError("bound receipt merge SHA requires two parents or an associated PR head")
    expected = {
        "Product-Version": version,
        "Release-Channel": channel,
        "Release-Target": reservation["releaseTarget"],
        "Release-Policy": reservation["policy"],
        "Release-Reservation-Id": reservation["reservationId"],
        "Release-Reservation-Ref": reservation["ref"],
        "Release-Reservation-Owner": reservation["Reservation-Owner"],
        "Release-Claim-Key": reservation["Reservation-Claim-Key"],
        "Release-Boundary-Token": reservation["Reservation-Boundary-Token"],
    }
    for head_sha in head_shas:
        head = local_commit_info(head_sha, cwd)
        if head["tree"] != merge["tree"]:
            continue
        current = head
        for _ in range(256):
            parents = current["parents"]
            if len(parents) == 1:
                values = trailers_from_message(current["message"])
                if (
                    values.get("Release-Source-SHA") == parents[0]
                    and all(values.get(key) == value for key, value in expected.items())
                ):
                    reservation_source = values.get("Release-Reservation-Source-SHA")
                    if reservation_source:
                        if normalize_sha(reservation_source, "reservation source SHA") == reservation["sourceSha"]:
                            return
                    elif parents[0] == reservation["sourceSha"]:
                        return
            if not current["parents"]:
                break
            current = local_commit_info(current["parents"][0], cwd)
    raise ReservationError("merge SHA does not contain the matching prepared release identity")


def verify_github_merge_identity(
    *, client: GitHubRefClient, merge_sha: str, source_sha: str, version: str, channel: str,
    reservation: dict[str, Any], pr_head_sha: str | None = None
) -> None:
    merge = client.commit_info(merge_sha)
    parents = [parent.get("sha") for parent in merge.get("parents", [])]
    if any(not isinstance(parent, str) for parent in parents):
        raise ReservationError("bound receipt merge SHA has incomplete parent metadata")
    if len(parents) == 2:
        head_shas = parents
    elif len(parents) == 1 and pr_head_sha:
        candidate = normalize_sha(pr_head_sha, "PR head SHA")
        head = client.commit_info(candidate)
        if head.get("tree", {}).get("sha") != merge.get("tree", {}).get("sha"):
            raise ReservationError("PR head tree does not match the squash merge tree")
        head_shas = [candidate]
    else:
        raise ReservationError("bound receipt merge SHA requires two parents or an associated PR head")
    expected = {
        "Product-Version": version,
        "Release-Channel": channel,
        "Release-Target": reservation["releaseTarget"],
        "Release-Policy": reservation["policy"],
        "Release-Reservation-Id": reservation["reservationId"],
        "Release-Reservation-Ref": reservation["ref"],
        "Release-Reservation-Owner": reservation["Reservation-Owner"],
        "Release-Claim-Key": reservation["Reservation-Claim-Key"],
        "Release-Boundary-Token": reservation["Reservation-Boundary-Token"],
    }
    merge_tree = merge.get("tree", {}).get("sha")
    for head_sha in head_shas:
        head = client.commit_info(head_sha)
        if head.get("tree", {}).get("sha") != merge_tree:
            continue
        current = head
        for _ in range(256):
            current_parents = [parent.get("sha") for parent in current.get("parents", [])]
            if len(current_parents) == 1 and isinstance(current_parents[0], str):
                values = trailers_from_message(str(current.get("message", "")))
                if (
                    values.get("Release-Source-SHA") == current_parents[0]
                    and all(values.get(key) == value for key, value in expected.items())
                ):
                    reservation_source = values.get("Release-Reservation-Source-SHA")
                    if reservation_source:
                        if normalize_sha(reservation_source, "reservation source SHA") == reservation["sourceSha"]:
                            return
                    elif current_parents[0] == reservation["sourceSha"]:
                        return
            if not current_parents or not isinstance(current_parents[0], str):
                break
            current = client.commit_info(current_parents[0])
    raise ReservationError("merge SHA does not contain the matching prepared release identity")


def create_local_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, pr_head_sha: str | None = None, cwd: Path = ROOT
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    reservation = verify_local_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, cwd=cwd,
    )
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
        release_target=reservation["releaseTarget"], policy=reservation["policy"],
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
            channel=reservation["channel"], reservation=reservation, pr_head_sha=pr_head_sha, cwd=cwd,
        )
        create_local_decision(state="bound", merge_sha=merge, reservation=reservation, cwd=cwd)
    elif state == "consumed":
        verify_local_merge_identity(
            merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation, pr_head_sha=pr_head_sha, cwd=cwd,
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
                claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
                pr_head_sha=pr_head_sha, cwd=cwd
            )
        raise ReservationError(update.stderr.strip() or "receipt ref creation failed")
    return {"ref": ref, "target": commit, **fields}


def create_github_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, repository: str, token: str,
    api_root: str = "https://api.github.com", pr_head_sha: str | None = None
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    client = GitHubRefClient(repository, token, api_root)
    reservation = verify_github_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, repository=repository,
        token=token, api_root=api_root,
    )
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
        release_target=reservation["releaseTarget"], policy=reservation["policy"],
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
            repository=repository, token=token, api_root=api_root, pr_head_sha=pr_head_sha,
        )
        verify_github_decision_state(state="bound", merge_sha=merge, reservation=reservation, client=client)
    if state == "released" and (bound_target or consumed_target):
        raise ReservationError("released receipt is only valid for an unbound claim")
    if state == "bound":
        verify_github_merge_identity(
            client=client, merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation, pr_head_sha=pr_head_sha,
        )
        create_github_decision(state="bound", merge_sha=merge, reservation=reservation, client=client)
    elif state == "consumed":
        verify_github_merge_identity(
            client=client, merge_sha=merge, source_sha=reservation["sourceSha"], version=version,
            channel=reservation["channel"], reservation=reservation, pr_head_sha=pr_head_sha,
        )
    elif state == "released":
        create_github_decision(state="released", merge_sha=merge, reservation=reservation, client=client)
    existing = client.ref_target(ref)
    if existing:
        info = client.commit_info(existing)
        merge_info = client.commit_info(merge)
        parents = [parent.get("sha") for parent in info.get("parents", [])]
        allowed_trees = {merge_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA}
        if parents != [merge] or info.get("tree", {}).get("sha") not in allowed_trees:
            raise ReservationError("existing remote receipt provenance does not match its merge SHA")
        actual = trailers_from_message(str(info.get("message", "")))
        if any(actual.get(key) != value for key, value in fields.items()):
            raise ReservationError("existing remote receipt does not match the requested identity")
        return {"ref": ref, "target": existing, **fields}
    merge_info = client.commit_info(merge)
    tree = client.create_tree()
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
            repository=repository, token=token, api_root=api_root, pr_head_sha=pr_head_sha
        )
    return {"ref": ref, "target": target, **fields}


def verify_local_receipt(
    *, state: str, version: str, merge_sha: str, reservation_id: str, owner: str, claim_key: str,
    boundary_token: str, reservation_ref_value: str, pr_head_sha: str | None = None, cwd: Path = ROOT
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    reservation = verify_local_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, cwd=cwd,
    )
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
        release_target=reservation["releaseTarget"], policy=reservation["policy"],
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
            channel=reservation["channel"], reservation=reservation, pr_head_sha=pr_head_sha, cwd=cwd,
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
    api_root: str = "https://api.github.com", pr_head_sha: str | None = None
) -> dict[str, str]:
    merge = normalize_sha(merge_sha, "merge SHA")
    ref = receipt_ref(state, version, reservation_id if state == "released" else merge)
    reservation = verify_github_reservation_claim(
        reservation_ref_value=reservation_ref_value, version=version, reservation_id=reservation_id,
        owner=owner, claim_key=claim_key, boundary_token=boundary_token, repository=repository,
        token=token, api_root=api_root,
    )
    fields = receipt_fields(
        state=state, version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
        claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
        release_target=reservation["releaseTarget"], policy=reservation["policy"],
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
            channel=reservation["channel"], reservation=reservation, pr_head_sha=pr_head_sha,
        )
    if state == "bound":
        verify_github_decision_state(state="bound", merge_sha=merge, reservation=reservation, client=client)
    elif state == "consumed":
        if not bound_target:
            raise ReservationError("consumed receipt requires a matching bound receipt")
        verify_github_receipt(
            state="bound", version=version, merge_sha=merge, reservation_id=reservation_id, owner=owner,
            claim_key=claim_key, boundary_token=boundary_token, reservation_ref_value=reservation_ref_value,
            repository=repository, token=token, api_root=api_root, pr_head_sha=pr_head_sha,
        )
        verify_github_decision_state(state="bound", merge_sha=merge, reservation=reservation, client=client)
    elif state == "released":
        if bound_target or consumed_target:
            raise ReservationError("released receipt is only valid for an unbound claim")
        verify_github_decision_state(state="released", merge_sha=merge, reservation=reservation, client=client)
    info = client.commit_info(target)
    merge_info = client.commit_info(merge)
    parents = [parent.get("sha") for parent in info.get("parents", [])]
    allowed_trees = {merge_info.get("tree", {}).get("sha"), IDENTITY_METADATA_TREE_SHA}
    if parents != [merge] or info.get("tree", {}).get("sha") not in allowed_trees:
        raise ReservationError("remote receipt provenance does not match its merge SHA")
    actual = trailers_from_message(str(info.get("message", "")))
    if any(actual.get(key) != value for key, value in fields.items()):
        raise ReservationError("remote receipt does not match the requested identity")
    return {"ref": ref, "target": target, **fields}


def intent_snapshot(**values: Any) -> dict[str, Any]:
    required = {"version", "channel", "release_target", "policy", "merge_sha", "source_sha", "release_mode", "reservation"}
    missing = sorted(required - values.keys())
    if missing:
        raise ReservationError(f"release snapshot is missing fields: {', '.join(missing)}")
    normalize_sha(str(values["source_sha"]), "source SHA")
    normalize_sha(str(values["merge_sha"]), "merge SHA")
    version = str(values["version"])
    snapshot = {
        "schema_version": 2,
        "pull_request": values.get("pull_request"),
        "source_sha": values["source_sha"],
        "merge_sha": values["merge_sha"],
        "release_mode": values["release_mode"],
        "covered_merge_sha": values.get("covered_merge_sha", ""),
        "release_target": values["release_target"],
        "policy": values["policy"],
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
    reserve.add_argument("--release-target", required=True)
    reserve.add_argument("--release-policy", default=RELEASE_POLICY_ID)
    reserve.add_argument("--owner", required=True)
    reserve.add_argument("--claim-key", required=True)
    reserve.add_argument("--reservation-id")
    reserve.add_argument("--boundary-token")
    reserve.add_argument("--repository")
    reserve.add_argument("--token")
    reserve.add_argument("--api-root", default="https://api.github.com")
    reserve.add_argument("--local-root", type=Path)
    reserve.add_argument("--output", type=Path)
    retire = sub.add_parser("retire")
    retire.add_argument("--version", required=True)
    retire.add_argument("--reservation-id", required=True)
    retire.add_argument("--owner", default="github-actions[bot]")
    retire.add_argument("--reason", default="no-product-publication")
    retire.add_argument("--evidence-json", type=Path, required=True)
    retire.add_argument("--local-root", type=Path)
    retire.add_argument("--repository")
    retire.add_argument("--token")
    retire.add_argument("--api-root", default="https://api.github.com")
    retire.add_argument("--output", type=Path)
    find_claim = sub.add_parser("find-claim")
    find_claim.add_argument("--claim-key", required=True)
    find_claim.add_argument("--owner", required=True)
    find_claim.add_argument("--release-target", required=True)
    find_claim.add_argument("--release-policy", default=RELEASE_POLICY_ID)
    find_claim.add_argument("--local-root", type=Path)
    receipt = sub.add_parser("receipt")
    for option in ("state", "version", "merge-sha", "reservation-id", "owner", "claim-key", "boundary-token", "reservation-ref"):
        receipt.add_argument(f"--{option}", required=True)
    receipt.add_argument("--pr-head")
    receipt.add_argument("--maintainer-confirmed", action="store_true")
    receipt.add_argument("--local-root", type=Path)
    receipt.add_argument("--repository")
    receipt.add_argument("--token")
    receipt.add_argument("--api-root", default="https://api.github.com")
    receipt.add_argument("--output", type=Path)
    artifact = sub.add_parser("artifact-receipt")
    artifact.add_argument("--state", choices=("pending", "attempt", "finalized"), required=True)
    artifact.add_argument("--version", required=True)
    artifact.add_argument("--merge-sha", required=True)
    artifact.add_argument("--run-id", required=True)
    artifact.add_argument("--run-attempt", required=True)
    artifact.add_argument("--packaging-policy-sha", required=True)
    artifact.add_argument("--artifact-id", default="")
    artifact.add_argument("--artifact-digest", default="")
    artifact.add_argument("--helper-mode", default="pending")
    artifact.add_argument("--helper-tag", default="")
    artifact.add_argument("--helper-commit", default="")
    artifact.add_argument("--artifact-name", default="release-assets")
    artifact.add_argument("--local-root", type=Path)
    artifact.add_argument("--repository")
    artifact.add_argument("--token")
    artifact.add_argument("--api-root", default="https://api.github.com")
    artifact.add_argument("--output", type=Path)
    verify_artifact = sub.add_parser("verify-artifact-receipt")
    verify_artifact.add_argument("--state", choices=("pending", "attempt", "finalized"), required=True)
    verify_artifact.add_argument("--version", required=True)
    verify_artifact.add_argument("--merge-sha", required=True)
    verify_artifact.add_argument("--run-id")
    verify_artifact.add_argument("--run-attempt")
    verify_artifact.add_argument("--local-root", type=Path)
    verify_artifact.add_argument("--repository")
    verify_artifact.add_argument("--token")
    verify_artifact.add_argument("--api-root", default="https://api.github.com")
    snapshot = sub.add_parser("snapshot")
    snapshot.add_argument("--input", type=Path, required=True)
    snapshot.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "reserve":
            generated = deterministic_identity(args.owner, args.claim_key)
            reservation_id = args.reservation_id or generated["reservationId"]
            expected = {
                "ref": reservation_ref_for_claim(
                    version=args.version, reservation_id=reservation_id, owner=args.owner,
                    claim_key=args.claim_key, release_target=args.release_target, repository=args.repository,
                    token=args.token, api_root=args.api_root, cwd=args.local_root or ROOT,
                ),
                "sourceSha": normalize_sha(args.source_sha, "source SHA"),
                "version": args.version,
                "channel": args.channel,
                "releaseTarget": args.release_target,
                "policy": args.release_policy,
                "reservationId": reservation_id,
                "Reservation-Owner": args.owner,
                "Reservation-Claim-Key": args.claim_key,
                "Reservation-Boundary-Token": args.boundary_token or generated["boundaryToken"],
            }
            if args.repository and args.token:
                result = create_github_reservation(expected, repository=args.repository, token=args.token, api_root=args.api_root)
            else:
                result = create_local_reservation(expected, args.local_root or ROOT)
        elif args.command == "retire":
            evidence = json.loads(args.evidence_json.read_text(encoding="utf-8"))
            validate_retirement_evidence(evidence, args.version)
            if args.repository and args.token:
                result = create_github_retirement(
                    version=args.version, reservation_id=args.reservation_id, evidence=evidence,
                    repository=args.repository, token=args.token, api_root=args.api_root,
                    owner=args.owner, reason=args.reason,
                )
            else:
                result = create_local_retirement(
                    version=args.version, reservation_id=args.reservation_id, evidence=evidence,
                    owner=args.owner, reason=args.reason, cwd=args.local_root or ROOT,
                )
        elif args.command == "find-claim":
            result = find_local_reservation_by_claim(
                claim_key=args.claim_key,
                owner=args.owner,
                release_target=args.release_target,
                policy=args.release_policy,
                cwd=args.local_root or ROOT,
            )
            if result is None:
                return 2
        elif args.command == "receipt":
            if args.state == "released" and not args.maintainer_confirmed:
                raise ReservationError("released receipt requires explicit maintainer confirmation")
            if args.repository and args.token:
                result = create_github_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha, reservation_id=args.reservation_id,
                    owner=args.owner, claim_key=args.claim_key, boundary_token=args.boundary_token,
                    reservation_ref_value=args.reservation_ref, repository=args.repository, token=args.token,
                    api_root=args.api_root, pr_head_sha=args.pr_head,
                )
            else:
                result = create_local_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha, reservation_id=args.reservation_id,
                    owner=args.owner, claim_key=args.claim_key, boundary_token=args.boundary_token,
                    reservation_ref_value=args.reservation_ref, pr_head_sha=args.pr_head, cwd=args.local_root or ROOT
                )
        elif args.command == "artifact-receipt":
            if args.repository and args.token:
                result = create_github_artifact_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha, run_id=args.run_id,
                    run_attempt=args.run_attempt, packaging_policy_sha=args.packaging_policy_sha,
                    repository=args.repository, token=args.token, artifact_id=args.artifact_id,
                    artifact_digest=args.artifact_digest, helper_mode=args.helper_mode,
                    helper_tag=args.helper_tag, helper_commit=args.helper_commit, artifact_name=args.artifact_name,
                    api_root=args.api_root,
                )
            else:
                result = create_local_artifact_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha, run_id=args.run_id,
                    run_attempt=args.run_attempt, packaging_policy_sha=args.packaging_policy_sha,
                    artifact_id=args.artifact_id, artifact_digest=args.artifact_digest,
                    helper_mode=args.helper_mode, helper_tag=args.helper_tag, helper_commit=args.helper_commit,
                    artifact_name=args.artifact_name,
                    cwd=args.local_root or ROOT,
                )
        elif args.command == "verify-artifact-receipt":
            if args.repository and args.token:
                result = verify_github_artifact_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha,
                    repository=args.repository, token=args.token, api_root=args.api_root,
                    run_id=args.run_id or "", run_attempt=args.run_attempt or "",
                )
            else:
                result = verify_local_artifact_receipt(
                    state=args.state, version=args.version, merge_sha=args.merge_sha,
                    run_id=args.run_id or "", run_attempt=args.run_attempt or "", cwd=args.local_root or ROOT,
                )
        else:
            value = json.loads(args.input.read_text(encoding="utf-8"))
            write_json(args.output, intent_snapshot(**value))
            result = intent_snapshot(**value)
        if getattr(args, "output", None) and args.command != "snapshot":
            write_json(args.output, result)
        print(json.dumps(result, sort_keys=True))
        return 0
    except ArtifactReceiptMissing as exc:
        print(f"release_reservation.py: {exc}", file=sys.stderr)
        return 2
    except (ReservationError, OSError, json.JSONDecodeError) as exc:
        print(f"release_reservation.py: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
