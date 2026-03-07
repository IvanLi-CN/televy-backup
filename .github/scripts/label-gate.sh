#!/usr/bin/env bash
set -euo pipefail

api_root="${GITHUB_API_URL:-https://api.github.com}"
repo="${GITHUB_REPOSITORY:-}"
token="${GITHUB_TOKEN:-}"
pr_number="${PR_NUMBER:-}"
labels_json_override="${LABELS_JSON:-}"

derive_pr_number_from_event() {
  if [[ -n "${pr_number}" || -z "${GITHUB_EVENT_PATH:-}" || ! -f "${GITHUB_EVENT_PATH}" ]]; then
    return 0
  fi

  pr_number="$(python3 - <<'PY'
from __future__ import annotations

import json
import os

path = os.environ.get("GITHUB_EVENT_PATH", "")
with open(path, "r", encoding="utf-8") as f:
    payload = json.load(f)
number = payload.get("pull_request", {}).get("number")
print(number if isinstance(number, int) else "")
PY
)"
}

fetch_labels_json() {
  if [[ -n "${labels_json_override}" ]]; then
    printf '%s\n' "${labels_json_override}"
    return 0
  fi

  if [[ -z "${repo}" ]]; then
    echo "label-gate: missing GITHUB_REPOSITORY" >&2
    exit 2
  fi

  if [[ -z "${token}" ]]; then
    echo "label-gate: missing GITHUB_TOKEN" >&2
    exit 2
  fi

  derive_pr_number_from_event
  if [[ -z "${pr_number}" ]]; then
    echo "label-gate: missing PR_NUMBER" >&2
    exit 2
  fi

  curl -fsSL \
    --max-time 15 \
    -H "Accept: application/vnd.github+json" \
    -H "Authorization: Bearer ${token}" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "${api_root}/repos/${repo}/issues/${pr_number}/labels?per_page=100"
}

labels_json="$(fetch_labels_json)"
export labels_json
python3 - <<'PY'
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


def fail(message: str) -> None:
    print(f"::error::{message}")
    print(f"Allowed intent labels: {', '.join(sorted(allowed_intents))}")
    print(f"Allowed channel labels: {', '.join(sorted(allowed_channels))}")
    print(f"Found labels: {', '.join(found) if found else '<none>'}")
    sys.exit(1)


if unknown_intents:
    fail(f"Unknown intent label(s): {', '.join(unknown_intents)}")

if unknown_channels:
    fail(f"Unknown channel label(s): {', '.join(unknown_channels)}")

if len(channel_present) == 0:
    fail("Missing channel label: PR must have exactly one channel label")

if len(channel_present) > 1:
    fail(f"Conflicting channel labels: {', '.join(channel_present)} (must be exactly one)")

if len(intent_present) == 0:
    fail("Missing intent label: PR must have exactly one intent label")

if len(intent_present) > 1:
    fail(f"Conflicting intent labels: {', '.join(intent_present)} (must be exactly one)")

intent = intent_present[0]
release_channel = "rc" if channel_present == ["channel:rc"] else "stable"
bump_level = "" if intent in {"type:docs", "type:skip"} else intent.removeprefix("type:")

out_path = os.environ.get("GITHUB_OUTPUT")
if out_path:
    with open(out_path, "a", encoding="utf-8") as f:
        f.write(f"release_intent_label={intent}\n")
        f.write(f"release_channel={release_channel}\n")
        f.write(f"bump_level={bump_level}\n")

print(f"Intent label OK: {intent}")
print(f"release_channel={release_channel}")
print(f"bump_level={bump_level or '<none>'}")
PY
