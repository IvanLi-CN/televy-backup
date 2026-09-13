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
[{"name":"type:patch"},{"name":"channel:prod"}]
JSON
python3 - "$tmp_dir/checks.json" <<'PY'
import json
import sys
names = ["Release intent label gate", "quality", "macOS Swift tests", "arm64 native package", "x86_64 native package", "Universal 2 assembly"]
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump({"check_runs": [{"name": name, "conclusion": "success"} for name in names]}, handle)
PY
python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 0.0.1 --channel prod \
  --owner fixture --claim-key "pr:1:source:${source_sha}:type:type:patch:channel:channel:prod" \
  --output "$tmp_dir/reservation.json" >/dev/null
out="$(python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$source_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode allocate \
  --reservation-json "$tmp_dir/reservation.json")"
[[ "$out" == *'"prepared": "created"'* ]]
prepared_sha="$(git -C "$repo_dir" rev-parse HEAD)"
existing="$(python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$prepared_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode allocate)"
[[ "$existing" == *'"prepared": "existing"'* ]]
echo "release preparation fixture tests passed"
