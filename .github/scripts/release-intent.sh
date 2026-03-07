#!/usr/bin/env bash
set -euo pipefail

api_root="${GITHUB_API_URL:-https://api.github.com}"
repo="${GITHUB_REPOSITORY:-}"
token="${GITHUB_TOKEN:-}"
sha="${WORKFLOW_RUN_SHA:-${GITHUB_SHA:-}}"
pulls_json_override="${PULLS_JSON:-}"
labels_json_override="${LABELS_JSON:-}"

conservative_skip() {
  local reason="$1"

  echo "should_release=false"
  echo "bump_level="
  echo "release_intent_label="
  echo "release_channel=stable"
  echo "pr_number="
  echo "pr_url="
  echo "reason=${reason}"

  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    {
      echo "should_release=false"
      echo "bump_level="
      echo "release_intent_label="
      echo "release_channel=stable"
      echo "pr_number="
      echo "pr_url="
      echo "reason=${reason}"
    } >> "${GITHUB_OUTPUT}"
  fi
}

fetch_commit_pulls() {
  if [[ -n "${pulls_json_override}" ]]; then
    printf '%s\n' "${pulls_json_override}"
    return 0
  fi

  if [[ -z "${repo}" ]]; then
    echo "release-intent: missing GITHUB_REPOSITORY" >&2
    exit 2
  fi

  if [[ -z "${token}" ]]; then
    echo "release-intent: missing GITHUB_TOKEN" >&2
    exit 2
  fi

  if [[ -z "${sha}" ]]; then
    echo "release-intent: missing WORKFLOW_RUN_SHA (or GITHUB_SHA)" >&2
    exit 2
  fi

  curl -fsSL \
    --max-time 15 \
    -H "Accept: application/vnd.github+json" \
    -H "Authorization: Bearer ${token}" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "${api_root}/repos/${repo}/commits/${sha}/pulls?per_page=100"
}

fetch_pr_labels() {
  local pr_number="$1"

  if [[ -n "${labels_json_override}" ]]; then
    printf '%s\n' "${labels_json_override}"
    return 0
  fi

  curl -fsSL \
    --max-time 15 \
    -H "Accept: application/vnd.github+json" \
    -H "Authorization: Bearer ${token}" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "${api_root}/repos/${repo}/issues/${pr_number}/labels?per_page=100"
}

pulls_json=""
if ! pulls_json="$(fetch_commit_pulls)"; then
  echo "::warning::release-intent: GitHub API failed while mapping commit to PR (sha=${sha}); conservative skip"
  conservative_skip "api_failure:commit_pulls"
  exit 0
fi

export pulls_json
pull_resolution="$(python3 - <<'PY'
from __future__ import annotations

import json
import os
import sys

pulls = json.loads(os.environ["pulls_json"])
if not isinstance(pulls, list):
    print("count=0")
    sys.exit(0)

count = len(pulls)
print(f"count={count}")
if count != 1:
    sys.exit(0)

pr = pulls[0]
pr_number = pr.get("number")
pr_url = pr.get("html_url", "")
if not isinstance(pr_number, int):
    sys.exit(0)

print(f"pr_number={pr_number}")
print(f"pr_url={pr_url}")
PY
)"

count="$(printf '%s\n' "${pull_resolution}" | sed -n 's/^count=//p')"
pr_number="$(printf '%s\n' "${pull_resolution}" | sed -n 's/^pr_number=//p')"
pr_url="$(printf '%s\n' "${pull_resolution}" | sed -n 's/^pr_url=//p')"

if [[ "${count:-0}" != "1" ]]; then
  echo "::notice::release-intent: commit ${sha:-<unknown>} maps to ${count:-0} PR(s); conservative skip"
  conservative_skip "ambiguous_or_missing_pr(count=${count:-0})"
  exit 0
fi

