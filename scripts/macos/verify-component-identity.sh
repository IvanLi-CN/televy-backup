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
  python3 - "$1" "${2:-raw}" <<'PY'
import hashlib, os, sys
import stat as stat_module
path = sys.argv[1]
mode_policy = sys.argv[2]
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
            entry_stat = os.lstat(entry)
            mode = entry_stat.st_mode
            if mode_policy == 'canonical':
                if stat_module.S_ISLNK(mode):
                    permissions = 0o777
                elif stat_module.S_ISDIR(mode) or mode & 0o111:
                    permissions = 0o755
                else:
                    permissions = 0o644
                mode = (mode & ~0o777) | permissions
            digest.update(b'entry\0' + relative.encode() + b'\0')
            digest.update(str(mode).encode() + b'\0')
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
reference_artifact_sha="$(artifact_sha "$reference" canonical)"
candidate_artifact_sha="$(artifact_sha "$candidate" canonical)"
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

# Universal ad-hoc signatures can print the commutative cdhash alternatives in
# slice-dependent order. Normalize only that presentation detail and the
# non-semantic comment marker; keep the rest of the requirement exact.
normalize_requirement() {
  python3 - "$1" <<'PY'
import re
import sys

requirement = sys.argv[1].strip()
requirement = re.sub(r"^#\s*", "", requirement)

def normalize_space(value):
    result = []
    pending_space = False
    in_quote = False
    escaped = False
    for character in value:
        if in_quote:
            result.append(character)
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_quote = False
        elif character == '"':
            if pending_space and result:
                result.append(" ")
                pending_space = False
            result.append(character)
            in_quote = True
        elif character.isspace():
            pending_space = True
        else:
            if pending_space and result:
                result.append(" ")
            pending_space = False
            result.append(character)
    return "".join(result).strip()

requirement = normalize_space(requirement)
cdhash_term = r'cdhash\s+H"([0-9A-Fa-f]+)"'
cdhash_or = re.compile(rf"{cdhash_term}(?:\s+or\s+{cdhash_term})+")

def sort_cdhash_alternatives(match):
    values = re.findall(cdhash_term, match.group(0))
    return " or ".join(f'cdhash H"{value.lower()}"' for value in sorted(values, key=str.lower))

print(cdhash_or.sub(sort_cdhash_alternatives, requirement))
PY
}

reference_requirement_raw="$(codesign -d -r- "$reference" 2>&1 | sed -n '/designated =>/p')"
candidate_requirement_raw="$(codesign -d -r- "$candidate" 2>&1 | sed -n '/designated =>/p')"
reference_requirement="$(normalize_requirement "$reference_requirement_raw")"
candidate_requirement="$(normalize_requirement "$candidate_requirement_raw")"
[[ -n "$reference_requirement" && "$reference_requirement" == "$candidate_requirement" ]] || {
  echo "Snapshot Access designated requirement changed" >&2
  printf 'reference_requirement=%q\n' "$reference_requirement" >&2
  printf 'candidate_requirement=%q\n' "$candidate_requirement" >&2
  exit 1
}

if [[ -n "$manifest" ]]; then
  reference_cdhash="$(printf '%s\n' "$reference_signature" | awk -F= '/^cdhash=/{print $2}')"
  reference_sha256="$reference_sha"
  # A DMG mount can normalize bundle file modes differently on Intel and
  # Apple Silicon. The manifest identity uses canonical bundle modes, while
  # reference/candidate require the same canonical local bundle digest. Keep
  # the raw digest as a compatibility path for older manifests generated
  # before canonical mode normalization.
  reference_legacy_artifact_sha="$(artifact_sha "$reference" raw)"
  python3 - "$manifest" "$reference_sha256" "$reference_artifact_sha" "$reference_legacy_artifact_sha" "$reference_cdhash" "$reference_requirement" "$candidate_metadata" <<'PY'
import json
import re
import sys

component = json.load(open(sys.argv[1], encoding="utf-8"))["components"]["snapshot_access"]
def require(condition, message):
    if not condition:
        raise SystemExit(message)

