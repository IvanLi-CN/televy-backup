#!/usr/bin/env python3
"""Resolve the formal product VERSION and local development identity."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERSION_PATH = ROOT / "VERSION"
FORMAL_VERSION_RE = re.compile(
    r"^(?P<core>(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*))(?:-(?P<kind>beta|rc|dev)\.(?P<ordinal>[1-9]\d*))?$"
)


class VersionError(ValueError):
    """Raised when VERSION or a requested identity is invalid."""


def parse_version(value: str) -> dict[str, str | int | None]:
    """Parse a formal product version, including beta/rc/dev ordinals."""
    match = FORMAL_VERSION_RE.fullmatch(value)
    if match is None:
        raise VersionError(f"invalid product version: {value!r}")
    major, minor, patch = match.group("core").split(".")
    kind = match.group("kind")
    ordinal = int(match.group("ordinal")) if match.group("ordinal") else None
    return {
        "major": major,
        "minor": minor,
        "patch": patch,
        "kind": kind,
        "ordinal": ordinal,
        "prerelease": f"{kind}.{ordinal}" if kind else None,
        "channel": kind or "prod",
    }


def read_version(path: Path = VERSION_PATH) -> str:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise VersionError(f"cannot read {path}: {error}") from error
    return read_version_from_text(text, path)


def read_version_from_text(text: str, path: Path | None = None) -> str:
    if not text.endswith("\n") or text.count("\n") != 1:
        subject = str(path) if path else "VERSION"
        raise VersionError(f"{subject} must contain exactly one semver line ending in LF")
    value = text[:-1]
    parse_version(value)
    return value


def next_patch(version: str) -> str:
    parsed = parse_version(version)
    return f"{parsed['major']}.{parsed['minor']}.{int(parsed['patch']) + 1}"


def next_base(version: str, change: str) -> str:
    parsed = parse_version(version)
    major, minor, patch = int(parsed["major"]), int(parsed["minor"]), int(parsed["patch"])
    if change == "major":
        return f"{major + 1}.0.0"
    if change == "minor":
        return f"{major}.{minor + 1}.0"
    if change == "patch":
        return f"{major}.{minor}.{patch + 1}"
    raise VersionError(f"unsupported version change: {change!r}")


def format_release_version(base: str, channel: str, ordinal: int | None = None) -> str:
    """Format the identity selected by the release label contract."""
    parsed = parse_version(base)
    if parsed["kind"] is not None:
        raise VersionError("release base must be a final version")
    if channel == "prod":
        if ordinal is not None:
            raise VersionError("prod releases do not have an ordinal")
        return base
    if channel not in {"beta", "rc", "dev"}:
        raise VersionError(f"unsupported release channel: {channel!r}")
    if ordinal is None or ordinal < 1:
        raise VersionError("prerelease ordinal must be positive")
    return f"{base}-{channel}.{ordinal}"


def git_sha() -> str:
    try:
        value = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError) as error:
        raise VersionError("source SHA is required outside a git checkout") from error
    if not re.fullmatch(r"[0-9a-fA-F]{40}", value):
        raise VersionError(f"invalid source SHA: {value!r}")
    return value.lower()


def resolve(mode: str, source_sha: str | None = None) -> dict[str, str | int | None]:
    current = read_version()
    sha = source_sha or git_sha()
    if not re.fullmatch(r"[0-9a-fA-F]{40}", sha):
        raise VersionError(f"invalid source SHA: {sha!r}")
    sha = sha.lower()
    if mode == "release":
        version = current
        parsed = parse_version(version)
        channel = str(parsed["channel"])
    elif mode == "development":
        # Local builds retain their short-SHA identity and are not formal dev releases.
        version = f"{next_patch(current)}-dev.{sha[:7]}"
        channel = "development"
    else:
        raise VersionError(f"unsupported build mode: {mode!r}")
    return {
        "version": version,
        "sourceSha": sha,
        "shortSha": sha[:7],
        "mode": mode,
        "channel": channel,
        "buildId": sha[:16],
        "baseVersion": current,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("development", "release"), required=True)
    parser.add_argument("--source-sha")
    parser.add_argument("--format", choices=("plain", "json"), default="plain")
    args = parser.parse_args(argv)
    try:
        identity = resolve(args.mode, args.source_sha)
    except VersionError as error:
        parser.error(str(error))
    if args.format == "json":
        print(json.dumps(identity, sort_keys=True))
    else:
        print(identity["version"])
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
