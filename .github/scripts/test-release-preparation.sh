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
existing_source_sha="$(printf '%s' "$existing" | python3 -c 'import json,sys; print(json.load(sys.stdin)["source_sha"])')"
[[ "$existing_source_sha" == "$source_sha" ]]

# A retry may advance the source head after reservation, but only along the
# same ancestry; the immutable reservation source remains the verification key.
retry_dir="$tmp_dir/retry"
cp -R "$repo_dir" "$retry_dir"
printf 'source update after reservation\n' > "$retry_dir/RETRY"
git -C "$retry_dir" add RETRY
git -C "$retry_dir" commit -qm "source update after reservation"
retry_source_sha="$(git -C "$retry_dir" rev-parse HEAD)"
retry_reservation="$tmp_dir/retry-reservation.json"
python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$retry_dir" --source-sha "$source_sha" --version 0.0.2 --channel prod \
  --owner fixture --claim-key "retry:${source_sha}" --output "$retry_reservation" >/dev/null
retry_out="$(python3 - "$retry_reservation" "$source_sha" "$retry_source_sha" <<'PY'
import json
import sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
value["sourceSha"] = sys.argv[2]
value["reservationSourceSha"] = sys.argv[2]
json.dump(value, open(sys.argv[1], "w", encoding="utf-8"))
PY
python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$retry_dir" --source-sha "$retry_source_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode allocate \
  --reservation-json "$retry_reservation")"
[[ "$retry_out" == *'"prepared": "created"'* ]]
retry_prepared_sha="$(git -C "$retry_dir" rev-parse HEAD)"
retry_source_recorded="$(git -C "$retry_dir" show -s --format='%(trailers:key=Release-Reservation-Source-SHA,valueonly)' "$retry_prepared_sha")"
[[ "$retry_source_recorded" == "$source_sha" ]]
echo "release preparation fixture tests passed"
