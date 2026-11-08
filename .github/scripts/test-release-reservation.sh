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
claim_key="pr:7:target:1.0.0-beta.1"

first="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --release-target 1.0.0-beta.1 --owner fixture --claim-key "$claim_key")"
second="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --release-target 1.0.0-beta.1 --owner fixture --claim-key "$claim_key")"
[[ "$(printf '%s' "$first" | jq -r .target)" == "$(printf '%s' "$second" | jq -r .target)" ]]
[[ "$(git -C "$repo_dir" for-each-ref --format='%(refname)' refs/tags/release-reservation)" == "refs/tags/release-reservation/v1.0.0-beta.1" ]]
found="$(python3 "$root_dir/.github/scripts/release_reservation.py" find-claim \
  --local-root "$repo_dir" --release-target 1.0.0-beta.1 --owner fixture --claim-key "$claim_key")"
[[ "$(printf '%s' "$found" | jq -r .sourceSha)" == "$source_sha" ]]

if python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --release-target 1.0.0-beta.1 --owner foreign --claim-key "$claim_key" >/dev/null 2>&1; then
  echo "foreign reservation claim was accepted" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel rc \
  --release-target 1.0.0-beta.1 --owner fixture --claim-key "$claim_key" >/dev/null 2>&1; then
  echo "version/channel mismatch was accepted" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --release-target patch --owner fixture --claim-key "$claim_key" >/dev/null 2>&1; then
  echo "symbolic target was bound to a prerelease version" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --release-target 1.0.0-beta.2 --owner fixture --claim-key "$claim_key" >/dev/null 2>&1; then
  echo "explicit target/version mismatch was accepted" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state consumed --version 1.0.0-beta.1 --merge-sha "$source_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 >/dev/null 2>&1; then
  echo "consumed receipt was created without bound receipt" >&2
  exit 1
fi

