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
rows = [{"name": name, "conclusion": "success"} for name in names]
rows.extend([
    {
        "name": "Release intent label gate",
        "conclusion": "success",
        "started_at": "2026-09-07T11:29:15Z",
        "completed_at": "2026-09-07T11:29:21Z",
    },
    {
        "name": "Release intent label gate",
        "conclusion": "failure",
        "started_at": "2026-09-07T11:07:02Z",
        "completed_at": "2026-09-07T11:07:09Z",
    },
])
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump({"check_runs": rows}, handle)
PY
python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 0.0.1 --channel prod \
  --owner fixture --claim-key "pr:1:source:${source_sha}:type:type:patch:channel:channel:prod" \
  --output "$tmp_dir/reservation.json" >/dev/null
python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$repo_dir" --source-sha "$source_sha" --base-sha "$source_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode allocate \
  --reservation-json "$tmp_dir/reservation.json" --provenance github-native-verified >/dev/null
prepared_sha="$(git -C "$repo_dir" rev-parse HEAD)"
printf '{"sha":"%s","commit":{"verification":{"verified":true}}}\n' "$prepared_sha" > "$tmp_dir/github-verification.json"
printf 'source update after preparation\n' > "$repo_dir/POST_PREPARATION"
git -C "$repo_dir" add POST_PREPARATION
git -C "$repo_dir" commit -qm "source update after preparation"
head_sha="$(git -C "$repo_dir" rev-parse HEAD)"
printf '[{"merge_commit_sha":"%s","merged_at":"2026-09-07T11:29:21Z","base":{"ref":"main","repo":{"full_name":"fixture/repo"}}}]\n' "$prepared_sha" > "$tmp_dir/prepared-pulls.json"
python3 - "$root_dir" "$repo_dir" "$prepared_sha" "$tmp_dir/prepared-pulls.json" <<'PY'
import importlib.util
import pathlib
import sys

root, repo, covered, proof = map(pathlib.Path, sys.argv[1:])
spec = importlib.util.spec_from_file_location("release_completion", root / ".github/scripts/release_completion.py")
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.CHAIN.ROOT = repo
try:
    module.verify_version_only_covered_merge(covered.name, covered.name, proof, "fixture/repo")
except module.CompletionError as error:
    assert "already has a release identity" in str(error)
else:
    raise AssertionError("prepared single-parent covered commit was accepted")
PY
out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" \
  --commit "$head_sha" --base "$source_sha" --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/reservation.json" --require-github-verification \
  --github-verification-json "$tmp_dir/github-verification.json")"
[[ "$out" == *'"status": "ready"'* ]]

python3 - "$root_dir" "$repo_dir" "$tmp_dir/reservation.json" "$source_sha" "$head_sha" <<'PY'
import importlib.util
import json
import pathlib
import sys

root, repo, reservation_path, reservation_source, current_source = map(pathlib.Path, sys.argv[1:])
spec = importlib.util.spec_from_file_location("release_completion", root / ".github/scripts/release_completion.py")
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.CHAIN.ROOT = repo
prepared = module.CHAIN.find_prepared(current_source.name, reservation_source.name)
assert prepared["sourceSha"] == reservation_source.name
prepared["sourceSha"] = current_source.name
module.verify_reservation(reservation_path, prepared, repository=None, token=None, api_root="https://api.github.com")
PY

if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" \
  --commit "$head_sha" --base "$source_sha" --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/reservation.json" --require-github-verification >/dev/null 2>&1; then
  echo "production completion accepted missing GitHub verification evidence" >&2
  exit 1
fi

printf '{"sha":"%s","commit":{"verification":{"verified":false}}}\n' "$prepared_sha" > "$tmp_dir/github-verification.json"
if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" \
  --commit "$head_sha" --base "$source_sha" --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/reservation.json" --require-github-verification \
  --github-verification-json "$tmp_dir/github-verification.json" >/dev/null 2>&1; then
  echo "unverified preparation passed the production completion gate" >&2
  exit 1
fi

printf '[{"name":"type:skip"}]\n' > "$tmp_dir/skip-labels.json"
printf '{"check_runs":[]}\n' > "$tmp_dir/skip-checks.json"
skip_out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$repo_dir" \
  --commit "$source_sha" --base "$source_sha" --labels-json "$tmp_dir/skip-labels.json" --checks-json "$tmp_dir/skip-checks.json")"
[[ "$skip_out" == *'"status": "skip"'* ]]

