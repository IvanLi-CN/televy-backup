#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

fake_bin="$tmp_dir/bin"
reference="$tmp_dir/reference/TelevyBackup Snapshot Access.app"
candidate="$tmp_dir/candidate/TelevyBackup Snapshot Access.app"
manifest="$tmp_dir/BUILD-MANIFEST.json"
mkdir -p "$fake_bin" "$reference/Contents/MacOS" "$reference/Contents/_CodeSignature"

cat > "$reference/Contents/MacOS/televybackup-snapshot-access" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "--component-metadata" ]] || exit 2
printf '%s\n' '{"bundleId":"com.ivan.televybackup.snapshot-access","relativePath":"Contents/Library/LoginItems/TelevyBackup Snapshot Access.app","componentVersion":"0.2.0","protocolVersion":2}'
SH
chmod 700 "$reference/Contents/MacOS/televybackup-snapshot-access"
printf '%s\n' fixture > "$reference/Contents/Info.plist"
printf '%s\n' fixture > "$reference/Contents/_CodeSignature/CodeResources"
chmod 600 "$reference/Contents/Info.plist" "$reference/Contents/_CodeSignature/CodeResources"
chmod 700 "$reference" "$reference/Contents" "$reference/Contents/MacOS" "$reference/Contents/_CodeSignature"

mkdir -p "$(dirname "$candidate")"
cp -R "$reference" "$candidate"

cat > "$fake_bin/codesign" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "-dvvv" ]]; then
  printf '%s\n' 'Signature=adhoc' 'Identifier=com.ivan.televybackup.snapshot-access' 'CDHash=fixture-cdhash' >&2
elif [[ "${1:-}" == "-d" && "${2:-}" == "-r-" ]]; then
  printf '%s\n' 'designated => identifier "com.ivan.televybackup.snapshot-access"' >&2
else
  exit 2
fi
SH
chmod 755 "$fake_bin/codesign"

python3 - "$manifest" "$reference" <<'PY'
import hashlib
import json
import os
import stat
import sys

manifest_path, bundle = sys.argv[1:]

def digest(path):
    result = hashlib.sha256()
    for root, directories, files in os.walk(path, followlinks=False):
        directories.sort()
        files.sort()
        relative_root = os.path.relpath(root, path)
        if relative_root == ".":
            relative_root = ""
        for name in directories + files:
            entry = os.path.join(root, name)
            relative = os.path.join(relative_root, name)
            entry_stat = os.lstat(entry)
            if stat.S_ISLNK(entry_stat.st_mode):
                permissions = 0o777
            elif stat.S_ISDIR(entry_stat.st_mode) or entry_stat.st_mode & 0o111:
                permissions = 0o755
            else:
                permissions = 0o644
            mode = (entry_stat.st_mode & ~0o777) | permissions
            result.update(b"entry\0" + relative.encode() + b"\0")
            result.update(str(mode).encode() + b"\0")
            if os.path.islink(entry):
                result.update(b"link\0" + os.readlink(entry).encode() + b"\0")
            elif os.path.isfile(entry):
                with open(entry, "rb") as handle:
                    result.update(b"file\0" + handle.read())
            else:
                result.update(b"other\0")
    return result.hexdigest()

binary = os.path.join(bundle, "Contents/MacOS/televybackup-snapshot-access")
payload = {
    "components": {
        "snapshot_access": {
            "sha256": hashlib.sha256(open(binary, "rb").read()).hexdigest(),
            "artifact_sha256": digest(bundle),
            "cdhash": "fixture-cdhash",
            "designated_requirement": 'designated => identifier "com.ivan.televybackup.snapshot-access"',
            "bundle_id": "com.ivan.televybackup.snapshot-access",
            "relative_path": "Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
            "component_version": "0.2.0",
            "protocol_version": 2,
        }
    }
}
with open(manifest_path, "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY

identity_script="$root_dir/scripts/macos/verify-component-identity.sh"
PATH="$fake_bin:$PATH" bash "$identity_script" \
  --reference "$reference" \
  --candidate "$candidate" \
  --manifest "$manifest" >/dev/null

printf '%s\n' changed >> "$candidate/Contents/Info.plist"
if PATH="$fake_bin:$PATH" bash "$identity_script" \
  --reference "$reference" \
  --candidate "$candidate" \
  --manifest "$manifest" >/dev/null 2>&1; then
  echo "component identity accepted a changed bundle" >&2
  exit 1
fi

cp -R "$reference" "$candidate"
python3 - "$manifest" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
payload["components"]["snapshot_access"]["artifact_sha256"] = "0" * 64
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY
if PATH="$fake_bin:$PATH" bash "$identity_script" \
  --reference "$reference" \
  --candidate "$candidate" \
  --manifest "$manifest" >/dev/null 2>&1; then
  echo "component identity accepted a mismatched source artifact digest" >&2
  exit 1
fi

echo "component identity fixture tests passed"