require(component["sha256"] == sys.argv[2], "Snapshot Access binary identity does not match the manifest")
require(
    component["artifact_sha256"] in {sys.argv[3], sys.argv[4]},
    "Snapshot Access bundle identity does not match the manifest",
)
requirement_cdhashes = {
    value.lower()
    for value in re.findall(r'\bcdhash\s+H"([0-9A-Fa-f]+)"', sys.argv[6])
}
require(
    requirement_cdhashes
    and sys.argv[5].lower() in requirement_cdhashes,
    "Snapshot Access observed CodeDirectory identity is not covered by its requirement",
)
manifest_requirement = component.get("designated_requirement")
require(
    isinstance(manifest_requirement, str) and manifest_requirement,
    "Snapshot Access designated requirement is missing from the manifest",
)
manifest_requirement = re.sub(r"^#\s*", "", manifest_requirement.strip())

def normalize_space(value):
    result = []
    pending_space = False
    in_quote = False
    escaped = False
    for character in value:
        if in_quote:
            result.append(character)
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_quote = False
        elif character == '"':
            if pending_space and result:
                result.append(" ")
                pending_space = False
            result.append(character)
            in_quote = True
        elif character.isspace():
            pending_space = True
        else:
            if pending_space and result:
                result.append(" ")
            pending_space = False
            result.append(character)
    return "".join(result).strip()

manifest_requirement = normalize_space(manifest_requirement)
actual_requirement = normalize_space(sys.argv[6])
cdhash_term = r'cdhash\s+H"([0-9A-Fa-f]+)"'
manifest_cdhashes = {
    value.lower() for value in re.findall(cdhash_term, manifest_requirement)
}
actual_cdhashes = {
    value.lower() for value in re.findall(cdhash_term, actual_requirement)
}
require(actual_cdhashes, "Snapshot Access designated requirement has no CDHash identities")
require(
    actual_cdhashes <= manifest_cdhashes,
    "Snapshot Access native requirement contains a CDHash not recorded in the manifest",
)
require(
    {component["cdhash"].lower(), sys.argv[5].lower()} <= manifest_cdhashes,
    "Snapshot Access CodeDirectory identity does not match the manifest requirement",
)
require(
    sys.argv[5].lower() in actual_cdhashes,
    "Snapshot Access observed CodeDirectory hash is not covered by its designated requirement",
)

cdhash_pattern = re.compile(cdhash_term, re.IGNORECASE)
cdhash_expression = re.compile(
    rf"{cdhash_term}(?:\s+or\s+{cdhash_term})*",
    re.IGNORECASE,
)
def requirement_shape(value, name):
    matches = list(cdhash_pattern.finditer(value))
    require(matches, f"{name} designated requirement has no CDHash identities")
    first = matches[0]
    last = matches[-1]
    segment = value[first.start():last.end()]
    require(
        cdhash_expression.fullmatch(segment) is not None,
        f"{name} designated requirement has invalid CDHash alternative syntax",
    )
    return value[:first.start()] + "__SNAPSHOT_ACCESS_CDHASHES__" + value[last.end():]

manifest_shape = requirement_shape(manifest_requirement, "manifest")
actual_shape = requirement_shape(actual_requirement, "actual")

require(
    manifest_shape == actual_shape,
    "Snapshot Access designated requirement does not match the manifest\n"
    f"manifest_shape={manifest_shape!r}\nactual_shape={actual_shape!r}",
)
metadata = json.loads(sys.argv[7])
require(component["bundle_id"] == metadata["bundleId"], "Snapshot Access bundle id does not match the manifest")
require(component["relative_path"] == metadata["relativePath"], "Snapshot Access relative path does not match the manifest")
require(
    component["component_version"] == metadata["componentVersion"],
    "Snapshot Access component version does not match the manifest",
)
require(
    component["protocol_version"] == metadata["protocolVersion"],
    "Snapshot Access protocol version does not match the manifest",
)
PY
fi

echo "Snapshot Access component identity is unchanged"
echo "sha256=$candidate_sha"
echo "artifact_sha256=$candidate_artifact_sha"
echo "$candidate_signature"
echo "designated=$candidate_requirement"
