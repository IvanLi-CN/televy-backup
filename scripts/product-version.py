#!/usr/bin/env python3
"""Resolve TelevyBackup product identity from the checked-in VERSION file."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERSION_PATH = ROOT / "VERSION"
VERSION_RE = re.compile(r"^(?P<core>\d+\.\d+\.\d+)(?:-rc\.(?P<rc>\d+))?\n$")


class VersionError(ValueError):
    """Raised when VERSION or a requested identity is invalid."""


def parse_version(value: str) -> dict[str, str | None]:
    match = VERSION_RE.fullmatch(value + "\n") if "\n" not in value else VERSION_RE.fullmatch(value)
    if match is None:
        raise VersionError(f"invalid product version: {value!r}")
    major, minor, patch = match.group("core").split(".")
    return {
        "major": major,
        "minor": minor,
        "patch": patch,
        "rc": match.group("rc"),
        "prerelease": f"rc.{match.group('rc')}" if match.group("rc") else None,
    }


def read_version(path: Path = VERSION_PATH) -> str:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise VersionError(f"cannot read {path}: {error}") from error
    return read_version_from_text(text, path)


def read_version_from_text(text: str, path: Path | None = None) -> str:
    match = VERSION_RE.fullmatch(text)
    if match is None:
        subject = str(path) if path else "VERSION"
        raise VersionError(f"{subject} must contain exactly one semver line ending in LF")
    return match.group("core") + (f"-rc.{match.group('rc')}" if match.group("rc") else "")


def next_patch(version: str) -> str:
    parsed = parse_version(version)
    return f"{parsed['major']}.{parsed['minor']}.{int(parsed['patch']) + 1}"


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


def resolve(mode: str, source_sha: str | None = None) -> dict[str, str | None]:
    current = read_version()
    sha = source_sha or git_sha()
    if not re.fullmatch(r"[0-9a-fA-F]{40}", sha):
        raise VersionError(f"invalid source SHA: {sha!r}")
    sha = sha.lower()
    if mode == "release":
        version = current
        parsed = parse_version(version)
        channel = "rc" if parsed["prerelease"] else "stable"
    else:
        version = f"{next_patch(current)}-dev.{sha[:7]}"
        parse_version(next_patch(current))
        channel = "development"
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
