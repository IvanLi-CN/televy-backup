#!/usr/bin/env python3
"""Verify the on-disk DMG format and the filesystem reported after attach."""

from __future__ import annotations

import argparse
import plistlib
import re
from pathlib import Path


def load_plist(path: Path) -> dict[str, object]:
    with path.open("rb") as handle:
        value = plistlib.load(handle)
    if not isinstance(value, dict):
        raise SystemExit(f"DMG metadata plist must contain a dictionary: {path}")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image-info", type=Path, required=True)
    parser.add_argument("--filesystem-info", type=Path, required=True)
    parser.add_argument("--expected-format", default="UDZO")
    parser.add_argument("--expected-filesystem", default="HFS+")
    args = parser.parse_args()

    image_info = load_plist(args.image_info)
    actual_format = image_info.get("Format")
    if actual_format != args.expected_format:
        raise SystemExit(
            f"DMG image format mismatch: {actual_format!r} != {args.expected_format!r}"
        )

    filesystem_info = load_plist(args.filesystem_info)
    observed = {
        re.sub(r"[^a-z0-9]", "", str(filesystem_info.get(key, "")).lower())
        for key in ("FilesystemType", "FilesystemPersonality", "FilesystemName")
    }
    expected = re.sub(r"[^a-z0-9]", "", args.expected_filesystem.lower())
    accepted = {expected}
    if expected == "hfs":
        accepted.update({"journaledhfs", "macosextendedjournaled"})
    if not expected or not observed.intersection(accepted):
        raise SystemExit(
            "attached filesystem mismatch: "
            f"observed={sorted(observed)!r}, expected={args.expected_filesystem!r}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
