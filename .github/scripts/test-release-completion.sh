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
names = ["Release intent label gate", "quality", "macOS Swift tests", "arm64 native package", "x86_64 native package", "Universal 2 assembly"]
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump({"check_runs": [{"name": name, "conclusion": "success"} for name in names]}, handle)
PY
python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$source_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode automatic >/dev/null
prepared_sha="$(git -C "$repo_dir" rev-parse HEAD)"
out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" \
  --commit "$prepared_sha" --base "$source_sha" --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json")"
[[ "$out" == *'"status": "ready"'* ]]

migration_dir="$tmp_dir/migration"
mkdir -p "$migration_dir"
git -C "$migration_dir" init -q
git -C "$migration_dir" config user.name fixture
git -C "$migration_dir" config user.email fixture@example.com
printf 'source\n' > "$migration_dir/README"
git -C "$migration_dir" add README
git -C "$migration_dir" commit -qm source
migration_base="$(git -C "$migration_dir" rev-parse HEAD)"
printf '0.9.2\n' > "$migration_dir/VERSION"
git -C "$migration_dir" add VERSION
git -C "$migration_dir" commit -qm migration
migration_sha="$(git -C "$migration_dir" rev-parse HEAD)"
printf '[{"name":"type:skip"},{"name":"channel:stable"}]\n' > "$tmp_dir/migration-labels.json"
migration_out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$migration_dir" --commit "$migration_sha" --base "$migration_base" \
  --labels-json "$tmp_dir/migration-labels.json" --checks-json "$tmp_dir/checks.json" \
  --allow-migration --migration-version 0.9.2)"
[[ "$migration_out" == *'"status": "migration"'* ]]
echo "release completion fixture tests passed"
