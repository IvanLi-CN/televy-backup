#!/usr/bin/env python3
"""Validate machine-readable DMG verification event streams."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import NoReturn


SEQUENCE = ("dmg_verify", "dmg_attach", "dmg_filesystem_verify", "dmg_detach")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"DMG evidence validation failed: {message}")


parser = argparse.ArgumentParser()
parser.add_argument("--evidence", required=True)
parser.add_argument("--asset-dir", required=True)
args = parser.parse_args()

evidence_path = Path(args.evidence)
asset_dir = Path(args.asset_dir)
if evidence_path.is_symlink() or not evidence_path.is_file():
    fail("evidence must be a regular file")
if asset_dir.is_symlink() or not asset_dir.is_dir():
    fail("asset directory must be a real directory")

expected = {
    str(path.resolve()): path.name
    for path in sorted(asset_dir.glob("*.dmg"))
    if path.is_file() and not path.is_symlink()
}
if not expected:
    fail("asset directory contains no DMG files")

observed: dict[str, list[dict[str, str]]] = {}
for line_number, line in enumerate(evidence_path.read_text(encoding="utf-8").splitlines(), 1):
    try:
        event = json.loads(line)
    except json.JSONDecodeError as error:
        fail(f"line {line_number} is not JSON: {error}")
    if not isinstance(event, dict):
        fail(f"line {line_number} is not an object")
    path = event.get("dmg")
    name = event.get("event")
    if not isinstance(path, str) or not isinstance(name, str):
        fail(f"line {line_number} is missing dmg/event")
    resolved = str(Path(path).resolve())
    if resolved not in expected:
        fail(f"line {line_number} references an unexpected DMG: {path}")
    if name not in SEQUENCE:
        fail(f"line {line_number} has an unexpected event: {name}")
    observed.setdefault(resolved, []).append(event)

if set(observed) != set(expected):
    missing = sorted(set(expected) - set(observed))
    extra = sorted(set(observed) - set(expected))
    fail(f"DMG event coverage mismatch; missing={missing!r}, extra={extra!r}")

for resolved, events in observed.items():
    if len(events) % len(SEQUENCE) != 0:
        fail(f"{expected[resolved]} event count is not a complete sequence: {len(events)}")
    for offset in range(0, len(events), len(SEQUENCE)):
        sequence = events[offset : offset + len(SEQUENCE)]
        names = tuple(event["event"] for event in sequence)
        if names != SEQUENCE:
            fail(f"{expected[resolved]} event sequence is {names!r}, expected {SEQUENCE!r}")
        verify, attach, filesystem, detach = sequence
        if verify["device"] or verify["mount_point"]:
            fail(f"{expected[resolved]} verify event must not claim a device or mount")
        device = attach.get("device")
        mount = attach.get("mount_point")
        if not isinstance(device, str) or not device or not isinstance(mount, str) or not mount:
            fail(f"{expected[resolved]} attach event lacks device or mount")
        if filesystem.get("device") != device or filesystem.get("mount_point") != mount:
            fail(f"{expected[resolved]} filesystem verification changed device or mount")
        if detach.get("device") != device or detach.get("mount_point") != mount:
            fail(f"{expected[resolved]} detach changed device or mount")

print(json.dumps({"dmgs": sorted(expected.values()), "events": sum(map(len, observed.values()))}, sort_keys=True))
