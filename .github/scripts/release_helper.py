#!/usr/bin/env python3
"""Resolve an authorization-stable Snapshot Access helper source."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import Any


VERSION_RE = re.compile(
    r"^(?P<core>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))(?:-(?P<channel>beta|rc|dev)\.(?P<ordinal>[1-9]\d*))?$"
)
TAG_RE = re.compile(r"^v(?P<version>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-(?:beta|rc|dev)\.[1-9]\d*)?)$")
SHA256_RE = re.compile(r"^[0-9a-fA-F]{64}$")
CDHASH_RE = re.compile(r"^[0-9a-fA-F]{40}$")
COMMIT_RE = re.compile(r"^[0-9a-fA-F]{40}$")
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


def verify_source_assets(
    asset_dir: str, lock: dict[str, Any], source_tag: str, expected_source_commit: str
) -> dict[str, str]:
    """Verify the immutable helper Release assets selected for this workflow run."""
    source_version = _tag_version(source_tag)
    if COMMIT_RE.fullmatch(expected_source_commit) is None:
        raise HelperResolutionError("helper source commit must be a full commit SHA")
    root = Path(asset_dir)
    manifest_path = root / "BUILD-MANIFEST.json"
    checksums_path = root / "SHA256SUMS"
    dmg_name = f"TelevyBackup-{source_version}.dmg"
    dmg_path = root / dmg_name
    for path in (manifest_path, checksums_path, dmg_path):
        if not path.is_file():
            raise HelperResolutionError(f"helper source asset is missing: {path.name}")

    manifest = _read_json(str(manifest_path))
    if not isinstance(manifest, dict):
        raise HelperResolutionError("helper source manifest must be a JSON object")
    identities = validate_source_manifest(manifest, lock, source_tag)
    if manifest.get("source_commit") != expected_source_commit:
        raise HelperResolutionError("helper source manifest commit does not match its tag")

    raw_assets = manifest.get("assets")
    if not isinstance(raw_assets, list) or not raw_assets:
        raise HelperResolutionError("helper source manifest has no release assets")
    manifest_assets: dict[str, tuple[str, int]] = {}
    for asset in raw_assets:
        if not isinstance(asset, dict):
            raise HelperResolutionError("helper source manifest contains an invalid asset")
        name = asset.get("name")
        digest = asset.get("sha256")
        size = asset.get("bytes")
        if not isinstance(name, str) or not name:
            raise HelperResolutionError("helper source manifest contains an asset without a name")
        if name in manifest_assets:
            raise HelperResolutionError(f"helper source manifest contains duplicate asset: {name}")
        if not isinstance(digest, str) or SHA256_RE.fullmatch(digest) is None:
            raise HelperResolutionError(f"helper source manifest has invalid asset digest: {name}")
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise HelperResolutionError(f"helper source manifest has invalid asset size: {name}")
        manifest_assets[name] = (digest.lower(), size)
    if dmg_name not in manifest_assets:
        raise HelperResolutionError(f"helper source manifest does not contain its Universal DMG: {dmg_name}")

    checksum_assets: dict[str, str] = {}
    for line in checksums_path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        fields = line.split(maxsplit=1)
        if len(fields) != 2 or SHA256_RE.fullmatch(fields[0]) is None or not fields[1]:
            raise HelperResolutionError("helper source SHA256SUMS contains an invalid line")
        name = fields[1][1:] if fields[1].startswith("*") else fields[1]
        if name in checksum_assets:
            raise HelperResolutionError(f"helper source SHA256SUMS contains duplicate asset: {name}")
        checksum_assets[name] = fields[0].lower()
    if checksum_assets != {name: digest for name, (digest, _) in manifest_assets.items()}:
        raise HelperResolutionError("helper source SHA256SUMS does not match BUILD-MANIFEST.json")

    digest = hashlib.sha256(dmg_path.read_bytes()).hexdigest()
    expected_digest, expected_size = manifest_assets[dmg_name]
    if digest != expected_digest or dmg_path.stat().st_size != expected_size:
        raise HelperResolutionError("helper source Universal DMG does not match BUILD-MANIFEST.json")
    return {
        "source_tag": source_tag,
        "source_commit": expected_source_commit,
        "dmg_sha256": digest,
        **identities,
    }


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

    assets = sub.add_parser("verify-assets")
    assets.add_argument("--asset-dir", required=True)
    assets.add_argument("--lock", required=True)
    assets.add_argument("--source-tag", required=True)
    assets.add_argument("--expected-source-commit", required=True)

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
        elif args.command == "verify-manifest":
            manifest = _read_json(args.manifest)
            lock = _read_json(args.lock)
            if not isinstance(manifest, dict) or not isinstance(lock, dict):
                raise HelperResolutionError("helper manifest and lock must be JSON objects")
            result = validate_source_manifest(manifest, lock, args.source_tag)
        else:
            lock = _read_json(args.lock)
            if not isinstance(lock, dict):
                raise HelperResolutionError("helper lock must be a JSON object")
            result = verify_source_assets(
                args.asset_dir, lock, args.source_tag, args.expected_source_commit
            )
        print(json.dumps(result, sort_keys=True))
        return 0
    except (HelperResolutionError, OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"release_helper.py: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
