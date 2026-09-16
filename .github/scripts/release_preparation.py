#!/usr/bin/env python3
"""Stage a VERSION-only preparation commit after identity reservation."""

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


class PreparationError(RuntimeError):
    """Raised when a preparation request is not safe to stage."""


def read_json(path: Path) -> object:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PreparationError(f"cannot read JSON {path}: {error}") from error


def latest_check_outcomes(payload: object) -> dict[str, str]:
    rows = payload.get("check_runs") if isinstance(payload, dict) else None
    if not isinstance(rows, list):
        raise PreparationError("check-runs JSON must contain check_runs")
    outcomes: dict[str, tuple[str, str]] = {}
    for index, row in enumerate(rows):
        if not isinstance(row, dict) or not isinstance(row.get("name"), str):
            continue
        conclusion = row.get("conclusion")
        if not isinstance(conclusion, str):
            continue
        stamp = str(row.get("completed_at") or row.get("started_at") or "")
        candidate = (f"{stamp}:{index:06d}", conclusion)
        name = row["name"]
        if name not in outcomes or candidate[0] >= outcomes[name][0]:
            outcomes[name] = candidate
    return {name: result for name, (_, result) in outcomes.items()}


def source_checks_ready(payload: object) -> bool:
    outcomes = latest_check_outcomes(payload)
    return all(outcomes.get(name) == "success" for name in REQUIRED_SOURCE_CHECKS)


def source_is_ready(repo_root: Path, source_sha: str, base_sha: str, release_mode: str) -> None:
    source = CHAIN.git("rev-parse", f"{source_sha}^{{commit}}")
    base = CHAIN.git("rev-parse", f"{base_sha}^{{commit}}")
    if source != source_sha:
        raise PreparationError(f"checked out source is {source}, expected {source_sha}")
    if CHAIN.git("merge-base", base, source) != base:
        raise PreparationError("PR source is not based on current main")
    changed = CHAIN.git("diff", "--name-only", f"{base}...{source}").splitlines()
    if release_mode == "normal" and "VERSION" in changed:
        raise PreparationError("normal source commits must not modify VERSION before preparation")
    if release_mode == "version-only-release-pr" and changed != ["VERSION"]:
        raise PreparationError("version-only-release-pr must change only VERSION")


def output(values: dict[str, str], path: str | None) -> None:
    if path:
        with Path(path).open("a", encoding="utf-8") as handle:
            for key, value in values.items():
                handle.write(f"{key}={value}\n")
    print(json.dumps(values, sort_keys=True))


