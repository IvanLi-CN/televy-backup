#!/usr/bin/env python3
"""Validate and allocate the TelevyBackup release identity chain."""

from __future__ import annotations

import argparse
import functools
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
VALID_CHANNELS = {"channel:prod", "channel:beta", "channel:rc", "channel:dev"}
LEGACY_LABELS = {"channel:stable", "channel:canary", "type:none"}
PRODUCT_TAG_RE = re.compile(
    r"^v(?P<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:beta|rc|dev)\.[1-9]\d*)?)$"
)
IDENTITY_REF_RE = re.compile(
    r"^refs/tags/release-(?:reservation|decision|bound|consumed|released)/v(?P<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:beta|rc|dev)\.[1-9]\d*)?)(?:/|$)"
)
SHA_RE = re.compile(r"^[0-9a-fA-F]{40}$")
PRODUCT_TAG_OWNER = "protected-release-automation"
PRODUCT_TAGGER = "github-actions[bot]"
PRODUCT_TAGGER_EMAIL = "41898282+github-actions[bot]@users.noreply.github.com"


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


def canonical_sha(value: str, label: str = "SHA") -> str:
    if not SHA_RE.fullmatch(value):
        raise ReleaseChainError(f"invalid {label}: {value!r}")
    return value.lower()


def tree_path_exists(commit: str, path: str) -> bool:
    result = subprocess.run(
        ["git", "cat-file", "-e", f"{commit}:{path}"], cwd=ROOT, text=True, capture_output=True
    )
    return result.returncode == 0


def normalize_label(value: str, prefix: str) -> str:
    return value if value.startswith(prefix) else f"{prefix}{value}"


def release_action(type_label: str, channel: str | None = None) -> str:
    if type_label not in VALID_TYPES:
        raise ReleaseChainError(f"unsupported release intent type: {type_label}")
    if type_label in {"type:docs", "type:skip"}:
        if channel:
            raise ReleaseChainError("docs/skip release intents must not have a channel")
        return "skip"
    if not channel:
        raise ReleaseChainError("product release intent requires a channel")
    normalized = normalize_label(channel, "channel:")
    if normalized not in VALID_CHANNELS:
        raise ReleaseChainError(f"unsupported release intent channel: {channel}")
    return "allocate"


