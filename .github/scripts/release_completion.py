#!/usr/bin/env python3
"""Validate the Release completion merge gate and freeze identity provenance."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_chain as CHAIN  # noqa: E402
import release_reservation as RESERVATION  # noqa: E402


REQUIRED_SOURCE_CHECKS = {
    "Release intent label gate",
    "quality",
    "macOS Swift tests",
    "arm64 native package",
    "x86_64 native package",
    "Universal 2 assembly",
}


class CompletionError(RuntimeError):
    """Raised when a PR cannot become merge-ready."""


IDENTITY_REF_PREFIXES = (
    "refs/tags/release-reservation",
    "refs/tags/release-bound",
    "refs/tags/release-consumed",
    "refs/tags/release-released",
)
RELEASE_IDENTITY_TRAILERS = {
    "Release-Source-SHA",
    "Product-Version",
    "Release-Reservation-Id",
    "Release-Reservation-Ref",
    "Release-Reservation-Owner",
    "Release-Claim-Key",
    "Release-Boundary-Token",
    "Release-Provenance",
}


def labels(path: Path) -> dict[str, str]:
    try:
        return CHAIN.intent_from_labels(json.loads(path.read_text(encoding="utf-8")))
    except (OSError, json.JSONDecodeError) as error:
        raise CompletionError(f"cannot read labels: {error}") from error


def checks_ready(path: Path) -> bool:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CompletionError(f"cannot read check runs: {error}") from error
    rows = payload.get("check_runs", []) if isinstance(payload, dict) else []
    latest: dict[str, tuple[str, str | None]] = {}
    for row in rows:
        if not isinstance(row, dict) or not row.get("name"):
            continue
        timestamp = row.get("completed_at") or row.get("started_at") or ""
        previous = latest.get(row["name"])
        if previous is None or timestamp >= previous[0]:
            latest[row["name"]] = (timestamp, row.get("conclusion"))
    return all(latest.get(name, ("", None))[1] == "success" for name in REQUIRED_SOURCE_CHECKS)


def verify_migration(commit: str, base: str, version: str) -> None:
    if CHAIN.tree_path_exists(base, "VERSION"):
        raise CompletionError("migration is only allowed when base has no VERSION")
    if CHAIN.commit_version(commit) != version:
        raise CompletionError("migration VERSION does not match the approved baseline")
    changed = CHAIN.git("diff", "--name-only", f"{base}...{commit}").splitlines()
    if changed != ["VERSION"]:
        raise CompletionError("migration PR must add only VERSION")


def verify_no_existing_covered_identity(covered: str) -> None:
    product_tags = [row["tag"] for row in CHAIN.product_tags() if row.get("target") == covered]
    identity_refs: list[str] = []
    for ref in CHAIN.git("for-each-ref", "--format=%(refname)", *IDENTITY_REF_PREFIXES).splitlines():
        try:
            target = CHAIN.git("rev-parse", f"{ref}^{{commit}}")
            parents = CHAIN.git("show", "-s", "--format=%P", target).split()
        except CHAIN.ReleaseChainError:
            continue
        if target == covered or covered in parents or ref.endswith(f"/{covered}"):
            identity_refs.append(ref)
    if product_tags or identity_refs:
        found = ", ".join(product_tags + identity_refs)
        raise CompletionError(f"covered merge already has a release identity: {found}")


def verify_squash_merge_proof(path: Path, covered: str, repository: str | None) -> None:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CompletionError(f"cannot read covered merge proof: {error}") from error
    if not isinstance(payload, list):
        raise CompletionError("covered merge proof must be a GitHub pull request array")
    for row in payload:
        if not isinstance(row, dict) or row.get("merge_commit_sha") != covered or not row.get("merged_at"):
            continue
        base = row.get("base")
        base_repo = base.get("repo") if isinstance(base, dict) else None
        if not isinstance(base, dict) or base.get("ref") != "main":
            continue
        if repository and (not isinstance(base_repo, dict) or base_repo.get("full_name") != repository):
            continue
        return
    raise CompletionError("covered single-parent commit is not an authoritative merged PR result")


def verify_version_only_covered_merge(
    covered: str, current_main: str, proof_path: Path | None = None, repository: str | None = None
) -> None:
    if not CHAIN.SHA_RE.fullmatch(covered):
        raise CompletionError("covered merge SHA must be a full commit SHA")
    parents = CHAIN.git("show", "-s", "--format=%P", covered).split()
    if len(parents) not in {1, 2}:
        raise CompletionError("covered merge SHA must identify a mainline commit")
    if not CHAIN.is_ancestor(covered, current_main):
        raise CompletionError("covered merge SHA must belong to the current mainline ancestry")
    verify_no_existing_covered_identity(covered)
    covered_trailers = CHAIN.trailers(covered)
    if len(parents) == 1:
        if proof_path is None:
            raise CompletionError("single-parent covered merge requires authoritative PR proof")
        verify_squash_merge_proof(proof_path, covered, repository)
        try:
            CHAIN.verify_prepared(covered)
        except CHAIN.ReleaseChainError:
            if RELEASE_IDENTITY_TRAILERS.intersection(covered_trailers):
                raise CompletionError("covered single-parent commit has incomplete release identity")
        else:
            raise CompletionError("covered single-parent commit already has a release identity")
    else:
        if CHAIN.verify_merged(covered).get("prepared") == "true":
            raise CompletionError("covered merge already has a release identity")


def verify_reservation(
    path: Path, prepared: dict[str, str], *, repository: str | None, token: str | None, api_root: str
) -> None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CompletionError(f"cannot read reservation JSON: {error}") from error
    if not isinstance(value, dict):
        raise CompletionError("reservation JSON must be an object")
    expected = {
        "sourceSha": prepared["sourceSha"],
        "version": prepared["version"],
        "channel": prepared["channel"].removeprefix("channel:"),
        "reservationId": prepared["reservationId"],
        "ref": prepared["reservationRef"],
        "Reservation-Owner": prepared["reservationOwner"],
        "Reservation-Claim-Key": prepared["claimKey"],
        "Reservation-Boundary-Token": prepared["boundaryToken"],
    }
    if any(value.get(key) != expected_value for key, expected_value in expected.items()):
        raise CompletionError("reservation JSON does not match preparation provenance")
    try:
        if repository and token:
            RESERVATION.verify_github_reservation(
                expected, repository=repository, token=token, api_root=api_root
            )
        else:
            RESERVATION.verify_local_reservation_claim(
                reservation_ref_value=expected["ref"], version=expected["version"],
                reservation_id=expected["reservationId"], owner=expected["Reservation-Owner"],
                claim_key=expected["Reservation-Claim-Key"],
                boundary_token=expected["Reservation-Boundary-Token"], cwd=CHAIN.ROOT,
            )
    except RESERVATION.ReservationError as error:
        raise CompletionError(f"reservation provenance is not verified: {error}") from error


def verify_github_verification(path: Path, commit: str) -> None:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise CompletionError(f"cannot read GitHub commit verification: {error}") from error
    if not isinstance(payload, dict) or payload.get("sha") != commit:
        raise CompletionError("GitHub commit verification does not match the preparation commit")
    verification = payload.get("commit", {}).get("verification") if isinstance(payload.get("commit"), dict) else None
    if not isinstance(verification, dict) or verification.get("verified") is not True:
        raise CompletionError("preparation commit is not GitHub-verified")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--labels-json", type=Path, required=True)
    parser.add_argument("--checks-json", type=Path, required=True)
    parser.add_argument("--release-mode", choices=("normal", "version-only-release-pr"), default="normal")
    parser.add_argument("--covered-merge-sha", default="")
    parser.add_argument("--reservation-json", type=Path)
    parser.add_argument("--repository")
    parser.add_argument("--token")
    parser.add_argument("--api-root", default="https://api.github.com")
    parser.add_argument("--require-github-verification", action="store_true")
    parser.add_argument("--github-verification-json", type=Path)
    parser.add_argument("--covered-merge-proof-json", type=Path)
    parser.add_argument("--allow-migration", action="store_true")
    parser.add_argument("--migration-version")
    args = parser.parse_args(argv)
    try:
        if args.repo_root:
            CHAIN.ROOT = args.repo_root.resolve()
        intent = labels(args.labels_json)
        changed = CHAIN.git("diff", "--name-only", f"{args.base}...{args.commit}").splitlines()
        if intent["action"] == "skip":
            if "VERSION" in changed:
                if args.allow_migration and args.migration_version:
                    verify_migration(args.commit, args.base, args.migration_version)
                    print(json.dumps({"status": "migration"}, sort_keys=True))
                    return 0
                raise CompletionError("non-migration skip PR must not modify VERSION")
            print(json.dumps({"status": "skip"}, sort_keys=True))
            return 0
        if not checks_ready(args.checks_json):
            raise CompletionError("source PR checks are not all successful")
        prepared = CHAIN.verify_prepared(args.commit)
        if args.require_github_verification:
            if not args.github_verification_json:
                raise CompletionError("production completion requires GitHub commit verification evidence")
            verify_github_verification(args.github_verification_json, args.commit)
            if prepared["provenance"] != "github-native-verified":
                raise CompletionError("production completion requires a GitHub-native verified preparation commit")
        if not args.reservation_json:
            raise CompletionError("product release completion requires reservation provenance")
        verify_reservation(
            args.reservation_json, prepared, repository=args.repository, token=args.token, api_root=args.api_root
        )
        if prepared["type"] != intent["type"] or prepared["channel"] != intent["channel"]:
            raise CompletionError("preparation intent does not match current PR labels")
        if prepared["mode"] != args.release_mode:
            raise CompletionError("release mode does not match preparation provenance")
        if CHAIN.git("merge-base", args.base, prepared["sourceSha"]) != CHAIN.git("rev-parse", args.base):
            raise CompletionError("preparation source is not based on current main")
        if args.release_mode == "version-only-release-pr":
            covered = args.covered_merge_sha or prepared["coveredMergeSha"]
            if not covered or covered != prepared["coveredMergeSha"]:
                raise CompletionError("version-only-release-pr covered merge SHA is not frozen")
            verify_version_only_covered_merge(
                covered, args.base, args.covered_merge_proof_json, args.repository
            )
            if changed != ["VERSION"]:
                raise CompletionError("version-only-release-pr must be a non-empty VERSION-only PR")
        print(json.dumps({"status": "ready", **prepared}, sort_keys=True))
        return 0
    except (CompletionError, CHAIN.ReleaseChainError, CHAIN.PRODUCT_VERSION.VersionError) as error:
        print(f"release_completion.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
