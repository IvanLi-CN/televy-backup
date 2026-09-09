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
subprocess.run(["git", "-C", str(repo), "tag", "v0.9.3", source], check=True)

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
assert prepared["version"] == "0.9.4"
assert prepared["tag"] == "v0.9.4"
assert chain.diff_names(chain.git("rev-parse", "HEAD")) == ["VERSION"]
preparation_sha = chain.git("rev-parse", "HEAD")

def assert_sequence_rejected(version: str, expected: str, marker: str) -> None:
    try:
        chain.verify_release_sequence(version, expected)
    except chain.ReleaseChainError as error:
        assert marker in str(error), (marker, error)
    else:
        raise AssertionError(f"{version} unexpectedly passed sequence validation")

chain.git("tag", "v0.9.7", source)
assert_sequence_rejected("0.9.4", source, "superseded_by_product_tag")
matching = chain.verify_release_sequence("0.9.7", source)
assert matching["status"] == "matching"

chain.git("tag", "v0.9.8-rc.2", source)
assert_sequence_rejected("0.9.8-rc.1", source, "superseded_by_product_tag")
assert chain.verify_release_sequence("0.9.8", source)["status"] == "available"
chain.git("tag", "v0.9.8", source)
assert_sequence_rejected("0.9.8", chain.git("rev-parse", "HEAD"), "product_tag_conflict")
chain.git("tag", "v0.9.9-beta", source)
assert all(item["tag"] != "v0.9.9-beta" for item in chain.product_tags())

(repo / "README").write_text("invalid\n", encoding="utf-8")
subprocess.run(["git", "-C", str(repo), "add", "README"], check=True)
subprocess.run(["git", "-C", str(repo), "commit", "-qm", "invalid extra file"], check=True)
try:
    chain.verify_prepared(chain.git("rev-parse", "HEAD"))
except chain.ReleaseChainError:
    pass
else:
    raise AssertionError("preparation with an extra file was accepted")

# The merge verifier needs the complete ancestry hidden by a depth-2 clone.
topology = repo / "topology"
topology.mkdir()
subprocess.run(["git", "init", "-q", str(topology)], check=True)
for key, value in (("user.name", "fixture"), ("user.email", "fixture@example.com")):
    subprocess.run(["git", "-C", str(topology), "config", key, value], check=True)
(topology / "VERSION").write_text("0.9.2\n", encoding="utf-8")
(topology / "README").write_text("topology\n", encoding="utf-8")
subprocess.run(["git", "-C", str(topology), "add", "."], check=True)
subprocess.run(["git", "-C", str(topology), "commit", "-qm", "topology base"], check=True)
base_sha = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
subprocess.run(["git", "-C", str(topology), "switch", "-q", "-c", "source"], check=True)
for index in (1, 2, 3):
    (topology / f"source-{index}").write_text(f"source {index}\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(topology), "add", "."], check=True)
    subprocess.run(["git", "-C", str(topology), "commit", "-qm", f"source {index}"], check=True)
topology_source = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
(topology / "VERSION").write_text("0.9.3\n", encoding="utf-8")
subprocess.run(["git", "-C", str(topology), "add", "VERSION"], check=True)
subprocess.run(
    [
        "git", "-C", str(topology), "commit", "-qm", "chore(release): v0.9.3",
        "-m", f"Release-Source-SHA: {topology_source}\nProduct-Version: 0.9.3\n"
        "Release-Intent-Type: type:patch\nRelease-Intent-Channel: channel:stable\n"
        "Release-Intent-Action: automatic\nRelease-Intent-Components: none",
    ],
    check=True,
)
topology_preparation = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()
subprocess.run(["git", "-C", str(topology), "switch", "-q", "-c", "mainline", base_sha], check=True)
subprocess.run(["git", "-C", str(topology), "merge", "--no-ff", "-m", "fixture product merge", topology_preparation], check=True)
topology_merge = subprocess.check_output(["git", "-C", str(topology), "rev-parse", "HEAD"], text=True).strip()

shallow = repo / "shallow"
subprocess.run(["git", "clone", "-q", "--depth", "2", f"file://{topology}", str(shallow)], check=True)
chain.ROOT = shallow
try:
    chain.verify_merged(topology_merge)
except chain.ReleaseChainError:
    pass
else:
    raise AssertionError("depth-2 clone unexpectedly verified merge ancestry")
subprocess.run(["git", "-C", str(shallow), "fetch", "-q", "--unshallow"], check=True)
assert chain.verify_merged(topology_merge)["prepared"] == "true"
PY

echo "release chain fixture tests passed"
