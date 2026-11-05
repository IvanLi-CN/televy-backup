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

python3 - "$tmp_dir/checks.json" <<'PY'
import json
import sys

names = [
    "Release target gate",
    "quality",
    "macOS Swift tests",
    "arm64 native package",
    "x86_64 native package",
    "Universal 2 assembly",
]
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump({"check_runs": [{"name": name, "conclusion": "success"} for name in names]}, handle)
PY

python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 0.0.1 --channel prod \
  --release-target patch --owner fixture --claim-key "pr:1:target:patch" \
  --output "$tmp_dir/reservation.json" >/dev/null
python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$source_sha" --base-sha "$source_sha" \
  --release-target patch --checks-json "$tmp_dir/checks.json" --release-mode normal \
  --reservation-json "$tmp_dir/reservation.json" --provenance github-native-verified >/dev/null
prepared_sha="$(git -C "$repo_dir" rev-parse HEAD)"
printf '{"sha":"%s","commit":{"verification":{"verified":true}}}\n' "$prepared_sha" > "$tmp_dir/github-verification.json"
printf 'source update after preparation\n' > "$repo_dir/POST_PREPARATION"
git -C "$repo_dir" add POST_PREPARATION
git -C "$repo_dir" commit -qm "source update after preparation"
head_sha="$(git -C "$repo_dir" rev-parse HEAD)"

out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" --commit "$head_sha" --base "$source_sha" \
  --checks-json "$tmp_dir/checks.json" --reservation-json "$tmp_dir/reservation.json" \
  --require-github-verification --github-verification-json "$tmp_dir/github-verification.json")"
[[ "$out" == *'"status": "ready"'* ]]
[[ "$out" == *'"releaseTarget": "patch"'* ]]
[[ "$out" == *'"policy": "televybackup-manual-semver-v1"'* ]]

if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" --commit "$head_sha" --base "$source_sha" \
  --checks-json "$tmp_dir/checks.json" --reservation-json "$tmp_dir/reservation.json" \
  --require-github-verification >/dev/null 2>&1; then
  echo "production completion accepted missing GitHub verification evidence" >&2
  exit 1
fi
printf '{"sha":"%s","commit":{"verification":{"verified":false}}}\n' "$prepared_sha" > "$tmp_dir/github-verification.json"
if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" --commit "$head_sha" --base "$source_sha" \
  --checks-json "$tmp_dir/checks.json" --reservation-json "$tmp_dir/reservation.json" \
  --require-github-verification --github-verification-json "$tmp_dir/github-verification.json" >/dev/null 2>&1; then
  echo "unverified preparation passed the production completion gate" >&2
  exit 1
fi

not_release="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" --commit "$source_sha" --base "$source_sha" \
  --checks-json "$tmp_dir/checks.json")"
[[ "$not_release" == *'"status": "not-release"'* ]]

# A VERSION change without a valid manual preparation commit fails closed.
unprepared_dir="$tmp_dir/unprepared"
cp -R "$repo_dir" "$unprepared_dir"
git -C "$unprepared_dir" reset --hard -q "$source_sha"
printf '0.9.3\n' > "$unprepared_dir/VERSION"
git -C "$unprepared_dir" add VERSION
git -C "$unprepared_dir" commit -qm "unprepared version change"
unprepared_sha="$(git -C "$unprepared_dir" rev-parse HEAD)"
if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$unprepared_dir" --commit "$unprepared_sha" --base "$source_sha" \
  --checks-json "$tmp_dir/checks.json" >/dev/null 2>&1; then
  echo "unprepared VERSION change passed completion" >&2
  exit 1
fi

# Version-only release PRs use the same manual target and reservation contract.
squash_dir="$tmp_dir/squash"
mkdir -p "$squash_dir"
git -C "$squash_dir" init -q
git -C "$squash_dir" config user.name fixture
git -C "$squash_dir" config user.email fixture@example.com
printf '0.9.2\n' > "$squash_dir/VERSION"
printf 'base\n' > "$squash_dir/README"
git -C "$squash_dir" add .
git -C "$squash_dir" commit -qm "squash base"
squash_base_sha="$(git -C "$squash_dir" rev-parse HEAD)"
printf 'squashed product fix\n' > "$squash_dir/README"
git -C "$squash_dir" add README
git -C "$squash_dir" commit -qm "squashed product fix"
squash_covered_sha="$(git -C "$squash_dir" rev-parse HEAD)"
printf '0.9.9-rc.1\n' > "$squash_dir/VERSION"
git -C "$squash_dir" add VERSION
git -C "$squash_dir" commit -qm "stage squash recovery"
squash_source_sha="$(git -C "$squash_dir" rev-parse HEAD)"
python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$squash_dir" --source-sha "$squash_source_sha" --version 0.9.9-rc.2 --channel rc \
  --release-target 0.9.9-rc.2 --owner fixture --claim-key "pr:2:target:0.9.9-rc.2" \
  --output "$tmp_dir/squash-reservation.json" >/dev/null
python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$squash_dir" --source-sha "$squash_source_sha" --base-sha "$squash_covered_sha" \
  --release-target 0.9.9-rc.2 --checks-json "$tmp_dir/checks.json" \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha" \
  --reservation-json "$tmp_dir/squash-reservation.json" >/dev/null
squash_prepared_sha="$(git -C "$squash_dir" rev-parse HEAD)"
printf '[{"merge_commit_sha":"%s","merged_at":"2026-09-07T11:29:21Z","base":{"ref":"main","repo":{"full_name":"fixture/repo"}}}]\n' \
  "$squash_covered_sha" > "$tmp_dir/squash-pulls.json"
squash_out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$squash_dir" --commit "$squash_prepared_sha" --base "$squash_covered_sha" \
  --checks-json "$tmp_dir/checks.json" --reservation-json "$tmp_dir/squash-reservation.json" \
  --repository fixture/repo --covered-merge-proof-json "$tmp_dir/squash-pulls.json" \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha")"
[[ "$squash_out" == *'"status": "ready"'* ]]

echo "release completion fixture tests passed"
