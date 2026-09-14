#!/usr/bin/env python3
"""Resolve an authorization-stable Snapshot Access helper source."""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any


VERSION_RE = re.compile(
    r"^(?P<core>\d+\.\d+\.\d+)(?:-(?P<channel>beta|rc|dev)\.(?P<ordinal>[1-9]\d*))?$"
)
TAG_RE = re.compile(r"^v(?P<version>\d+\.\d+\.\d+(?:-(?:beta|rc|dev)\.[1-9]\d*)?)$")
SHA256_RE = re.compile(r"^[0-9a-fA-F]{64}$")
CDHASH_RE = re.compile(r"^[0-9a-fA-F]{40}$")
IDENTITY_FIELDS = ("sha256", "artifact_sha256", "cdhash", "designated_requirement")
COMPONENT_FIELDS = (
    "bundle_id",
    "relative_path",
    "binary",
    "component_version",
    "protocol_version",
    "reuse_policy",
)


class HelperResolutionError(ValueError):
    """Raised when an approved helper source cannot be proven."""


def _version(value: str) -> re.Match[str]:
    match = VERSION_RE.fullmatch(value)
    if match is None:
        raise HelperResolutionError(f"invalid product version: {value!r}")
    return match


def _tag_version(tag: str) -> str:
    match = TAG_RE.fullmatch(tag)
    if match is None:
        raise HelperResolutionError(f"invalid helper release tag: {tag!r}")
    return match.group("version")


def candidate_tags(version: str, bootstrap_release_tag: str, releases: list[dict[str, Any]]) -> list[str]:
    """Return preferred helper Release tags, independent of the product RC ordinal."""
    core = _version(version).group("core")
    preferred = (f"v{core}-rc.1", bootstrap_release_tag)
    candidates: list[str] = []

    def add(tag: str) -> None:
        if not tag:
            return
        _tag_version(tag)
        if tag not in candidates:
            candidates.append(tag)

    for tag in preferred:
        add(tag)

    discovered: list[tuple[str, str]] = []
    for release in releases:
        if not isinstance(release, dict):
            continue
        if release.get("isDraft") is not False or release.get("isPrerelease") is not True:
            continue
        tag = release.get("tagName")
        if not isinstance(tag, str) or TAG_RE.fullmatch(tag) is None:
            continue
        discovered.append((str(release.get("publishedAt") or ""), tag))
    for _, tag in sorted(discovered, key=lambda item: (item[0], item[1]), reverse=True):
        add(tag)
    return candidates


def resolve_helper(
    *, version: str, requested_mode: str, approved_source_tag: str | None = None
) -> dict[str, str]:
    """Resolve the final helper mode after candidate Release verification."""
    _version(version)
    if requested_mode not in {"auto", "bootstrap"}:
        raise HelperResolutionError(f"unsupported helper mode: {requested_mode}")
    if approved_source_tag:
        _tag_version(approved_source_tag)
        if requested_mode == "bootstrap":
            raise HelperResolutionError(
                f"approved helper Release exists: {approved_source_tag}; refusing bootstrap"
            )
        return {
            "version": version,
            "mode": "reuse",
            "source_tag": approved_source_tag,
            "reason": "approved-release-manifest",
        }
    if requested_mode == "bootstrap":
        return {
            "version": version,
            "mode": "bootstrap",
            "source_tag": "",
            "reason": "no-approved-helper-release",
        }
    raise HelperResolutionError(
        "no approved Snapshot Access helper Release; rerun same-SHA recovery with helper_mode=bootstrap"
    )


def validate_source_manifest(
    manifest: dict[str, Any], lock: dict[str, Any], source_tag: str
) -> dict[str, str]:
    """Verify the source Release manifest against the checked-in helper contract."""
    source_version = _tag_version(source_tag)
    if manifest.get("product") != lock.get("product"):
        raise HelperResolutionError("helper source manifest product does not match the lock")
    if manifest.get("signing") != lock.get("signing"):
        raise HelperResolutionError("helper source manifest signing does not match the lock")
    if manifest.get("release_version") != source_version:
        raise HelperResolutionError("helper source manifest version does not match its release tag")

    try:
        actual = manifest["components"]["snapshot_access"]
        expected = lock["components"]["snapshot_access"]
    except (KeyError, TypeError) as error:
        raise HelperResolutionError("helper source manifest is missing Snapshot Access metadata") from error
    if not isinstance(actual, dict) or not isinstance(expected, dict):
        raise HelperResolutionError("Snapshot Access component metadata must be objects")
    for field in COMPONENT_FIELDS:
        if actual.get(field) != expected.get(field):
            raise HelperResolutionError(f"Snapshot Access source component contract mismatch: {field}")

    locked_identity = expected.get("identity")
    if not isinstance(locked_identity, dict):
        raise HelperResolutionError("Snapshot Access lock is missing identity references")
    for field in IDENTITY_FIELDS:
        expected_pointer = f"BUILD-MANIFEST.json#/components/snapshot_access/{field}"
        if locked_identity.get(field) != expected_pointer:
            raise HelperResolutionError(f"Snapshot Access lock identity reference is invalid: {field}")
        value = actual.get(field)
        if not isinstance(value, str) or not value:
            raise HelperResolutionError(f"Snapshot Access source manifest is missing identity: {field}")
        if field in {"sha256", "artifact_sha256"} and SHA256_RE.fullmatch(value) is None:
            raise HelperResolutionError(f"Snapshot Access source manifest has invalid identity: {field}")
        if field == "cdhash" and CDHASH_RE.fullmatch(value) is None:
            raise HelperResolutionError("Snapshot Access source manifest has invalid identity: cdhash")

    return {field: str(actual[field]) for field in IDENTITY_FIELDS}


def _read_json(path: str) -> Any:
    if path == "-":
        return json.load(sys.stdin)
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    candidates = sub.add_parser("candidates")
    candidates.add_argument("--version", required=True)
    candidates.add_argument("--bootstrap-tag", default="")
    candidates.add_argument("--releases", default="-", help="JSON file, or - for stdin")

    resolve = sub.add_parser("resolve")
    resolve.add_argument("--version", required=True)
    resolve.add_argument("--requested-mode", choices=("auto", "bootstrap"), required=True)
    resolve.add_argument("--approved-source-tag")

    verify = sub.add_parser("verify-manifest")
    verify.add_argument("--manifest", required=True)
    verify.add_argument("--lock", required=True)
    verify.add_argument("--source-tag", required=True)

    args = parser.parse_args(argv)
    try:
        if args.command == "candidates":
            releases = _read_json(args.releases)
            if not isinstance(releases, list):
                raise HelperResolutionError("GitHub release listing must be an array")
            result: Any = {
                "version": args.version,
                "candidates": candidate_tags(args.version, args.bootstrap_tag, releases),
            }
        elif args.command == "resolve":
            result = resolve_helper(
                version=args.version,
                requested_mode=args.requested_mode,
                approved_source_tag=args.approved_source_tag,
            )
        else:
            manifest = _read_json(args.manifest)
            lock = _read_json(args.lock)
            if not isinstance(manifest, dict) or not isinstance(lock, dict):
                raise HelperResolutionError("helper manifest and lock must be JSON objects")
            result = validate_source_manifest(manifest, lock, args.source_tag)
        print(json.dumps(result, sort_keys=True))
        return 0
    except (HelperResolutionError, OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"release_helper.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
