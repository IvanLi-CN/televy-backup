#!/usr/bin/env bash
set -euo pipefail

mode="${1:-}"
case "${mode}" in
  labels|completion) ;;
  *) echo "usage: $0 labels|completion" >&2; exit 2 ;;
esac

event_path="${GITHUB_EVENT_PATH:?}"
repository="${GITHUB_REPOSITORY:?}"
token="${GITHUB_TOKEN:-${GH_TOKEN:?}}"
api_root="${GITHUB_API_URL:-https://api.github.com}"
head_sha="${MERGE_GROUP_HEAD_SHA:?}"

api_pr_numbers="$(gh api "repos/${repository}/commits/${head_sha}/pulls?per_page=100" --jq '.[].number')"
pr_numbers="$(
  {
    EVENT_PATH="${event_path}" python3 - <<'PY'
import json
import os
import re

payload = json.load(open(os.environ["EVENT_PATH"], encoding="utf-8"))
head_ref = str(payload.get("merge_group", {}).get("head_ref", ""))
numbers = re.findall(r"(?:^|/)pr-([0-9]+)(?:-|$)", head_ref)
for number in dict.fromkeys(numbers):
    print(number)
PY
    printf '%s\n' "${api_pr_numbers}"
  } | sort -n -u
)"

if [[ -z "${pr_numbers}" ]]; then
  echo "merge-group gate: cannot resolve a pull request from ${head_sha}" >&2
  exit 1
fi

check_state() {
  local sha="$1"
  local name="$2"
  gh api "repos/${repository}/commits/${sha}/check-runs?filter=latest&per_page=100" |
    python3 -c '
import json
import sys

name = sys.argv[1]
payload = json.load(sys.stdin)
rows = [row for row in payload.get("check_runs", []) if row.get("name") == name]
if not rows:
    print("missing")
else:
    row = max(rows, key=lambda value: str(value.get("completed_at") or value.get("started_at") or ""))
    if row.get("status") != "completed":
        print("pending")
    elif row.get("conclusion") == "success":
        print("success")
    else:
        print("failed")
' "${name}"
}

