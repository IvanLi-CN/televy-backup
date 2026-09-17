#!/usr/bin/env python3
"""Reject symlinked path components while allowing macOS /tmp and /var aliases."""

from __future__ import annotations

import os
import sys
from pathlib import Path


ALLOWED_SYSTEM_ALIASES = {"/tmp", "/var"}


def main() -> int:
    if len(sys.argv) != 2:
        return 2
    absolute = Path(os.path.abspath(sys.argv[1]))
    current = Path(absolute.anchor)
    for component in absolute.parts[1:]:
        current /= component
        if current.is_symlink() and str(current) not in ALLOWED_SYSTEM_ALIASES:
            print(f"path contains a symlinked component: {current}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
