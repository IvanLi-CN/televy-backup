#!/usr/bin/env python3
"""Validate the manual release target boundary for a pull request."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_chain as CHAIN  # noqa: E402


class TargetGateError(RuntimeError):
    """Raised when a PR contains an invalid manual release boundary."""


def validate(commit: str, base: str) -> dict[str, str]:
    commit_sha = CHAIN.git("rev-parse", f"{commit}^{{commit}}")
    base_sha = CHAIN.git("rev-parse", f"{base}^{{commit}}")
    changed = CHAIN.git("diff", "--name-only", f"{base_sha}...{commit_sha}").splitlines()
    try:
        prepared = CHAIN.find_prepared(commit_sha, base_sha)
    except CHAIN.ReleaseChainError:
        if "VERSION" in changed:
            raise TargetGateError("VERSION changed without a verified manual release preparation")
        return {"status": "not-release", "commitSha": commit_sha, "baseSha": base_sha}
    if CHAIN.git("merge-base", base_sha, prepared["sourceSha"]) != base_sha:
        raise TargetGateError("preparation source is not based on the trusted PR base")
    verified = CHAIN.verify_prepared(
        prepared["preparationSha"], prepared["sourceSha"], expected_target=prepared["releaseTarget"]
    )
    return {
        "status": "ready",
        "commitSha": commit_sha,
        "baseSha": base_sha,
        "preparationSha": verified["preparationSha"] if "preparationSha" in verified else verified["releaseSha"],
        "releaseTarget": verified["releaseTarget"],
        "policy": verified["policy"],
        "version": verified["version"],
        "channel": verified["channel"],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--base", required=True)
    args = parser.parse_args(argv)
    try:
        if args.repo_root:
            CHAIN.ROOT = args.repo_root.resolve()
        print(json.dumps(validate(args.commit, args.base), sort_keys=True))
        return 0
    except (TargetGateError, CHAIN.ReleaseChainError, CHAIN.PRODUCT_VERSION.VersionError) as error:
        print(f"release_target_gate.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
