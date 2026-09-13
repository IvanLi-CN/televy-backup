#!/usr/bin/env python3
"""Validate the Release completion merge gate and freeze identity provenance."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_chain as CHAIN  # noqa: E402


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


def verify_version_only_covered_merge(covered: str) -> None:
    if not CHAIN.SHA_RE.fullmatch(covered):
        raise CompletionError("covered merge SHA must be a full commit SHA")
    identity = CHAIN.verify_merged(covered)
    if identity.get("prepared") == "true":
        raise CompletionError("covered merge already has a release identity")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--labels-json", type=Path, required=True)
    parser.add_argument("--checks-json", type=Path, required=True)
    parser.add_argument("--release-mode", choices=("normal", "version-only-release-pr"), default="normal")
    parser.add_argument("--covered-merge-sha", default="")
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
            verify_version_only_covered_merge(covered)
            if changed != ["VERSION"]:
                raise CompletionError("version-only-release-pr must be a non-empty VERSION-only PR")
        print(json.dumps({"status": "ready", **prepared}, sort_keys=True))
        return 0
    except (CompletionError, CHAIN.ReleaseChainError, CHAIN.PRODUCT_VERSION.VersionError) as error:
        print(f"release_completion.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
