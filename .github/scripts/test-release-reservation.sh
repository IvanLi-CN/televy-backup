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
claim_key="pr:7:source:${source_sha}:type:type:patch:channel:channel:beta"

first="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --owner fixture --claim-key "$claim_key")"
second="$(python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --owner fixture --claim-key "$claim_key")"
[[ "$(printf '%s' "$first" | jq -r .target)" == "$(printf '%s' "$second" | jq -r .target)" ]]
[[ "$(git -C "$repo_dir" for-each-ref --format='%(refname)' refs/tags/release-reservation)" == "refs/tags/release-reservation/v1.0.0-beta.1" ]]

if python3 "$root_dir/.github/scripts/release_reservation.py" reserve \
  --local-root "$repo_dir" --source-sha "$source_sha" --version 1.0.0-beta.1 --channel beta \
  --owner foreign --claim-key "$claim_key" >/dev/null 2>&1; then
  echo "foreign reservation claim was accepted" >&2
  exit 1
fi

bound="$(python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state bound --version 1.0.0-beta.1 --merge-sha "$source_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1)"
consumed="$(python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state consumed --version 1.0.0-beta.1 --merge-sha "$source_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1)"
[[ "$(printf '%s' "$bound" | jq -r .ref)" == refs/tags/release-bound/* ]]
[[ "$(printf '%s' "$consumed" | jq -r .ref)" == refs/tags/release-consumed/* ]]

if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state bound --version 1.0.0-beta.1 --merge-sha "$source_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner foreign \
  --claim-key "$claim_key" --boundary-token "wrong" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 >/dev/null 2>&1; then
  echo "receipt ref was overwritten" >&2
  exit 1
fi

if python3 "$root_dir/.github/scripts/release_reservation.py" receipt \
  --local-root "$repo_dir" --state released --version 1.0.0-beta.1 --merge-sha "$source_sha" \
  --reservation-id "$(printf '%s' "$first" | jq -r .reservationId)" --owner fixture \
  --claim-key "$claim_key" --boundary-token "$(printf '%s' "$first" | jq -r '."Reservation-Boundary-Token"')" \
  --reservation-ref refs/tags/release-reservation/v1.0.0-beta.1 >/dev/null 2>&1; then
  echo "released receipt bypassed maintainer confirmation" >&2
  exit 1
fi

echo "release reservation fixture tests passed"
