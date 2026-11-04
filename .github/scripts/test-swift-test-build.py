#!/usr/bin/env python3
"""Exercise module reuse and compile failures without requiring the macOS SDK."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = (ROOT / 'scripts/macos/swift-unit-tests.sh').read_text()
FUNCTION = SCRIPT[SCRIPT.index('shared_module_ready=false'):SCRIPT.index('bin_target_presentation=')]


class SwiftTestBuildTests(unittest.TestCase):
    def exercise(self, mode='shared', failure=''):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            compiler = root / 'swiftc'
            compiler.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['CALLS'], 'a') as f: f.write(json.dumps(args) + '\\n')
module = '-emit-library' in args
if os.environ.get('FAIL') == ('module' if module else 'link'): sys.exit(19)
out = pathlib.Path(args[args.index('-o') + 1])
out.write_text('#!/bin/sh\\nprintf "test-executed\\n"\\n')
out.chmod(0o755)
''')
            compiler.chmod(0o755)
            (root / 'macos/TelevyBackupApp').mkdir(parents=True)
            (root / 'macos/TelevyBackupApp/App.swift').write_text('struct App {}')
            test = root / 'One.swift'
            test.write_text('@main struct One {}\n')
            calls = root / 'calls.jsonl'
            env = dict(os.environ, CALLS=str(calls), FAIL=failure)
            command = '''set -euo pipefail
root_dir="$1"; out_dir="$1/out"; swiftc="$1/swiftc"; sdk_path=fake; swift_test_build_mode="$2"
mkdir -p "$out_dir"
''' + FUNCTION + '''
compile_app_test "$out_dir/one" "$root_dir/One.swift"
"$out_dir/one"
compile_app_test "$out_dir/two" "$root_dir/One.swift"
"$out_dir/two"
'''
            result = subprocess.run(['bash', '-c', command, 'fixture', str(root), mode], env=env, capture_output=True, text=True)
            args = [json.loads(line) for line in calls.read_text().splitlines()]
            generated = root / 'out/app-test-module/One.swift'
            return result, args, generated.read_text() if generated.exists() else None

    def test_shared_module_compiles_once_and_both_executables_run(self):
        result, calls, source = self.exercise()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sum('-emit-library' in call for call in calls), 1)
        self.assertEqual(len(calls), 3)
        self.assertTrue(source.startswith('@testable import TelevyBackupTestSupport\n'))
        self.assertEqual(result.stdout.count('test-executed'), 2)
        for call in calls[1:]:
            self.assertIn('-lTelevyBackupTestSupport', call)
            self.assertIn('-rpath', call)

    def test_reference_mode_keeps_isolated_builds(self):
        result, calls, source = self.exercise('isolated')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 2)
        self.assertTrue(all('-emit-library' not in call for call in calls))
        self.assertIsNone(source)
        self.assertEqual(result.stdout.count('test-executed'), 2)

    def test_module_failure_stops_before_any_test(self):
        result, calls, _ = self.exercise(failure='module')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(calls), 1)
        self.assertNotIn('test-executed', result.stdout)

    def test_link_failure_stops_before_any_test(self):
        result, calls, _ = self.exercise(failure='link')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(calls), 2)
        self.assertNotIn('test-executed', result.stdout)


class CargoBuildTests(unittest.TestCase):
    def exercise(self, failure):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cargo = root / 'cargo'
            cargo.write_text('''#!/usr/bin/env python3
import os, pathlib, sys, time
kind = 'helper' if '--manifest-path' in sys.argv else 'workspace'
root = pathlib.Path(os.environ['BUILD_FIXTURE'])
(root / (kind + '.started')).touch()
other = 'helper' if kind == 'workspace' else 'workspace'
for _ in range(100):
    if (root / (other + '.started')).exists(): break
    time.sleep(0.02)
else: sys.exit(27)
(root / (kind + '.finished')).touch()
sys.exit(19 if os.environ.get('BUILD_FAIL') == kind else 0)
''')
            cargo.chmod(0o755)
            source = (ROOT / 'scripts/macos/build-app.sh').read_text()
            block = source[source.index('helper_build=(cargo'):source.index('cp "$binary_dir/televybackup"')]
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'],
                       BUILD_FIXTURE=str(root), BUILD_FAIL=failure)
            script = 'set -euo pipefail\nroot_dir="$1"; cargo_target=x86_64-apple-darwin\nworkspace_build=(cargo build --locked --release)\n' + block
            result = subprocess.run(['bash', '-c', script, 'fixture', str(root)], env=env, capture_output=True, text=True)
            return result, all((root / (kind + '.finished')).exists() for kind in ('helper', 'workspace'))

    def test_independent_builds_overlap_and_are_joined(self):
        result, joined = self.exercise('')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(joined)

    def test_either_failure_is_fail_closed_after_join(self):
        for failure in ('workspace', 'helper'):
            with self.subTest(failure=failure):
                result, joined = self.exercise(failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue(joined)


if __name__ == '__main__':
    unittest.main()
