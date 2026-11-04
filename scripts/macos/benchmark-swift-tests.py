#!/usr/bin/env python3
"""Compare isolated/shared compilation with identical test entry points."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--sample', type=int, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
args.output.mkdir(parents=True, exist_ok=False)
results = {}
order = ['isolated', 'shared'] if args.sample % 2 else ['shared', 'isolated']
for mode in order:
    env = dict(os.environ, TELEVYBACKUP_SWIFT_TEST_BUILD_MODE=mode,
               OUT_DIR=str(args.output / mode))
    start = time.monotonic()
    with (args.output / f'{mode}.log').open('w') as log:
        result = subprocess.run(['bash', str(root / 'scripts/macos/swift-unit-tests.sh')], cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT)
    seconds = time.monotonic() - start
    text = (args.output / f'{mode}.log').read_text()
    markers = sorted(line for line in text.splitlines() if re.match(r'^(OK:|[A-Za-z]+Tests: PASS)', line))
    results[mode] = {'seconds': seconds, 'exit_code': result.returncode, 'test_markers': markers}
    print(f'{mode}: {seconds:.2f}s exit={result.returncode}', flush=True)
    if result.returncode:
        print(text[-20000:], flush=True)
        break
passed = (len(results) == 2 and all(r['exit_code'] == 0 for r in results.values())
          and results['isolated']['test_markers'] == results['shared']['test_markers']
          and len(results['shared']['test_markers']) > 10)
report = {'source_sha': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
          'workflow_sha': os.environ.get('GITHUB_SHA'), 'sample': args.sample,
          'order': order, 'results': results, 'equivalent_pass': passed,
          'compiled_app_cache': 'none: each mode compiles into a fresh output directory',
          'sdk_module_cache': 'runner ambient; order alternates across samples'}
(args.output / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
raise SystemExit(0 if passed else 1)
