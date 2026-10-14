"""Failure-path contracts for the disposable hosted upgrade fixture."""

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "upgrade_fixture", Path(__file__).parents[1] / "snapshot-helper-upgrade-integration-test.py"
)
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def immediate(probe, predicate):
    result = probe()
    if not predicate(result):
        raise AssertionError("service is still present or its absence is unverifiable")
    return result


class UpgradeCleanupTests(unittest.TestCase):
    def test_shutdown_failure_preserves_all_created_targets(self):
        for failed_service in (fixture.ACCESS_SERVICE, fixture.SERVICE):
            with self.subTest(service=failed_service), tempfile.TemporaryDirectory() as parent:
                root = Path(parent) / "fixture"
                root.mkdir()
                (root / "private-fixture-state").touch()
                calls = []

                def run(*args, **kwargs):
                    calls.append(args)
                    if "print" in args and args[-1] != failed_service:
                        return subprocess.CompletedProcess(args, 113, "", "Could not find service")
                    return subprocess.CompletedProcess(args, 5, "", "Input/output error")

                with patch.object(fixture, "run", run), patch.object(fixture, "wait_for", immediate):
                    with self.assertRaises(AssertionError):
                        fixture.cleanup_fixture(root, [root / "mounted-dmg"], True, True)
                self.assertTrue((root / "private-fixture-state").exists())
                self.assertFalse(any("rm" in args or "hdiutil" in args for args in calls))

    def test_all_shutdowns_are_verified_before_removal(self):
        with tempfile.TemporaryDirectory() as parent:
            root = Path(parent) / "fixture"
            root.mkdir()
            calls = []

            def run(*args, **kwargs):
                calls.append(args)
                if "print" in args:
                    return subprocess.CompletedProcess(args, 113, "", "Could not find service")
                # Already absent services may return a failed bootout; inspection
                # still proves it is safe to remove only the fixture's own files.
                return subprocess.CompletedProcess(args, 5 if "bootout" in args else 0, "", "")

            with patch.object(fixture, "run", run), patch.object(fixture, "wait_for", immediate):
                fixture.cleanup_fixture(root, [root / "mounted-dmg"], True, True)
            removals = [index for index, args in enumerate(calls) if "rm" in args]
            inspections = [index for index, args in enumerate(calls) if "print" in args]
            self.assertEqual(len(inspections), 2)
            self.assertLess(max(inspections), min(removals))
            self.assertFalse(root.exists())


if __name__ == "__main__":
    unittest.main()
