#!/usr/bin/env python3
"""Validate the TelevyBackup VERSION-only release chain."""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
RESOLVER_PATH = ROOT / "scripts/product-version.py"
SPEC = importlib.util.spec_from_file_location("product_version", RESOLVER_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"unable to load {RESOLVER_PATH}")
PRODUCT_VERSION = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PRODUCT_VERSION)

VALID_TYPES = {"type:patch", "type:minor", "type:major", "type:docs", "type:skip"}
PRODUCT_TYPES = VALID_TYPES - {"type:docs", "type:skip"}
VALID_CHANNELS = {"channel:stable", "channel:rc"}


class ReleaseChainError(RuntimeError):
    """Raised when a release chain invariant is violated."""


def git(*args: str, check: bool = True) -> str:
    result = subprocess.run(["git", *args], cwd=ROOT, text=True, capture_output=True)
    if check and result.returncode != 0:
        raise ReleaseChainError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout.strip()


def git_raw(*args: str, check: bool = True) -> str:
    result = subprocess.run(["git", *args], cwd=ROOT, text=True, capture_output=True)
    if check and result.returncode != 0:
        raise ReleaseChainError(result.stderr.strip() or f"git {' '.join(args)} failed")
    return result.stdout


def tree_path_exists(commit: str, path: str) -> bool:
    result = subprocess.run(
        ["git", "cat-file", "-e", f"{commit}:{path}"],
        cwd=ROOT,
        text=True,
        capture_output=True,
    )
    return result.returncode == 0


def release_action(type_label: str, channel: str) -> str:
    if type_label not in VALID_TYPES:
        raise ReleaseChainError(f"unsupported release intent type: {type_label}")
    if channel not in VALID_CHANNELS:
        raise ReleaseChainError(f"unsupported release intent channel: {channel}")
    if type_label in {"type:docs", "type:skip"}:
        return "skip"
    return "automatic" if type_label == "type:patch" and channel == "channel:stable" else "exact"


def intent_from_labels(labels: object) -> dict[str, str]:
    if not isinstance(labels, list):
        raise ReleaseChainError("PR labels must be an array")
    names = sorted({row.get("name", "") for row in labels if isinstance(row, dict)})
    types = [name for name in names if name.startswith("type:")]
    channels = [name for name in names if name.startswith("channel:")]
    unknown_types = sorted(set(types) - VALID_TYPES)
    unknown_channels = sorted(set(channels) - VALID_CHANNELS)
    if unknown_types or unknown_channels:
        raise ReleaseChainError(f"unknown release labels: {unknown_types + unknown_channels}")
    if len(types) != 1 or len(channels) != 1:
        raise ReleaseChainError("PR must have exactly one type:* and one channel:* label")
    components = sorted(name for name in names if name.startswith("component:"))
    return {
        "type": types[0],
        "channel": channels[0],
        "components": ",".join(components) if components else "none",
        "action": release_action(types[0], channels[0]),
    }


def commit_parent(commit: str) -> str:
    parents = git("show", "-s", "--format=%P", commit).split()
    if len(parents) != 1:
        raise ReleaseChainError(f"preparation commit {commit} must have exactly one parent")
    return parents[0]


def diff_names(commit: str) -> list[str]:
    parent = commit_parent(commit)
    return git("diff", "--name-only", f"{parent}..{commit}").splitlines()


def trailers(commit: str) -> dict[str, str]:
    raw = git("show", "-s", "--format=%(trailers:only,unfold)", commit)
    values: dict[str, str] = {}
    for line in raw.splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            values[key.strip()] = value.strip()
    return values


def commit_version(commit: str) -> str:
    contents = git_raw("show", f"{commit}:VERSION")
    return PRODUCT_VERSION.read_version_from_text(contents) if hasattr(PRODUCT_VERSION, "read_version_from_text") else _read_version_text(contents)


def _read_version_text(contents: str) -> str:
    match = PRODUCT_VERSION.VERSION_RE.fullmatch(contents)
    if match is None:
        raise ReleaseChainError("VERSION must contain exactly one valid semver line ending in LF")
    return match.group("core") + (f"-rc.{match.group('rc')}" if match.group("rc") else "")


