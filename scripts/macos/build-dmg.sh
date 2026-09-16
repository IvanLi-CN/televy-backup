#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: build-dmg.sh --source-dir DIR --volume-name NAME --output FILE" >&2
  exit 2
}

source_dir=""
volume_name=""
output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --source-dir) source_dir="${2:-}"; shift 2 ;;
    --volume-name) volume_name="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -d "$source_dir/TelevyBackup.app" && -n "$volume_name" && -n "$output" ]] || usage

root_dir="$(git rev-parse --show-toplevel)"
layout_path="$root_dir/assets/brand/macos/dmg/layout.json"
asset_dir="$(dirname "$layout_path")"
python3 - "$layout_path" "$asset_dir" <<'PY'
import hashlib
import json
import pathlib
import sys

layout_path = pathlib.Path(sys.argv[1])
asset_dir = pathlib.Path(sys.argv[2])
layout = json.loads(layout_path.read_text(encoding="utf-8"))
for name, expected in layout["asset_digests"].items():
    path = asset_dir / name
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise SystemExit(f"DMG asset digest mismatch: {path}: {actual} != {expected}")
PY

[[ "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["format"])' "$layout_path")" == "UDZO" ]] || {
  echo "DMG layout must use UDZO" >&2
  exit 1
}

venv_dir="$(mktemp -d "${TMPDIR:-/tmp}/televybackup-dmgbuild.XXXXXX")"
trap 'rm -rf "$venv_dir"' EXIT
python3 -m venv "$venv_dir"
"$venv_dir/bin/python" -m pip install --disable-pip-version-check --no-input --only-binary=:all: --require-hashes -r "$root_dir/scripts/macos/dmgbuild-requirements.txt" >/dev/null

export TELEVYBACKUP_ROOT_DIR="$root_dir"
export TELEVYBACKUP_DMG_SOURCE_DIR="$(cd "$source_dir" && pwd -P)"
export TELEVYBACKUP_DMG_VOLUME_NAME="$volume_name"
export TELEVYBACKUP_DMG_OUTPUT="$(cd "$(dirname "$output")" && pwd -P)/$(basename "$output")"
dmg_version="$("$venv_dir/bin/python" -c 'import dmgbuild; print(dmgbuild.__version__)')"
[[ "$dmg_version" == "1.6.7" ]] || {
  echo "unexpected dmgbuild version: $dmg_version" >&2
  exit 1
}
generated_background="$venv_dir/generated-background-composed.png"
xcrun swift "$root_dir/scripts/macos/generate-dmg-overlay.swift" \
  --layout "$layout_path" \
  --background "$asset_dir/$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["background"])' "$layout_path")" \
  "$generated_background"
expected_background_digest="$(python3 -c 'import json,sys; l=json.load(open(sys.argv[1])); print(l["asset_digests"][l["composed_background"]])' "$layout_path")"
actual_background_digest="$(shasum -a 256 "$generated_background" | awk '{print $1}')"
[[ "$actual_background_digest" == "$expected_background_digest" ]] || {
  echo "generated DMG background does not match the checked-in layout digest" >&2
  exit 1
}
"$venv_dir/bin/dmgbuild" -s "$root_dir/scripts/macos/dmgbuild-settings.py" "$volume_name" "$TELEVYBACKUP_DMG_OUTPUT"
echo "built $(basename "$output") with dmgbuild $dmg_version"