labels_json=""
if ! labels_json="$(fetch_pr_labels "${pr_number}")"; then
  echo "::warning::release-intent: GitHub API failed while reading PR labels (pr=${pr_number}); conservative skip"
  conservative_skip "api_failure:pr_labels"
  exit 0
fi

export labels_json
export pr_number
export pr_url
decision="$(python3 - <<'PY'
from __future__ import annotations

import json
import os
import sys

allowed_intents = {
    "type:docs",
    "type:skip",
    "type:patch",
    "type:minor",
    "type:major",
}
allowed_channels = {
    "channel:stable",
    "channel:rc",
}

labels = json.loads(os.environ["labels_json"])
names = [label.get("name", "") for label in labels if isinstance(label, dict)]
found = sorted(set(filter(None, names)))

intent_like = sorted({name for name in found if name.startswith("type:")})
unknown_intents = sorted({name for name in intent_like if name not in allowed_intents})
intent_present = sorted({name for name in found if name in allowed_intents})

channel_like = sorted({name for name in found if name.startswith("channel:")})
unknown_channels = sorted({name for name in channel_like if name not in allowed_channels})
channel_present = sorted({name for name in found if name in allowed_channels})

if unknown_intents:
    print("should_release=false")
    print("bump_level=")
    print("release_intent_label=")
    print("release_channel=stable")
    print(f"reason=unknown_intent_label({','.join(unknown_intents)})")
    sys.exit(0)

if unknown_channels:
    print("should_release=false")
    print("bump_level=")
    print("release_intent_label=")
    print("release_channel=stable")
    print(f"reason=unknown_channel_label({','.join(unknown_channels)})")
    sys.exit(0)

if len(channel_present) != 1:
    print("should_release=false")
    print("bump_level=")
    print("release_intent_label=")
    print("release_channel=stable")
    print(f"reason=invalid_channel_label_count({len(channel_present)})")
    sys.exit(0)

release_channel = "rc" if channel_present == ["channel:rc"] else "stable"

if len(intent_present) != 1:
    print("should_release=false")
    print("bump_level=")
    print("release_intent_label=")
    print(f"release_channel={release_channel}")
    print(f"reason=invalid_intent_label_count({len(intent_present)})")
    sys.exit(0)

intent_label = intent_present[0]
if intent_label in {"type:docs", "type:skip"}:
    print("should_release=false")
    print("bump_level=")
    print(f"release_intent_label={intent_label}")
    print(f"release_channel={release_channel}")
    print("reason=intent_skip")
    sys.exit(0)

print("should_release=true")
print(f"bump_level={intent_label.removeprefix('type:')}")
print(f"release_intent_label={intent_label}")
print(f"release_channel={release_channel}")
print("reason=intent_release")
PY
)"

should_release="$(printf '%s\n' "${decision}" | sed -n 's/^should_release=//p')"
bump_level="$(printf '%s\n' "${decision}" | sed -n 's/^bump_level=//p')"
intent_label="$(printf '%s\n' "${decision}" | sed -n 's/^release_intent_label=//p')"
release_channel="$(printf '%s\n' "${decision}" | sed -n 's/^release_channel=//p')"
reason="$(printf '%s\n' "${decision}" | sed -n 's/^reason=//p')"

echo "Release intent decision:"
echo "  sha=${sha}"
echo "  pr_number=${pr_number}"
echo "  intent_label=${intent_label:-<none>}"
echo "  release_channel=${release_channel:-stable}"
echo "  should_release=${should_release}"
echo "  bump_level=${bump_level:-<none>}"
echo "  reason=${reason}"

if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  {
    echo "should_release=${should_release}"
    echo "bump_level=${bump_level}"
    echo "release_intent_label=${intent_label}"
    echo "release_channel=${release_channel:-stable}"
    echo "pr_number=${pr_number}"
    echo "pr_url=${pr_url}"
    echo "reason=${reason}"
  } >> "${GITHUB_OUTPUT}"
fi
