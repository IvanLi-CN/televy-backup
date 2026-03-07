#!/usr/bin/env bash
set -euo pipefail

out_json="${1:-}"
if [[ -z "${out_json}" ]]; then
  echo "Usage: freeze-release-intent.sh <output-json>" >&2
  exit 2
fi

tmp_output="$(mktemp)"
trap 'rm -f "$tmp_output"' EXIT

GITHUB_OUTPUT="$tmp_output" bash ./.github/scripts/release-intent.sh >/dev/null

python3 - "$tmp_output" "$out_json" <<'PY'
from __future__ import annotations

import json
import sys
from pathlib import Path

src = Path(sys.argv[1])
out = Path(sys.argv[2])
data = {}
for line in src.read_text(encoding="utf-8").splitlines():
    if not line or "=" not in line:
        continue
    key, value = line.split("=", 1)
    data[key] = value
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