def prepare(args: argparse.Namespace) -> None:
    repo_root = args.repo_root.resolve()
    if not repo_root.is_dir():
        raise PreparationError(f"repository worktree does not exist: {repo_root}")
    CHAIN.ROOT = repo_root
    labels = read_json(args.labels_json)
    intent = CHAIN.intent_from_labels(labels)
    release_mode = args.release_mode
    if intent["action"] == "skip":
        output({"prepared": "not_required", "release_action": "skip", "source_sha": args.source_sha}, args.github_output)
        return
    if not source_checks_ready(read_json(args.checks_json)):
        output({"prepared": "waiting", "release_action": intent["action"], "source_sha": args.source_sha}, args.github_output)
        return
    try:
        existing = CHAIN.find_prepared(args.source_sha, args.base_sha)
    except CHAIN.ReleaseChainError:
        existing = None
    if existing is not None:
        if existing["type"] != intent["type"] or existing["channel"] != intent["channel"]:
            raise PreparationError("existing preparation intent does not match current PR labels")
        output(
            {
                "prepared": "existing",
                "release_action": existing["action"],
                "release_sha": existing["releaseSha"],
                "preparation_sha": existing["preparationSha"],
                "source_sha": existing["sourceSha"],
                "version": existing["version"],
                "tag": existing["tag"],
                "reservation_id": existing["reservationId"],
                "reservation_ref": existing["reservationRef"],
                "boundary_token": existing["boundaryToken"],
            },
            args.github_output,
        )
        return
    if not args.reservation_json:
        raise PreparationError("preparation requires a completed reservation JSON")
    reservation = read_json(args.reservation_json)
    if not isinstance(reservation, dict):
        raise PreparationError("reservation JSON must be an object")
    required = ("version", "reservationId", "ref", "Reservation-Owner", "Reservation-Claim-Key", "Reservation-Boundary-Token")
    if any(not reservation.get(key) for key in required):
        raise PreparationError("reservation JSON is missing immutable identity fields")
    source_is_ready(repo_root, args.source_sha, args.base_sha, release_mode)
    reservation_channel = str(reservation.get("channel", ""))
    expected_channel = intent["channel"].removeprefix("channel:")
    if reservation.get("sourceSha") != args.source_sha or reservation_channel != expected_channel:
        raise PreparationError("reservation source or channel does not match the release intent")
    reservation_expected = {
        "ref": reservation["ref"],
        "sourceSha": args.source_sha,
        "version": reservation["version"],
        "channel": expected_channel,
        "reservationId": reservation["reservationId"],
        "Reservation-Owner": reservation["Reservation-Owner"],
        "Reservation-Claim-Key": reservation["Reservation-Claim-Key"],
        "Reservation-Boundary-Token": reservation["Reservation-Boundary-Token"],
    }
    try:
        if args.repository and args.token:
            RESERVATION.verify_github_reservation(
                reservation_expected, repository=args.repository, token=args.token, api_root=args.api_root
            )
        else:
            RESERVATION.verify_local_reservation_claim(
                reservation_ref_value=reservation_expected["ref"], version=reservation_expected["version"],
                reservation_id=reservation_expected["reservationId"], owner=reservation_expected["Reservation-Owner"],
                claim_key=reservation_expected["Reservation-Claim-Key"],
                boundary_token=reservation_expected["Reservation-Boundary-Token"], cwd=repo_root,
            )
    except RESERVATION.ReservationError as error:
        raise PreparationError(f"reservation provenance is not verified: {error}") from error
    if release_mode == "version-only-release-pr" and not args.covered_merge_sha:
        raise PreparationError("version-only-release-pr requires one covered merge SHA")
    if release_mode == "version-only-release-pr" and args.covered_merge_sha == args.source_sha:
        raise PreparationError("version-only-release-pr cannot cover its own source SHA")
    stage_args = argparse.Namespace(
        source_sha=args.source_sha,
        mode="exact",
        version=reservation["version"],
        intent_type=intent["type"],
        intent_channel=intent["channel"],
        intent_action=intent["action"],
        intent_components=intent["components"],
        release_mode=release_mode,
        reservation_id=reservation["reservationId"],
        reservation_ref=reservation["ref"],
        reservation_owner=reservation["Reservation-Owner"],
        claim_key=reservation["Reservation-Claim-Key"],
        boundary_token=reservation["Reservation-Boundary-Token"],
        covered_merge_sha=args.covered_merge_sha or "",
        provenance=args.provenance,
    )
    CHAIN.stage(stage_args)
    prepared = CHAIN.verify_prepared(CHAIN.git("rev-parse", "HEAD"), args.source_sha)
    output(
        {
            "prepared": "created",
            "release_action": prepared["action"],
            "release_sha": prepared["releaseSha"],
            "source_sha": prepared["sourceSha"],
            "version": prepared["version"],
            "tag": prepared["tag"],
            "reservation_id": prepared["reservationId"],
            "reservation_ref": prepared["reservationRef"],
            "boundary_token": prepared["boundaryToken"],
        },
        args.github_output,
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--base-sha", required=True)
    parser.add_argument("--labels-json", type=Path, required=True)
    parser.add_argument("--checks-json", type=Path, required=True)
    parser.add_argument("--mode", choices=("automatic", "allocate", "exact"), default="allocate")
    parser.add_argument("--exact-version")
    parser.add_argument("--release-mode", choices=("normal", "version-only-release-pr"), default="normal")
    parser.add_argument("--covered-merge-sha", default="")
    parser.add_argument("--reservation-json", type=Path)
    parser.add_argument("--provenance", default="fixture-verified")
    parser.add_argument("--repository")
    parser.add_argument("--token")
    parser.add_argument("--api-root", default="https://api.github.com")
    parser.add_argument("--github-output")
    args = parser.parse_args(argv)
    try:
        prepare(args)
        return 0
    except (PreparationError, CHAIN.ReleaseChainError, CHAIN.PRODUCT_VERSION.VersionError) as error:
        print(f"release_preparation.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
