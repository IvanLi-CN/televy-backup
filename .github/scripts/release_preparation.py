#!/usr/bin/env python3
"""Stage one VERSION-only preparation commit for an in-repository PR."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import release_chain as CHAIN  # noqa: E402


REQUIRED_SOURCE_CHECKS = {
    "Validate PR labels",
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


def source_is_ready(repo_root: Path, source_sha: str, base_sha: str) -> None:
    source = CHAIN.git("rev-parse", f"{source_sha}^{{commit}}")
    base = CHAIN.git("rev-parse", f"{base_sha}^{{commit}}")
    if source != source_sha:
        raise PreparationError(f"checked out source is {source}, expected {source_sha}")
    if CHAIN.git("merge-base", base, source) != base:
        raise PreparationError("PR source is not based on current main")
    changed = CHAIN.git("diff", "--name-only", f"{base}...{source}").splitlines()
    if "VERSION" in changed:
        raise PreparationError("source commits must not modify VERSION before preparation")


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
    if not source_checks_ready(read_json(args.checks_json)):
        output({"prepared": "waiting", "release_action": intent["action"], "source_sha": args.source_sha}, args.github_output)
        return

    try:
        existing = CHAIN.verify_prepared(args.source_sha)
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
                "source_sha": existing["sourceSha"],
                "version": existing["version"],
                "tag": existing["tag"],
            },
            args.github_output,
        )
        return

    if not CHAIN.tree_path_exists(args.base_sha, "VERSION"):
        changed = CHAIN.git("diff", "--name-only", f"{args.base_sha}...{args.source_sha}").splitlines()
        if changed == ["VERSION"] and CHAIN.commit_version(args.source_sha) == "0.9.2":
            output(
                {"prepared": "migration", "release_action": "skip", "source_sha": args.source_sha},
                args.github_output,
            )
            return

    source_is_ready(repo_root, args.source_sha, args.base_sha)
    if intent["action"] == "skip":
        output({"prepared": "not_required", "release_action": "skip", "source_sha": args.source_sha}, args.github_output)
        return
    if args.mode == "automatic" and intent["action"] == "exact":
        output({"prepared": "waiting_for_exact", "release_action": "exact", "source_sha": args.source_sha}, args.github_output)
        return
    if args.mode != intent["action"]:
        raise PreparationError(f"labels require {intent['action']} preparation, got {args.mode}")
    if args.mode == "exact" and not args.exact_version:
        raise PreparationError("exact preparation requires --exact-version")

    CHAIN.stage(
        argparse.Namespace(
            source_sha=args.source_sha,
            mode=args.mode,
            exact_version=args.exact_version,
            expected_channel=intent["channel"].removeprefix("channel:"),
            intent_type=intent["type"],
            intent_channel=intent["channel"],
            intent_action=intent["action"],
            intent_components=intent["components"],
        )
    )
    prepared = CHAIN.verify_prepared(CHAIN.git("rev-parse", "HEAD"), args.source_sha)
    output(
        {
            "prepared": "created",
            "release_action": prepared["action"],
            "release_sha": prepared["releaseSha"],
            "source_sha": prepared["sourceSha"],
            "version": prepared["version"],
            "tag": prepared["tag"],
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
    parser.add_argument("--mode", choices=("automatic", "exact"), required=True)
    parser.add_argument("--exact-version")
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