for pr_number in ${pr_numbers}; do
  pr_json="${RUNNER_TEMP:-/tmp}/merge-group-pr-${pr_number}.json"
  gh api "repos/${repository}/pulls/${pr_number}" > "${pr_json}"
  test "$(jq -r '.state' "${pr_json}")" = open
  test "$(jq -r '.base.ref' "${pr_json}")" = main
  test "$(jq -r '.head.repo.full_name // empty' "${pr_json}")" = "${repository}"
  labels_json="$(jq -c '.labels' "${pr_json}")"
  PR_NUMBER="${pr_number}" LABELS_JSON="${labels_json}" GITHUB_TOKEN="${token}" GITHUB_API_URL="${api_root}" \
    GITHUB_REPOSITORY="${repository}" bash ./.github/scripts/label-gate.sh

  if [[ "${mode}" == completion ]]; then
    pr_head_sha="$(jq -r '.head.sha' "${pr_json}")"
    base_sha="$(jq -r '.base.sha' "${pr_json}")"
    git fetch --no-tags origin "${pr_head_sha}" "${base_sha}"
    verification_sha="${pr_head_sha}"
    prepared_json=''
    if prepared_json="$(python3 .github/scripts/release_chain.py verify-prepared --commit "${pr_head_sha}" 2>/dev/null)"; then
      verification_sha="$(printf '%s' "${prepared_json}" | jq -r .sourceSha)"
    fi
    checks_json="${RUNNER_TEMP:-/tmp}/merge-group-checks-${pr_number}.json"
    gh api "repos/${repository}/commits/${verification_sha}/check-runs?filter=latest&per_page=100" > "${checks_json}"
    labels_file="${RUNNER_TEMP:-/tmp}/merge-group-labels-${pr_number}.json"
    printf '%s' "${labels_json}" > "${labels_file}"
    completion_args=(
      --repo-root .
      --commit "${pr_head_sha}"
      --base "${base_sha}"
      --labels-json "${labels_file}"
      --checks-json "${checks_json}"
      --repository "${repository}"
      --token "${token}"
      --api-root "${api_root}"
      --require-github-verification
    )
    if [[ -n "${prepared_json}" ]]; then
      reservation_json="${RUNNER_TEMP:-/tmp}/merge-group-reservation-${pr_number}.json"
      jq -n \
        --arg ref "$(printf '%s' "${prepared_json}" | jq -r .reservationRef)" \
        --arg sourceSha "$(printf '%s' "${prepared_json}" | jq -r .sourceSha)" \
        --arg version "$(printf '%s' "${prepared_json}" | jq -r .version)" \
        --arg channel "$(printf '%s' "${prepared_json}" | jq -r .channel | sed 's/^channel://')" \
        --arg reservationId "$(printf '%s' "${prepared_json}" | jq -r .reservationId)" \
        --arg owner "$(printf '%s' "${prepared_json}" | jq -r .reservationOwner)" \
        --arg claimKey "$(printf '%s' "${prepared_json}" | jq -r .claimKey)" \
        --arg boundaryToken "$(printf '%s' "${prepared_json}" | jq -r .boundaryToken)" \
        '{ref:$ref,sourceSha:$sourceSha,version:$version,channel:$channel,reservationId:$reservationId,"Reservation-Owner":$owner,"Reservation-Claim-Key":$claimKey,"Reservation-Boundary-Token":$boundaryToken}' \
        > "${reservation_json}"
      completion_args+=(--reservation-json "${reservation_json}")
      release_mode="$(printf '%s' "${prepared_json}" | jq -r .mode)"
      completion_args+=(--release-mode "${release_mode}")
      if [[ "${release_mode}" == version-only-release-pr ]]; then
        completion_args+=(--covered-merge-sha "$(printf '%s' "${prepared_json}" | jq -r .coveredMergeSha)")
      fi
    fi
    required=("quality" "macOS Swift tests" "arm64 native package" "x86_64 native package" "Universal 2 assembly")
    deadline=$((SECONDS + 1800))
    while :; do
      pending=()
      for name in "${required[@]}"; do
        case "$(check_state "${head_sha}" "${name}")" in
          success) ;;
          pending) pending+=("${name}") ;;
          missing) pending+=("${name}") ;;
          failed) echo "merge-group gate: required check failed: ${name}" >&2; exit 1 ;;
          *) echo "merge-group gate: unexpected check state for ${name}" >&2; exit 1 ;;
        esac
      done
      if (( ${#pending[@]} == 0 )); then
        break
      fi
      if (( SECONDS >= deadline )); then
        echo "merge-group gate: timed out waiting for ${pending[*]} on ${head_sha}" >&2
        exit 1
      fi
      sleep 10
    done
    final_pr_json="${RUNNER_TEMP:-/tmp}/merge-group-final-pr-${pr_number}.json"
    gh api "repos/${repository}/pulls/${pr_number}" > "${final_pr_json}"
    test "$(jq -r '.state' "${final_pr_json}")" = open
    test "$(jq -r '.base.ref' "${final_pr_json}")" = main
    test "$(jq -r '.head.repo.full_name // empty' "${final_pr_json}")" = "${repository}"
    test "$(jq -r '.head.sha' "${final_pr_json}")" = "${pr_head_sha}"
    test "$(jq -r '.base.sha' "${final_pr_json}")" = "${base_sha}"
    labels_json="$(jq -c '.labels' "${final_pr_json}")"
    printf '%s' "${labels_json}" > "${labels_file}"
    gh api "repos/${repository}/commits/${verification_sha}/check-runs?filter=latest&per_page=100" > "${checks_json}"
    python3 .github/scripts/release_completion.py "${completion_args[@]}" \
      --allow-migration --migration-version 0.9.2
  fi
done