def intent_from_labels(labels: object) -> dict[str, str]:
    if not isinstance(labels, list):
        raise ReleaseChainError("PR labels must be an array")
    names = sorted({row.get("name", "") for row in labels if isinstance(row, dict)})
    types = [name for name in names if name.startswith("type:")]
    channels = [name for name in names if name.startswith("channel:")]
    unknown_types = sorted(set(types) - VALID_TYPES)
    unknown_channels = sorted(set(channels) - VALID_CHANNELS)
    legacy = sorted(set(types + channels) & LEGACY_LABELS)
    if unknown_types or unknown_channels or legacy:
        raise ReleaseChainError(
            f"unknown or legacy release labels: {unknown_types + unknown_channels + legacy}"
        )
    if len(types) != 1:
        raise ReleaseChainError("PR must have exactly one type:* label")
    type_label = types[0]
    if type_label in {"type:docs", "type:skip"}:
        if channels:
            raise ReleaseChainError("docs/skip release intents must not have a channel")
        channel = ""
    else:
        if len(channels) != 1:
            raise ReleaseChainError("product PR must have exactly one channel:* label")
        channel = channels[0]
    components = sorted(name for name in names if name.startswith("component:"))
    return {
        "type": type_label,
        "channel": channel,
        "components": ",".join(components) if components else "none",
        "action": release_action(type_label, channel or None),
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
    try:
        return PRODUCT_VERSION.read_version_from_text(contents)
    except PRODUCT_VERSION.VersionError as error:
        raise ReleaseChainError(str(error)) from error


def prepared_intent(commit: str) -> dict[str, str]:
    values = trailers(commit)
    type_label = values.get("Release-Intent-Type", "")
    channel = values.get("Release-Intent-Channel", "")
    action = values.get("Release-Intent-Action", "")
    components = values.get("Release-Intent-Components", "none")
    normalized_channel = normalize_label(channel, "channel:") if channel else ""
    expected_action = release_action(type_label, normalized_channel or None)
    if type_label not in PRODUCT_TYPES or normalized_channel not in VALID_CHANNELS:
        raise ReleaseChainError("preparation commit is missing a valid product release intent")
    if action != expected_action:
        raise ReleaseChainError("preparation commit has an invalid Release-Intent-Action")
    return {
        "type": type_label,
        "channel": normalized_channel,
        "action": action,
        "components": components,
    }


def verify_provenance(commit: str, version: str, parent: str) -> dict[str, str]:
    values = trailers(commit)
    if values.get("Release-Source-SHA") != parent:
        raise ReleaseChainError("Release-Source-SHA must match the preparation parent")
    if values.get("Product-Version") != version:
        raise ReleaseChainError("Product-Version must match VERSION")
    mode = values.get("Release-Mode", "normal")
    if mode not in {"normal", "version-only-release-pr"}:
        raise ReleaseChainError(f"unsupported release mode: {mode}")
    required = {
        "Release-Reservation-Id": "reservation id",
        "Release-Reservation-Ref": "reservation ref",
        "Release-Reservation-Owner": "reservation owner",
        "Release-Claim-Key": "claim key",
        "Release-Boundary-Token": "boundary token",
    }
    for key, label in required.items():
        if not values.get(key):
            raise ReleaseChainError(f"preparation provenance is missing {label}")
    if values.get("Release-Provenance") not in {"github-native-verified", "fixture-verified"}:
        raise ReleaseChainError("preparation provenance is not verified")
    reservation_source = values.get("Release-Reservation-Source-SHA", parent)
    reservation_source = canonical_sha(reservation_source, "reservation source SHA")
    if not is_ancestor(reservation_source, parent):
        raise ReleaseChainError("reservation source must be an ancestor of the preparation source")
    if mode == "version-only-release-pr" and not values.get("Release-Covered-Merge-SHA"):
        raise ReleaseChainError("version-only-release-pr must record one covered merge SHA")
    return {
        "mode": mode,
        "reservationId": values["Release-Reservation-Id"],
        "reservationRef": values["Release-Reservation-Ref"],
        "reservationOwner": values["Release-Reservation-Owner"],
        "claimKey": values["Release-Claim-Key"],
        "boundaryToken": values["Release-Boundary-Token"],
        "provenance": values["Release-Provenance"],
        "reservationSourceSha": reservation_source,
        "coveredMergeSha": values.get("Release-Covered-Merge-SHA", ""),
    }


def verify_prepared(
    commit: str, source_sha: str | None = None, expected_version: str | None = None
) -> dict[str, str]:
    release_sha = git("rev-parse", f"{commit}^{{commit}}")
    parent = commit_parent(release_sha)
    if source_sha and parent != git("rev-parse", f"{source_sha}^{{commit}}"):
        raise ReleaseChainError(f"preparation parent is {parent}, expected {source_sha}")
    if diff_names(release_sha) != ["VERSION"]:
        raise ReleaseChainError("preparation commit must modify only VERSION")
    version = commit_version(release_sha)
    if expected_version and version != expected_version:
        raise ReleaseChainError(f"preparation VERSION is {version}, expected {expected_version}")
    intent = prepared_intent(release_sha)
    provenance = verify_provenance(release_sha, version, parent)
    parsed = PRODUCT_VERSION.parse_version(version)
    if parsed["channel"] != intent["channel"].removeprefix("channel:"):
        raise ReleaseChainError("VERSION prerelease channel does not match release intent")
    values = {
        "releaseSha": release_sha,
        "sourceSha": parent,
        "version": version,
        "tag": f"v{version}",
    }
    values.update(intent)
    values.update(provenance)
    return values


def find_prepared(commit: str, base: str | None = None) -> dict[str, str]:
    """Find the current PR's preparation commit while preserving its identity."""
    release_sha = git("rev-parse", f"{commit}^{{commit}}")
    current_version = commit_version(release_sha)
    revisions = [release_sha]
    if base:
        base_sha = git("rev-parse", f"{base}^{{commit}}")
        revisions = git("rev-list", "--first-parent", f"{base_sha}..{release_sha}").splitlines()
    else:
        revisions = git("rev-list", "--first-parent", release_sha).splitlines()
    for candidate in revisions:
        try:
            prepared = verify_prepared(candidate)
        except (ReleaseChainError, PRODUCT_VERSION.VersionError):
            continue
        if prepared["version"] != current_version:
            raise ReleaseChainError(
                "current VERSION does not match the preparation commit"
            )
        prepared["preparationSha"] = prepared["releaseSha"]
        return prepared
    raise ReleaseChainError("no valid preparation commit found in the current PR ancestry")


def verify_merged(commit: str) -> dict[str, str]:
    merge_sha = git("rev-parse", f"{commit}^{{commit}}")
    if git("rev-parse", "--is-shallow-repository") == "true":
        raise ReleaseChainError("full repository history is required to verify a product merge")
    parents = git("show", "-s", "--format=%P", merge_sha).split()
    if len(parents) != 2:
        return {"prepared": "false", "reason": "not_merge_commit"}
    merge_parent, pr_head_sha = parents
    if subprocess.run(["git", "diff", "--quiet", merge_sha, pr_head_sha], cwd=ROOT).returncode != 0:
        return {"prepared": "false", "reason": "merge_tree_differs_from_preparation"}
    try:
        prepared = find_prepared(pr_head_sha, merge_parent)
    except (ReleaseChainError, PRODUCT_VERSION.VersionError):
        return {"prepared": "false", "reason": "no_prepared_product_merge"}
    preparation_sha = prepared["preparationSha"]
    source_sha = prepared["sourceSha"]
    if not is_ancestor(merge_parent, source_sha):
        raise ReleaseChainError("preparation source is not based on merged main parent")
    values = verify_prepared(preparation_sha, source_sha)
    values.update({"prepared": "true", "mergeSha": merge_sha, "preparationSha": preparation_sha})
    return values


def is_ancestor(ancestor: str, descendant: str) -> bool:
    return subprocess.run(
        ["git", "merge-base", "--is-ancestor", ancestor, descendant], cwd=ROOT
    ).returncode == 0


def tag_target(tag: str) -> str | None:
    if subprocess.run(
        ["git", "show-ref", "--tags", "--verify", "--quiet", f"refs/tags/{tag}"], cwd=ROOT
    ).returncode:
        return None
    return git("rev-parse", f"refs/tags/{tag}^{{commit}}")


def verify_product_tag_provenance(tag: str) -> dict[str, str]:
    ref = f"refs/tags/{tag}"
    if git("cat-file", "-t", ref, check=False) != "tag":
        raise ReleaseChainError(
            f"product tag {tag} is missing {PRODUCT_TAG_OWNER} annotated-tag provenance"
        )
    raw = git_raw("cat-file", "-p", ref)
    lines = raw.splitlines()
    object_type = next((line.split(" ", 1)[1] for line in lines if line.startswith("type ")), "")
    tagger = next((line.removeprefix("tagger ") for line in lines if line.startswith("tagger ")), "")
    if object_type != "commit" or not tagger.startswith(f"{PRODUCT_TAGGER} <{PRODUCT_TAGGER_EMAIL}>"):
        raise ReleaseChainError(
            f"product tag {tag} has foreign or incomplete {PRODUCT_TAG_OWNER} provenance"
        )
    return {"owner": PRODUCT_TAG_OWNER, "tagger": tagger}


def verify_tag(version: str, expected_sha: str | None = None, allow_existing: bool = False) -> dict[str, str]:
    PRODUCT_VERSION.parse_version(version)
    tag = f"v{version}"
    target = tag_target(tag)
    if target is None:
        return {"tag": tag, "status": "available"}
    provenance = verify_product_tag_provenance(tag)
    expected = git("rev-parse", f"{expected_sha}^{{commit}}") if expected_sha else None
    if allow_existing and expected and target == expected:
        return {"tag": tag, "status": "matching", "target": target, **provenance}
    raise ReleaseChainError(f"product tag {tag} is already owned by {target}")


def product_tag_provenance(tag: str, version: str) -> dict[str, str]:
    ref = f"refs/tags/{tag}"
    if git("cat-file", "-t", ref, check=False) != "tag":
        parsed = PRODUCT_VERSION.parse_version(version)
        if parsed["kind"] is None:
            raise ReleaseChainError(
                f"product tag {tag} is missing {PRODUCT_TAG_OWNER} annotated-tag provenance"
            )
        return {"owner": "legacy-prerelease", "tagger": ""}
    return verify_product_tag_provenance(tag)


def compare_versions(left: str, right: str) -> int:
    left_parsed = PRODUCT_VERSION.parse_version(left)
    right_parsed = PRODUCT_VERSION.parse_version(right)
    left_core = tuple(int(left_parsed[key]) for key in ("major", "minor", "patch"))
    right_core = tuple(int(right_parsed[key]) for key in ("major", "minor", "patch"))
    if left_core != right_core:
        return (left_core > right_core) - (left_core < right_core)
    left_kind, right_kind = left_parsed["kind"], right_parsed["kind"]
    if left_kind is None or right_kind is None:
        return (left_kind is None) - (right_kind is None)
    if left_kind != right_kind:
        ranks = {"beta": 0, "rc": 1, "dev": 2}
        return (ranks[left_kind] > ranks[right_kind]) - (ranks[left_kind] < ranks[right_kind])
    return (int(left_parsed["ordinal"]) > int(right_parsed["ordinal"])) - (
        int(left_parsed["ordinal"]) < int(right_parsed["ordinal"])
    )


def mainline_sha() -> str:
    for ref in ("refs/remotes/origin/main", "refs/heads/main"):
        value = git("rev-parse", f"{ref}^{{commit}}", check=False)
        if SHA_RE.fullmatch(value):
            return value
    return git("rev-parse", "HEAD^{commit}")


def product_tags() -> list[dict[str, str]]:
    mainline = mainline_sha()
    values: list[dict[str, str]] = []
    for tag in git("tag", "--list", "v*").splitlines():
        match = PRODUCT_TAG_RE.fullmatch(tag)
        if match is None:
            continue
        version = match.group("version")
        target = tag_target(tag)
        if not target:
            raise ReleaseChainError(f"product tag {tag} has no commit target")
        if not is_ancestor(target, mainline):
            raise ReleaseChainError(f"product tag {tag} targets an unreachable commit")
        provenance = product_tag_provenance(tag, version)
        values.append({"tag": tag, "version": version, "target": target, **provenance})
    return values


def occupied_identity_versions() -> list[str]:
    """Return versions already claimed by any append-only release identity ref."""
    prefixes = (
        "refs/tags/release-reservation",
        "refs/tags/release-decision",
        "refs/tags/release-bound",
        "refs/tags/release-consumed",
        "refs/tags/release-released",
    )
    refs = git("for-each-ref", "--format=%(refname)", *prefixes).splitlines()
    versions = []
    for ref in refs:
        match = IDENTITY_REF_RE.fullmatch(ref)
        if match:
            versions.append(match.group("version"))
    return sorted(set(versions), key=functools.cmp_to_key(compare_versions))


def final_tag_baseline(tags: list[dict[str, str]]) -> str:
    finals = [row["version"] for row in tags if PRODUCT_VERSION.parse_version(row["version"])["kind"] is None]
    return max(finals, key=functools.cmp_to_key(compare_versions), default="0.0.0")


def allocate_version(
    tags: list[dict[str, str]], type_label: str, channel_label: str, occupied_versions: list[str] | None = None,
    allow_occupied_version: str | None = None,
) -> dict[str, str | int]:
    type_name = type_label.removeprefix("type:")
    channel = channel_label.removeprefix("channel:")
    release_action(type_label, channel_label)
    baseline = final_tag_baseline(tags)
    base = PRODUCT_VERSION.next_base(baseline, type_name)
    if channel == "prod":
        version = base
        ordinal: int | None = None
        occupied = set(occupied_versions or []) | {row["version"] for row in tags}
        if allow_occupied_version:
            occupied.discard(allow_occupied_version)
        if version in occupied:
            raise ReleaseChainError(
                f"final release identity v{version} is already reserved or tagged; refusing to allocate a successor"
            )
    else:
        occupied = list(occupied_versions or []) + [row["version"] for row in tags]
        if allow_occupied_version:
            occupied = [value for value in occupied if value != allow_occupied_version]
        ordinals = []
        for value in occupied:
            try:
                parsed = PRODUCT_VERSION.parse_version(value)
            except PRODUCT_VERSION.VersionError:
                continue
            if parsed["kind"] == channel and ".".join(str(parsed[key]) for key in ("major", "minor", "patch")) == base:
                ordinals.append(int(parsed["ordinal"]))
        ordinal = max(ordinals, default=0) + 1
        version = PRODUCT_VERSION.format_release_version(base, channel, ordinal)
    return {
        "version": version,
        "baseVersion": base,
        "baselineVersion": baseline,
        "channel": channel,
        "type": type_name,
        "ordinal": ordinal or 0,
    }


def verify_release_sequence(version: str, expected_sha: str) -> dict[str, str]:
    PRODUCT_VERSION.parse_version(version)
    expected = git("rev-parse", f"{expected_sha}^{{commit}}")
    candidate_tag = f"v{version}"
    candidate_target = tag_target(candidate_tag)
    tags = product_tags()
    highest_final = final_tag_baseline(tags)
    candidate_core = PRODUCT_VERSION.parse_version(version)
    relation = compare_versions(
        ".".join(str(candidate_core[key]) for key in ("major", "minor", "patch")), highest_final
    )
    if relation < 0 or (relation == 0 and candidate_core["kind"] is not None):
        raise ReleaseChainError(f"superseded_by_product_tag: {candidate_tag} is below v{highest_final}")
    if relation == 0 and candidate_core["kind"] is None and candidate_target != expected:
        raise ReleaseChainError(
            f"product_tag_conflict: {candidate_tag} points to {candidate_target or 'no commit'}, expected {expected}"
        )
    if candidate_target is not None and candidate_target != expected:
        raise ReleaseChainError(
            f"product_tag_conflict: {candidate_tag} points to {candidate_target}, expected {expected}"
        )
    if candidate_target is not None:
        verify_product_tag_provenance(candidate_tag)
    return {
        "status": "matching" if candidate_target is not None else "available",
        "tag": candidate_tag,
        "version": version,
        "expectedSha": expected,
        "highestFinalTag": f"v{highest_final}",
        "highestFinalVersion": highest_final,
    }


def _stage_version(args: argparse.Namespace) -> str:
    if getattr(args, "version", None):
        version = args.version
        allocation = allocate_version(
            product_tags(), args.intent_type, args.intent_channel, occupied_identity_versions(),
            allow_occupied_version=version,
        )
        if version != allocation["version"]:
            raise ReleaseChainError(
                f"requested version {version} does not match final-tag-first allocation {allocation['version']}"
            )
        return version
    if args.mode in {"automatic", "allocate"}:
        return str(
            allocate_version(
                product_tags(), args.intent_type, args.intent_channel, occupied_identity_versions()
            )["version"]
        )
    raise ReleaseChainError("allocation mode requires a final-tag-first version")


def stage(args: argparse.Namespace) -> None:
    source_sha = git("rev-parse", "HEAD")
    if source_sha != args.source_sha:
        raise ReleaseChainError(f"checked out source is {source_sha}, expected {args.source_sha}")
    if git("status", "--porcelain"):
        raise ReleaseChainError("source checkout must be clean before preparation")
    version = _stage_version(args)
    verify_tag(version)
    (ROOT / "VERSION").write_text(version + "\n", encoding="utf-8")
    if git("diff", "--name-only") != "VERSION":
        raise ReleaseChainError("preparation staging may modify only VERSION")
    subprocess.run(["git", "add", "VERSION"], cwd=ROOT, check=True)
    reservation_source_sha = getattr(args, "reservation_source_sha", source_sha)
    metadata = [
        f"Release-Source-SHA: {source_sha}",
        f"Release-Reservation-Source-SHA: {reservation_source_sha}",
        f"Product-Version: {version}",
        f"Release-Intent-Type: {args.intent_type}",
        f"Release-Intent-Channel: {args.intent_channel}",
        f"Release-Intent-Action: {args.intent_action}",
        f"Release-Intent-Components: {args.intent_components or 'none'}",
        f"Release-Mode: {getattr(args, 'release_mode', 'normal')}",
        f"Release-Reservation-Id: {args.reservation_id}",
        f"Release-Reservation-Ref: {args.reservation_ref}",
        f"Release-Reservation-Owner: {args.reservation_owner}",
        f"Release-Claim-Key: {args.claim_key}",
        f"Release-Boundary-Token: {args.boundary_token}",
        f"Release-Provenance: {getattr(args, 'provenance', 'fixture-verified')}",
    ]
    reservation_source_sha = getattr(args, "reservation_source_sha", "")
    if reservation_source_sha and reservation_source_sha != source_sha:
        if not is_ancestor(reservation_source_sha, source_sha):
            raise ReleaseChainError("reservation source must be an ancestor of the preparation source")
        metadata.append(f"Release-Reservation-Source-SHA: {reservation_source_sha}")
    if getattr(args, "covered_merge_sha", ""):
        metadata.append(f"Release-Covered-Merge-SHA: {args.covered_merge_sha}")
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
    found = sub.add_parser("find-prepared")
    found.add_argument("--commit", default="HEAD")
    found.add_argument("--base")
    merged = sub.add_parser("verify-merged")
    merged.add_argument("--commit", default="HEAD")
    tag = sub.add_parser("verify-tag")
    tag.add_argument("--version", required=True)
    tag.add_argument("--expected-sha")
    tag.add_argument("--allow-existing", action="store_true")
    sequence = sub.add_parser("verify-release-sequence")
    sequence.add_argument("--version", required=True)
    sequence.add_argument("--expected-sha", required=True)
    provenance = sub.add_parser("verify-tag-provenance")
    provenance.add_argument("--tag", required=True)
    allocation = sub.add_parser("allocate-version")
    allocation.add_argument("--type", dest="intent_type", required=True)
    allocation.add_argument("--channel", dest="intent_channel", required=True)
    stage_parser = sub.add_parser("stage")
    stage_parser.add_argument("--source-sha", required=True)
    stage_parser.add_argument("--mode", choices=("automatic", "allocate", "exact"), required=True)
    stage_parser.add_argument("--version")
    stage_parser.add_argument("--intent-type", required=True)
    stage_parser.add_argument("--intent-channel", required=True)
    stage_parser.add_argument("--intent-action", required=True)
    stage_parser.add_argument("--intent-components", default="none")
    stage_parser.add_argument("--release-mode", default="normal")
    stage_parser.add_argument("--reservation-id", required=True)
    stage_parser.add_argument("--reservation-ref", required=True)
    stage_parser.add_argument("--claim-key", required=True)
    stage_parser.add_argument("--boundary-token", required=True)
    stage_parser.add_argument("--covered-merge-sha", default="")
    stage_parser.add_argument("--provenance", default="fixture-verified")
    args = parser.parse_args(argv)
    try:
        if args.command == "verify-prepared":
            print(json.dumps(verify_prepared(args.commit, args.source_sha, args.version), sort_keys=True))
        elif args.command == "find-prepared":
            print(json.dumps(find_prepared(args.commit, args.base), sort_keys=True))
        elif args.command == "verify-merged":
            print(json.dumps(verify_merged(args.commit), sort_keys=True))
        elif args.command == "verify-tag":
            print(json.dumps(verify_tag(args.version, args.expected_sha, args.allow_existing), sort_keys=True))
        elif args.command == "verify-release-sequence":
            print(json.dumps(verify_release_sequence(args.version, args.expected_sha), sort_keys=True))
        elif args.command == "verify-tag-provenance":
            print(json.dumps(verify_product_tag_provenance(args.tag), sort_keys=True))
        elif args.command == "allocate-version":
            print(
                json.dumps(
                    allocate_version(
                        product_tags(), args.intent_type, args.intent_channel, occupied_identity_versions()
                    ),
                    sort_keys=True,
                )
            )
        else:
            stage(args)
        return 0
    except (ReleaseChainError, PRODUCT_VERSION.VersionError) as error:
        print(f"release_chain.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