baseline_dir="$tmp_dir/baseline"
mkdir -p "$baseline_dir"
git -C "$baseline_dir" init -q
git -C "$baseline_dir" config user.name fixture
git -C "$baseline_dir" config user.email fixture@example.com
printf '0.9.9\n' > "$baseline_dir/VERSION"
git -C "$baseline_dir" add VERSION
git -C "$baseline_dir" commit -qm source
baseline_base="$(git -C "$baseline_dir" rev-parse HEAD)"
printf '0.9.8\n' > "$baseline_dir/VERSION"
git -C "$baseline_dir" add VERSION
git -C "$baseline_dir" commit -qm 'restore source baseline'
baseline_restore="$(git -C "$baseline_dir" rev-parse HEAD)"
python3 - "$root_dir" "$baseline_dir" "$baseline_base" "$baseline_restore" <<'PY'
import importlib.util
import pathlib
import sys

root, repo, base, commit = map(pathlib.Path, sys.argv[1:])
spec = importlib.util.spec_from_file_location("release_completion", root / ".github/scripts/release_completion.py")
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.CHAIN.ROOT = repo
module.CHAIN.final_tag_baseline = lambda tags: "0.9.8"
module.CHAIN.product_tags = lambda: []
module.verify_baseline_restore(commit.name, base.name)
try:
    module.CHAIN.final_tag_baseline = lambda tags: "0.9.7"
    module.verify_baseline_restore(commit.name, base.name)
except module.CompletionError as error:
    assert "highest final product tag" in str(error)
else:
    raise AssertionError("baseline restore accepted a VERSION outside the final-tag baseline")
PY

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
  --local-root "$squash_dir" --source-sha "$squash_source_sha" --version 0.0.1 --channel prod \
  --owner fixture --claim-key "squash:${squash_source_sha}" \
  --output "$tmp_dir/squash-reservation.json" >/dev/null
python3 "$root_dir/.github/scripts/release_preparation.py" \
  --repo-root "$squash_dir" --source-sha "$squash_source_sha" --base-sha "$squash_covered_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" --mode allocate \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha" \
  --reservation-json "$tmp_dir/squash-reservation.json" --provenance fixture-verified >/dev/null
squash_prepared_sha="$(git -C "$squash_dir" rev-parse HEAD)"
cat > "$tmp_dir/squash-pulls.json" <<JSON
[{"merge_commit_sha":"$squash_covered_sha","merged_at":"2026-09-07T11:29:21Z","base":{"ref":"main","repo":{"full_name":"fixture/repo"}}}]
JSON
if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$squash_dir" --commit "$squash_prepared_sha" --base "$squash_covered_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/squash-reservation.json" \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha" >/dev/null 2>&1; then
  echo "single-parent covered commit passed without authoritative PR proof" >&2
  exit 1
fi
squash_out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$squash_dir" --commit "$squash_prepared_sha" --base "$squash_covered_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/squash-reservation.json" \
  --repository fixture/repo --covered-merge-proof-json "$tmp_dir/squash-pulls.json" \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha")"
[[ "$squash_out" == *'"status": "ready"'* ]]
git -C "$squash_dir" tag v9.9.9 "$squash_covered_sha"
if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$squash_dir" --commit "$squash_prepared_sha" --base "$squash_covered_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/squash-reservation.json" --repository fixture/repo \
  --covered-merge-proof-json "$tmp_dir/squash-pulls.json" \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha" >/dev/null 2>&1; then
  echo "product tag targeting covered commit was accepted" >&2
  exit 1
fi
git -C "$squash_dir" tag -d v9.9.9 >/dev/null
python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$squash_dir" --source-sha "$squash_covered_sha" --version 0.0.2 --channel prod \
  --owner fixture --claim-key "covered:${squash_covered_sha}" \
  --output "$tmp_dir/covered-reservation.json" >/dev/null
if python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$squash_dir" --commit "$squash_prepared_sha" --base "$squash_covered_sha" \
  --labels-json "$tmp_dir/labels.json" --checks-json "$tmp_dir/checks.json" \
  --reservation-json "$tmp_dir/squash-reservation.json" --repository fixture/repo \
  --covered-merge-proof-json "$tmp_dir/squash-pulls.json" \
  --release-mode version-only-release-pr --covered-merge-sha "$squash_covered_sha" >/dev/null 2>&1; then
  echo "append-only identity ref targeting covered commit was accepted" >&2
  exit 1
fi

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
printf '[{"name":"type:skip"}]\n' > "$tmp_dir/migration-labels.json"
migration_out="$(python3 "$root_dir/.github/scripts/release_completion.py" \
  --repo-root "$migration_dir" --commit "$migration_sha" --base "$migration_base" \
  --labels-json "$tmp_dir/migration-labels.json" --checks-json "$tmp_dir/checks.json" \
  --allow-migration --migration-version 0.9.2)"
[[ "$migration_out" == *'"status": "migration"'* ]]
echo "release completion fixture tests passed"
