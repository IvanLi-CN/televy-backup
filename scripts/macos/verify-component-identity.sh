#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: verify-component-identity.sh --reference BUNDLE --candidate BUNDLE [--manifest FILE]" >&2
  exit 2
}
reference=""
candidate=""
manifest=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --reference) reference="${2:-}"; shift 2 ;;
    --candidate) candidate="${2:-}"; shift 2 ;;
    --manifest) manifest="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -d "$reference" && -d "$candidate" ]] || usage
[[ -z "$manifest" || -s "$manifest" ]] || usage
reference_binary="$reference/Contents/MacOS/televybackup-snapshot-access"
candidate_binary="$candidate/Contents/MacOS/televybackup-snapshot-access"
[[ -f "$reference_binary" && -f "$candidate_binary" ]] || {
  echo "Snapshot Access executable missing from identity check" >&2
  exit 1
}

artifact_sha() {
  python3 - "$1" <<'PY'
import hashlib, os, sys
path = sys.argv[1]
digest = hashlib.sha256()
if os.path.isfile(path):
    with open(path, 'rb') as handle:
        digest.update(handle.read())
else:
    for root, directories, files in os.walk(path, followlinks=False):
        directories.sort()
        files.sort()
        relative_root = os.path.relpath(root, path)
        if relative_root == '.':
            relative_root = ''
        for name in directories + files:
            entry = os.path.join(root, name)
            relative = os.path.join(relative_root, name)
            stat = os.lstat(entry)
            digest.update(b'entry\0' + relative.encode() + b'\0')
            digest.update(str(stat.st_mode).encode() + b'\0')
            if os.path.islink(entry):
                digest.update(b'link\0' + os.readlink(entry).encode() + b'\0')
            elif os.path.isfile(entry):
                with open(entry, 'rb') as handle:
                    digest.update(b'file\0' + handle.read())
            else:
                digest.update(b'other\0')
print(digest.hexdigest())
PY
}

reference_sha="$(shasum -a 256 "$reference_binary" | awk '{print $1}')"
candidate_sha="$(shasum -a 256 "$candidate_binary" | awk '{print $1}')"
[[ "$reference_sha" == "$candidate_sha" ]] || {
  echo "Snapshot Access SHA-256 changed: $reference_sha != $candidate_sha" >&2
  exit 1
}
reference_artifact_sha="$(artifact_sha "$reference")"
candidate_artifact_sha="$(artifact_sha "$candidate")"
[[ "$reference_artifact_sha" == "$candidate_artifact_sha" ]] || {
  echo "Snapshot Access bundle artifact changed: $reference_artifact_sha != $candidate_artifact_sha" >&2
  exit 1
}

reference_metadata="$("$reference_binary" --component-metadata)"
candidate_metadata="$("$candidate_binary" --component-metadata)"
[[ "$reference_metadata" == "$candidate_metadata" ]] || {
  echo "Snapshot Access component metadata changed" >&2
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

if [[ -n "$manifest" ]]; then
  reference_cdhash="$(printf '%s\n' "$reference_signature" | awk -F= '/^cdhash=/{print $2}')"
  reference_sha256="$reference_sha"
  python3 - "$manifest" "$reference_sha256" "$reference_artifact_sha" "$reference_cdhash" "$reference_requirement" "$candidate_metadata" <<'PY'
import json
import sys

component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_access"]
assert component["sha256"] == sys.argv[2]
assert component["artifact_sha256"] == sys.argv[3]
assert component["cdhash"] == sys.argv[4]
assert component["designated_requirement"] == sys.argv[5]
metadata = json.loads(sys.argv[6])
assert component["bundle_id"] == metadata["bundleId"]
assert component["relative_path"] == metadata["relativePath"]
assert component["component_version"] == metadata["componentVersion"]
assert component["protocol_version"] == metadata["protocolVersion"]
PY
fi

echo "Snapshot Access component identity is unchanged"
echo "sha256=$candidate_sha"
echo "artifact_sha256=$candidate_artifact_sha"
echo "$candidate_signature"
echo "designated=$candidate_requirement"