def prepared_intent(commit: str) -> dict[str, str]:
    values = trailers(commit)
    type_label = values.get("Release-Intent-Type", "")
    channel = values.get("Release-Intent-Channel", "")
    action = values.get("Release-Intent-Action", "")
    components = values.get("Release-Intent-Components", "none")
    expected_action = release_action(type_label, f"channel:{channel}" if not channel.startswith("channel:") else channel)
    normalized_channel = channel if channel.startswith("channel:") else f"channel:{channel}"
    if type_label not in PRODUCT_TYPES or normalized_channel not in VALID_CHANNELS:
        raise ReleaseChainError("preparation commit is missing a valid release intent")
    if action != expected_action:
        raise ReleaseChainError("preparation commit has an invalid Release-Intent-Action")
    return {
        "type": type_label,
        "channel": normalized_channel,
        "action": action,
        "components": components,
    }


def verify_prepared(commit: str, source_sha: str | None = None, expected_version: str | None = None) -> dict[str, str]:
    release_sha = git("rev-parse", f"{commit}^{{commit}}")
    parent = commit_parent(release_sha)
    if source_sha and parent != git("rev-parse", f"{source_sha}^{{commit}}"):
        raise ReleaseChainError(f"preparation parent is {parent}, expected {source_sha}")
    if diff_names(release_sha) != ["VERSION"]:
        raise ReleaseChainError("preparation commit must modify only VERSION")
    version = commit_version(release_sha)
    if expected_version and version != expected_version:
        raise ReleaseChainError(f"preparation VERSION is {version}, expected {expected_version}")
    commit_trailers = trailers(release_sha)
    if commit_trailers.get("Release-Source-SHA") != parent:
        raise ReleaseChainError("Release-Source-SHA must match the preparation parent")
    if commit_trailers.get("Product-Version") != version:
        raise ReleaseChainError("Product-Version must match VERSION")
    values = {"releaseSha": release_sha, "sourceSha": parent, "version": version, "tag": f"v{version}"}
    values.update(prepared_intent(release_sha))
    return values


def verify_merged(commit: str) -> dict[str, str]:
    merge_sha = git("rev-parse", f"{commit}^{{commit}}")
    parents = git("show", "-s", "--format=%P", merge_sha).split()
    if len(parents) != 2:
        return {"prepared": "false", "reason": "not_merge_commit"}
    merge_parent, preparation_sha = parents
    if subprocess.run(
        ["git", "diff", "--quiet", merge_sha, f"{merge_sha}^2"], cwd=ROOT
    ).returncode != 0:
        return {"prepared": "false", "reason": "merge_tree_differs_from_preparation"}
    prep_trailers = trailers(preparation_sha)
    if not ("Release-Source-SHA" in prep_trailers or "Product-Version" in prep_trailers):
        return {"prepared": "false", "reason": "no_prepared_product_merge"}
    source_sha = prep_trailers.get("Release-Source-SHA", "")
    if not source_sha or not prep_trailers.get("Product-Version"):
        raise ReleaseChainError("preparation identity trailers are incomplete")
    if not is_ancestor(merge_parent, source_sha):
        raise ReleaseChainError("preparation source is not based on merged main parent")
    values = verify_prepared(preparation_sha, source_sha)
    values.update({"prepared": "true", "mergeSha": merge_sha, "preparationSha": preparation_sha})
    return values


def is_ancestor(ancestor: str, descendant: str) -> bool:
    return subprocess.run(["git", "merge-base", "--is-ancestor", ancestor, descendant], cwd=ROOT).returncode == 0


def tag_target(tag: str) -> str | None:
    if subprocess.run(["git", "show-ref", "--tags", "--verify", "--quiet", f"refs/tags/{tag}"], cwd=ROOT).returncode:
        return None
    return git("rev-parse", f"refs/tags/{tag}^{{commit}}")


def verify_tag(version: str, expected_sha: str | None = None, allow_existing: bool = False) -> dict[str, str]:
    PRODUCT_VERSION.parse_version(version)
    tag = f"v{version}"
    target = tag_target(tag)
    if target is None:
        return {"tag": tag, "status": "available"}
    expected = git("rev-parse", f"{expected_sha}^{{commit}}") if expected_sha else None
    if allow_existing and expected and target == expected:
        return {"tag": tag, "status": "matching", "target": target}
    raise ReleaseChainError(f"product tag {tag} is already owned by {target}")


