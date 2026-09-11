#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: verify-component-identity.sh --reference BUNDLE --candidate BUNDLE" >&2
  exit 2
}
reference=""
candidate=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --reference) reference="${2:-}"; shift 2 ;;
    --candidate) candidate="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -d "$reference" && -d "$candidate" ]] || usage
reference_binary="$reference/Contents/MacOS/televybackup-snapshot-access"
candidate_binary="$candidate/Contents/MacOS/televybackup-snapshot-access"
[[ -f "$reference_binary" && -f "$candidate_binary" ]] || {
  echo "Snapshot Access executable missing from identity check" >&2
  exit 1
}

reference_sha="$(shasum -a 256 "$reference_binary" | awk '{print $1}')"
candidate_sha="$(shasum -a 256 "$candidate_binary" | awk '{print $1}')"
[[ "$reference_sha" == "$candidate_sha" ]] || {
  echo "Snapshot Access SHA-256 changed: $reference_sha != $candidate_sha" >&2
  exit 1
}

signature_value() {
  local bundle="$1"
  codesign -dvvv "$bundle" 2>&1 | awk -F= '/^CDHash=/{print "cdhash=" $2} /^Identifier=/{print "identifier=" $2}'
}
reference_signature="$(signature_value "$reference")"
candidate_signature="$(signature_value "$candidate")"
[[ "$reference_signature" == "$candidate_signature" ]] || {
  echo "Snapshot Access CodeDirectory identity changed" >&2
  diff -u <(printf '%s\n' "$reference_signature") <(printf '%s\n' "$candidate_signature") || true
  exit 1
}

reference_requirement="$(codesign -d -r- "$reference" 2>&1 | sed -n '/designated =>/p')"
candidate_requirement="$(codesign -d -r- "$candidate" 2>&1 | sed -n '/designated =>/p')"
[[ -n "$reference_requirement" && "$reference_requirement" == "$candidate_requirement" ]] || {
  echo "Snapshot Access designated requirement changed" >&2
  exit 1
}

echo "Snapshot Access component identity is unchanged"
echo "sha256=$candidate_sha"
echo "$candidate_signature"
echo "designated=$candidate_requirement"
