#!/usr/bin/env python3
"""Validate the Release completion required PR check."""

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
    payload = json.loads(path.read_text(encoding="utf-8"))
    rows = payload.get("check_runs", []) if isinstance(payload, dict) else []
    outcomes = {row.get("name"): row.get("conclusion") for row in rows if isinstance(row, dict)}
    return all(outcomes.get(name) == "success" for name in REQUIRED_SOURCE_CHECKS)


def verify_migration(commit: str, base: str, version: str) -> None:
    if CHAIN.tree_path_exists(base, "VERSION"):
        raise CompletionError("migration is only allowed when base has no VERSION")
    if CHAIN.commit_version(commit) != version:
        raise CompletionError("migration VERSION does not match the approved baseline")
    changed = CHAIN.git("diff", "--name-only", f"{base}...{commit}").splitlines()
    if "VERSION" not in changed:
        raise CompletionError("migration PR must add VERSION")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--labels-json", type=Path, required=True)
    parser.add_argument("--checks-json", type=Path, required=True)
    parser.add_argument("--allow-migration", action="store_true")
    parser.add_argument("--migration-version")
    args = parser.parse_args(argv)
    try:
        if args.repo_root:
            CHAIN.ROOT = args.repo_root.resolve()
        intent = labels(args.labels_json)
        changed = CHAIN.git("diff", "--name-only", f"{args.base}...{args.commit}").splitlines()
        if intent["action"] == "skip" and "VERSION" not in changed:
            print(json.dumps({"status": "skip"}, sort_keys=True))
            return 0
        if not checks_ready(args.checks_json):
            raise CompletionError("source PR checks are not all successful")
        try:
            prepared = CHAIN.verify_prepared(args.commit)
        except CHAIN.ReleaseChainError:
            if args.allow_migration and args.migration_version:
                verify_migration(args.commit, args.base, args.migration_version)
                print(json.dumps({"status": "migration"}, sort_keys=True))
                return 0
            if intent["action"] == "skip":
                if "VERSION" in changed:
                    raise CompletionError("non-migration skip PR must not modify VERSION")
                print(json.dumps({"status": "skip"}, sort_keys=True))
                return 0
            raise
        if prepared["type"] != intent["type"] or prepared["channel"] != intent["channel"]:
            raise CompletionError("preparation intent does not match current PR labels")
        if CHAIN.git("merge-base", args.base, prepared["sourceSha"]) != CHAIN.git("rev-parse", args.base):
            raise CompletionError("preparation source is not based on current main")
        print(json.dumps({"status": "ready", **prepared}, sort_keys=True))
        return 0
    except (CompletionError, CHAIN.ReleaseChainError, CHAIN.PRODUCT_VERSION.VersionError, json.JSONDecodeError) as error:
        print(f"release_completion.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
