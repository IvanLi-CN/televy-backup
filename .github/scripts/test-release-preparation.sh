#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
repo_dir="$tmp_dir/repo"
mkdir -p "$repo_dir"
git -C "$repo_dir" init -q
git -C "$repo_dir" config user.name fixture
git -C "$repo_dir" config user.email fixture@example.com
printf '0.9.2\n' > "$repo_dir/VERSION"
git -C "$repo_dir" add VERSION
git -C "$repo_dir" commit -qm source
source_sha="$(git -C "$repo_dir" rev-parse HEAD)"
cat > "$tmp_dir/labels.json" <<'JSON'
[{"name":"type:patch"},{"name":"channel:stable"}]
JSON
python3 - "$tmp_dir/checks.json" <<'PY'
import json
import sys
names = ["Validate PR labels", "quality", "macOS Swift tests", "arm64 native package", "x86_64 native package", "Universal 2 assembly"]
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump({"check_runs": [{"name": name, "conclusion": "success"} for name in names]}, handle)
PY
out="$(python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$source_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode automatic)"
[[ "$out" == *'"prepared": "created"'* ]]
prepared_sha="$(git -C "$repo_dir" rev-parse HEAD)"
existing="$(python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$prepared_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode automatic)"
[[ "$existing" == *'"prepared": "existing"'* ]]
echo "release preparation fixture tests passed"