def strictly_newer(candidate: str, current: str) -> bool:
    left = PRODUCT_VERSION.parse_version(candidate)
    right = PRODUCT_VERSION.parse_version(current)
    left_core = tuple(int(left[key]) for key in ("major", "minor", "patch"))
    right_core = tuple(int(right[key]) for key in ("major", "minor", "patch"))
    if left_core != right_core:
        return left_core > right_core
    left_rc, right_rc = left["rc"], right["rc"]
    if left_rc is None:
        return right_rc is not None
    return right_rc is not None and int(left_rc) > int(right_rc)


def stage(args: argparse.Namespace) -> None:
    source_sha = git("rev-parse", "HEAD")
    if source_sha != args.source_sha:
        raise ReleaseChainError(f"checked out source is {source_sha}, expected {args.source_sha}")
    if git("status", "--porcelain"):
        raise ReleaseChainError("source checkout must be clean before preparation")
    current = PRODUCT_VERSION.read_version(ROOT / "VERSION")
    if args.mode == "automatic":
        version = PRODUCT_VERSION.next_patch(current)
    elif args.mode == "exact" and args.exact_version:
        version = args.exact_version
        parsed = PRODUCT_VERSION.parse_version(version)
        if args.expected_channel and (("rc" if parsed["prerelease"] else "stable") != args.expected_channel):
            raise ReleaseChainError("VERSION channel does not match release intent")
        if not strictly_newer(version, current):
            raise ReleaseChainError("exact VERSION must be newer than current VERSION")
    else:
        raise ReleaseChainError("exact mode requires --exact-version")
    verify_tag(version)
    (ROOT / "VERSION").write_text(version + "\n", encoding="utf-8")
    if git("diff", "--name-only") != "VERSION":
        raise ReleaseChainError("preparation staging may modify only VERSION")
    subprocess.run(["git", "add", "VERSION"], cwd=ROOT, check=True)
    metadata = [
        f"Release-Source-SHA: {source_sha}",
        f"Product-Version: {version}",
        f"Release-Intent-Type: {args.intent_type}",
        f"Release-Intent-Channel: {args.intent_channel}",
        f"Release-Intent-Action: {args.intent_action}",
        f"Release-Intent-Components: {args.intent_components or 'none'}",
    ]
    subprocess.run(
        ["git", "commit", "--signoff", "-m", f"chore(release): v{version}", "-m", "\n".join(metadata)],
        cwd=ROOT,
        check=True,
    )
    print(json.dumps(verify_prepared(git("rev-parse", "HEAD"), source_sha), sort_keys=True))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prepared = sub.add_parser("verify-prepared")
    prepared.add_argument("--commit", default="HEAD")
    prepared.add_argument("--source-sha")
    prepared.add_argument("--version")
    merged = sub.add_parser("verify-merged")
    merged.add_argument("--commit", default="HEAD")
    tag = sub.add_parser("verify-tag")
    tag.add_argument("--version", required=True)
    tag.add_argument("--expected-sha")
    tag.add_argument("--allow-existing", action="store_true")
    stage_parser = sub.add_parser("stage")
    stage_parser.add_argument("--source-sha", required=True)
    stage_parser.add_argument("--mode", choices=("automatic", "exact"), required=True)
    stage_parser.add_argument("--exact-version")
    stage_parser.add_argument("--expected-channel")
    stage_parser.add_argument("--intent-type", required=True)
    stage_parser.add_argument("--intent-channel", required=True)
    stage_parser.add_argument("--intent-action", required=True)
    stage_parser.add_argument("--intent-components", default="none")
    args = parser.parse_args(argv)
    try:
        if args.command == "verify-prepared":
            print(json.dumps(verify_prepared(args.commit, args.source_sha, args.version), sort_keys=True))
        elif args.command == "verify-merged":
            print(json.dumps(verify_merged(args.commit), sort_keys=True))
        elif args.command == "verify-tag":
            print(json.dumps(verify_tag(args.version, args.expected_sha, args.allow_existing), sort_keys=True))
        else:
            stage(args)
        return 0
    except (ReleaseChainError, PRODUCT_VERSION.VersionError) as error:
        print(f"release_chain.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
