#!/usr/bin/env python3
"""Contract tests for the checked-in product version resolver."""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("product_version", ROOT / "scripts/product-version.py")
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ProductVersionTests(unittest.TestCase):
    def test_read_version_requires_one_lf(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "VERSION"
            path.write_text("0.9.2\n", encoding="utf-8")
            self.assertEqual(MODULE.read_version(path), "0.9.2")
            path.write_text("0.9.2\n\n", encoding="utf-8")
            with self.assertRaises(MODULE.VersionError):
                MODULE.read_version(path)

    def test_stable_and_rc_grammar(self) -> None:
        self.assertEqual(MODULE.parse_version("1.2.3")["prerelease"], None)
        self.assertEqual(MODULE.parse_version("1.2.3-rc.4")["prerelease"], "rc.4")
        for value in ("1.2", "1.2.3-dev.abc", "v1.2.3", "1.2.3-rc"):
            with self.assertRaises(MODULE.VersionError):
                MODULE.parse_version(value)

    def test_next_patch_ignores_rc_qualifier(self) -> None:
        self.assertEqual(MODULE.next_patch("0.9.2"), "0.9.3")
        self.assertEqual(MODULE.next_patch("0.9.3-rc.2"), "0.9.4")

    def test_resolve_modes(self) -> None:
        sha = "a" * 40
        current = MODULE.read_version()
        development = MODULE.resolve("development", sha)
        release = MODULE.resolve("release", sha)
        self.assertEqual(development["version"], f"{MODULE.next_patch(current)}-dev.aaaaaaa")
        self.assertEqual(release["version"], current)
        self.assertEqual(development["sourceSha"], sha)
        self.assertEqual(development["shortSha"], "a" * 7)


if __name__ == "__main__":
    unittest.main()
