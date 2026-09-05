#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

python3 - "$root_dir" "$tmp_dir" <<'PY'
from pathlib import Path
import argparse
import importlib.util
import subprocess
import sys

root = Path(sys.argv[1])
repo = Path(sys.argv[2])
repo.mkdir(parents=True, exist_ok=True)
subprocess.run(["git", "init", "-q", str(repo)], check=True)
for key, value in (("user.name", "fixture"), ("user.email", "fixture@example.com")):
    subprocess.run(["git", "-C", str(repo), "config", key, value], check=True)
(repo / "VERSION").write_text("0.9.2\n", encoding="utf-8")
(repo / "README").write_text("fixture\n", encoding="utf-8")
subprocess.run(["git", "-C", str(repo), "add", "."], check=True)
subprocess.run(["git", "-C", str(repo), "commit", "-qm", "fixture source"], check=True)
source = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()

spec = importlib.util.spec_from_file_location("release_chain", root / ".github/scripts/release_chain.py")
assert spec and spec.loader
chain = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chain)
chain.ROOT = repo

chain.stage(argparse.Namespace(
    source_sha=source,
    mode="automatic",
    exact_version=None,
    expected_channel="stable",
    intent_type="type:patch",
    intent_channel="channel:stable",
    intent_action="automatic",
    intent_components="none",
))
prepared = chain.verify_prepared(chain.git("rev-parse", "HEAD"), source)
assert prepared["version"] == "0.9.3"
assert prepared["tag"] == "v0.9.3"
assert chain.diff_names(chain.git("rev-parse", "HEAD")) == ["VERSION"]

(repo / "README").write_text("invalid\n", encoding="utf-8")
subprocess.run(["git", "-C", str(repo), "add", "README"], check=True)
subprocess.run(["git", "-C", str(repo), "commit", "-qm", "invalid extra file"], check=True)
try:
    chain.verify_prepared(chain.git("rev-parse", "HEAD"))
except chain.ReleaseChainError:
    pass
else:
    raise AssertionError("preparation with an extra file was accepted")
PY

echo "release chain fixture tests passed"