git -C "$repo_dir" switch -q -c prepared
printf '1.0.0-beta.1\n' > "$repo_dir/VERSION"
git -C "$repo_dir" add VERSION
git -C "$repo_dir" commit -qm "chore(release): v1.0.0-beta.1" -m "Release-Source-SHA: ${source_sha}
Product-Version: 1.0.0-beta.1
Release-Target: 1.0.0-beta.1
Release-Policy: televybackup-manual-semver-v1
Release-Channel: beta
Release-Mode: normal
Release-Reservation-Id: $(printf '%s' "$first" | jq -r .reservationId)
Release-Reservation-Ref: refs/tags/release-reservation/v1.0.0-beta.1
Release-Reservation-Owner: fixture
Release-Claim-Key: ${claim_key}
Release-Boundary-Token: $(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')
Release-Provenance: fixture-verified"
git -C "$repo_dir" switch -q -c post-preparation
printf 'post-preparation\n' > "$repo_dir/fixture.txt"
git -C "$repo_dir" add fixture.txt
git -C "$repo_dir" commit -qm "fixture post-preparation change"
post_preparation_sha="$(git -C "$repo_dir" rev-parse HEAD)"
git -C "$repo_dir" switch -q -c mainline "$source_sha"
git -C "$repo_dir" merge --no-ff -qm "fixture product merge" "$post_preparation_sha"
merge_sha="$(git -C "$repo_dir" rev-parse HEAD)"

bound="$(python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state bound --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1)"
consumed="$(python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state consumed --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1)"
[[ "$(printf '%s' "$bound" | jq -r .ref)" == refs/tags/release-bound/* ]]
[[ "$(printf '%s' "$consumed" | jq -r .ref)" == refs/tags/release-consumed/* ]]

artifact_pending="$(python3 "$root_dir/.github/scripts/release_reservation.py" artifact-receipt \
  --local-root "$repo_dir" --state pending --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --run-id 100 --run-attempt 1 --packaging-policy-sha "$source_sha")"
artifact_attempt="$(python3 "$root_dir/.github/scripts/release_reservation.py" artifact-receipt \
  --local-root "$repo_dir" --state attempt --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --run-id 100 --run-attempt 1 --packaging-policy-sha "$source_sha")"
artifact_finalized="$(python3 "$root_dir/.github/scripts/release_reservation.py" artifact-receipt \
  --local-root "$repo_dir" --state finalized --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --run-id 100 --run-attempt 1 --packaging-policy-sha "$source_sha" \
  --artifact-id 101 --artifact-digest "$(printf '%064d' 1)" --helper-mode reuse)"
artifact_finalized_again="$(python3 "$root_dir/.github/scripts/release_reservation.py" verify-artifact-receipt \
  --local-root "$repo_dir" --state finalized --version 1.0.0-beta.1 --merge-sha "$merge_sha")"
[[ "$(printf '%s' "$artifact_pending" | jq -r .ref)" == refs/tags/release-artifact-pending/* ]]
[[ "$(printf '%s' "$artifact_attempt" | jq -r .ref)" == refs/tags/release-artifact-attempts/* ]]
[[ "$(printf '%s' "$artifact_finalized" | jq -r .ref)" == refs/tags/release-artifacts/* ]]
[[ "$(printf '%s' "$artifact_finalized" | jq -r '."Release-Artifact-Digest"')" == sha256:* ]]
[[ "$(printf '%s' "$artifact_finalized_again" | jq -r '."Release-Artifact-Id"')" == 101 ]]
artifact_attempt_again="$(python3 "$root_dir/.github/scripts/release_reservation.py" verify-artifact-receipt \
  --local-root "$repo_dir" --state attempt --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --run-id 100 --run-attempt 1)"
[[ "$(printf '%s' "$artifact_attempt_again" | jq -r .ref)" == "$(printf '%s' "$artifact_attempt" | jq -r .ref)" ]]
artifact_named="$(python3 "$root_dir/.github/scripts/release_reservation.py" artifact-receipt \
  --local-root "$repo_dir" --state attempt --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --run-id 101 --run-attempt 2 --packaging-policy-sha "$source_sha" \
  --artifact-name release-assets-101-2)"
[[ "$(printf '%s' "$artifact_named" | jq -r '."Release-Artifact-Name"')" == release-assets-101-2 ]]
if python3 "$root_dir/.github/scripts/release_reservation.py" artifact-receipt \
  --local-root "$repo_dir" --state finalized --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --run-id 100 --run-attempt 1 --packaging-policy-sha "$source_sha" \
  --artifact-id 102 --artifact-digest "sha256:$(printf '%064d' 2)" --helper-mode reuse >/dev/null 2>&1; then
  echo "artifact receipt ref was overwritten" >&2
  exit 1
fi

decision_ref="refs/tags/release-decision/v1.0.0-beta.1"
decision_target="$(git -C "$repo_dir" rev-parse "$decision_ref")"
git -C "$repo_dir" update-ref -d "$decision_ref"
if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state consumed --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 >/dev/null 2>&1; then
  echo "consumed receipt bypassed the decision ref" >&2
  exit 1
fi
git -C "$repo_dir" update-ref "$decision_ref" "$decision_target"

if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state bound --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner foreign \
  --claim-key "$claim_key" --boundary-token "wrong" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 >/dev/null 2>&1; then
  echo "receipt ref was overwritten" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state released --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 >/dev/null 2>&1; then
  echo "released receipt bypassed maintainer confirmation" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state released --version 1.0.0-beta.1 --merge-sha "$merge_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 --maintainer-confirmed >/dev/null 2>&1; then
  echo "released receipt bypassed bound state" >&2
  exit 1
fi

orphan_repo="$tmp_dir/orphan-repo"
mkdir -p "$orphan_repo"
git -C "$orphan_repo" init -q
git -C "$orphan_repo" config user.name fixture
git -C "$orphan_repo" config user.email fixture@example.com
printf '0.9.6\n' > "$orphan_repo/VERSION"
git -C "$orphan_repo" add VERSION
git -C "$orphan_repo" commit -qm orphan-source
orphan_source_sha="$(git -C "$orphan_repo" rev-parse HEAD)"
orphan_claim_key="pr:305:target:0.9.7"
orphan_reservation="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$orphan_repo" --source-sha "$orphan_source_sha" --version 0.9.7 --channel prod \
  --release-target 0.9.7 --owner github-actions[bot] --claim-key "$orphan_claim_key")"
orphan_reservation_id="$(printf '%s' "$orphan_reservation" | jq -r .reservationId)"
orphan_boundary_token="$(printf '%s' "$orphan_reservation" | jq -r '."Reservation-Boundary-Token"')"
cat > "$tmp_dir/orphan-evidence.json" <<EOF
{"kind":"orphan-unbound","schema_version":1,"version":"0.9.7","main_sha":"$orphan_source_sha","product_tag":{"ref":"v0.9.7","status":"absent"},"github_release":{"tag":"v0.9.7","status":"absent"},"decision_refs":[],"reclaimed_refs":[],"bound_refs":[],"consumed_refs":[],"released_refs":[],"retired_refs":[],"artifact_refs":[],"release_runs":[{"id":"42506238969","status":"completed","conclusion":"success","head_branch":"main","workflow_path":".github/workflows/release.yml","artifacts":[]}]}
EOF
orphan_recovery="$(python3 "$root_dir/.github/scripts/release_reservation.py" recover-orphan \
  --local-root "$orphan_repo" --version 0.9.7 --reservation-id "$orphan_reservation_id" \
  --owner github-actions[bot] --claim-key "$orphan_claim_key" --release-target 0.9.7 \
  --boundary-token "$orphan_boundary_token" --evidence-json "$tmp_dir/orphan-evidence.json")"
[[ "$(printf '%s' "$orphan_recovery" | jq -r .ref)" == "refs/tags/release-orphaned/v0.9.7/$orphan_reservation_id" ]]
orphan_reservation_sha="$(git -C "$orphan_repo" rev-parse refs/tags/release-reservation/v0.9.7^{commit})"
orphan_recovery_again="$(python3 "$root_dir/.github/scripts/release_reservation.py" recover-orphan \
  --local-root "$orphan_repo" --version 0.9.7 --reservation-id "$orphan_reservation_id" \
  --owner github-actions[bot] --claim-key "$orphan_claim_key" --release-target 0.9.7 \
  --boundary-token "$orphan_boundary_token" --evidence-json "$tmp_dir/orphan-evidence.json")"
[[ "$(printf '%s' "$orphan_recovery_again" | jq -r .target)" == "$(printf '%s' "$orphan_recovery" | jq -r .target)" ]]
[[ "$(git -C "$orphan_repo" rev-parse refs/tags/release-reservation/v0.9.7^{commit})" == "$orphan_reservation_sha" ]]
orphan_reclaimed_claim="pr:306:target:0.9.7"
orphan_reclaimed="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$orphan_repo" --source-sha "$orphan_source_sha" --version 0.9.7 --channel prod \
  --release-target 0.9.7 --owner github-actions[bot] --claim-key "$orphan_reclaimed_claim")"
orphan_reclaimed_id="$(printf '%s' "$orphan_reclaimed" | jq -r .reservationId)"
[[ "$(printf '%s' "$orphan_reclaimed" | jq -r .ref)" == "refs/tags/release-reservation-reclaimed/v0.9.7/$orphan_reclaimed_id" ]]

legacy_repo="$tmp_dir/legacy-repo"
mkdir -p "$legacy_repo"
git -C "$legacy_repo" init -q
git -C "$legacy_repo" config user.name fixture
git -C "$legacy_repo" config user.email fixture@example.com
printf '0.9.8\n' > "$legacy_repo/VERSION"
git -C "$legacy_repo" add VERSION
git -C "$legacy_repo" commit -qm legacy-source
legacy_source_sha="$(git -C "$legacy_repo" rev-parse HEAD)"
legacy_claim_key="pr:125:target:0.9.9"
legacy_reservation="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$legacy_repo" --source-sha "$legacy_source_sha" --version 0.9.9 --channel prod \
  --release-target 0.9.9 --owner github-actions[bot] --claim-key "$legacy_claim_key")"
legacy_reservation_id="$(printf '%s' "$legacy_reservation" | jq -r .reservationId)"
legacy_boundary_token="$(printf '%s' "$legacy_reservation" | jq -r '."Reservation-Boundary-Token"')"
python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$legacy_repo" --state released --version 0.9.9 --merge-sha "$legacy_source_sha" \
  --reservation-id "$legacy_reservation_id" --owner github-actions[bot] \
  --claim-key "$legacy_claim_key" --boundary-token "$legacy_boundary_token" \
  --reservation-ref refs/tags/release-reservation/v0.9.9 --maintainer-confirmed >/dev/null
legacy_reservation_sha="$(git -C "$legacy_repo" rev-parse refs/tags/release-reservation/v0.9.9^{commit})"
legacy_decision_sha="$(git -C "$legacy_repo" rev-parse refs/tags/release-decision/v0.9.9^{commit})"
legacy_released_sha="$(git -C "$legacy_repo" rev-parse "refs/tags/release-released/v0.9.9/${legacy_reservation_id}^{commit}")"
git -C "$legacy_repo" update-ref "refs/tags/release-consumed/v0.9.9/$legacy_source_sha" "$legacy_source_sha"
cat > "$tmp_dir/retirement-evidence.json" <<EOF
{"schema_version":1,"version":"0.9.9","main_sha":"$legacy_source_sha","product_tag":{"ref":"v0.9.9","status":"absent"},"github_release":{"tag":"v0.9.9","status":"absent"},"consumed_refs":[],"artifact_refs":[],"release_runs":[{"id":"35336297939","status":"completed","conclusion":"success","head_sha":"$legacy_source_sha","workflow_path":".github/workflows/release.yml","artifacts":[]}]}
EOF
if python3 "$root_dir/.github/scripts/release_reservation.py" retire \
  --local-root "$legacy_repo" --version 0.9.9 --reservation-id "$legacy_reservation_id" \
  --evidence-json "$tmp_dir/retirement-evidence.json" >/dev/null 2>&1; then
  echo "retirement accepted a stable consumed ref" >&2
  exit 1
fi
git -C "$legacy_repo" update-ref -d "refs/tags/release-consumed/v0.9.9/$legacy_source_sha"
retired="$(python3 "$root_dir/.github/scripts/release_reservation.py" retire \
  --local-root "$legacy_repo" --version 0.9.9 --reservation-id "$legacy_reservation_id" \
  --evidence-json "$tmp_dir/retirement-evidence.json")"
[[ "$(printf '%s' "$retired" | jq -r .ref)" == "refs/tags/release-retired/v0.9.9/$legacy_reservation_id" ]]
[[ "$(git -C "$legacy_repo" rev-parse refs/tags/release-reservation/v0.9.9^{commit})" == "$legacy_reservation_sha" ]]
[[ "$(git -C "$legacy_repo" rev-parse refs/tags/release-decision/v0.9.9^{commit})" == "$legacy_decision_sha" ]]
[[ "$(git -C "$legacy_repo" rev-parse "refs/tags/release-released/v0.9.9/${legacy_reservation_id}^{commit}")" == "$legacy_released_sha" ]]

reclaimed_claim_key="pr:204:target:0.9.9"
reclaimed="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$legacy_repo" --source-sha "$legacy_source_sha" --version 0.9.9 --channel prod \
  --release-target 0.9.9 --owner github-actions[bot] --claim-key "$reclaimed_claim_key")"
reclaimed_ref="$(printf '%s' "$reclaimed" | jq -r .ref)"
reclaimed_id="$(printf '%s' "$reclaimed" | jq -r .reservationId)"
[[ "$reclaimed_ref" == "refs/tags/release-reservation-reclaimed/v0.9.9/$reclaimed_id" ]]
python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$legacy_repo" --state released --version 0.9.9 --merge-sha "$legacy_source_sha" \
  --reservation-id "$reclaimed_id" --owner github-actions[bot] \
  --claim-key "$reclaimed_claim_key" --boundary-token "$(printf '%s' "$reclaimed" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref "$reclaimed_ref" --maintainer-confirmed >/dev/null
[[ "$(git -C "$legacy_repo" rev-parse "refs/tags/release-decision-reclaimed/v0.9.9/$reclaimed_id^{commit}")" =~ ^[0-9a-f]{40}$ ]]

echo "release reservation fixture tests passed"
