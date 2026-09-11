#!/usr/bin/env bash
set -euo pipefail

usage() { echo "usage: generate-release-manifest.sh --mode release|development --asset-dir DIR --source-commit SHA --packaging-commit SHA --output FILE" >&2; exit 2; }
mode=""; asset_dir=""; source_commit=""; packaging_commit=""; output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --mode) mode="${2:-}"; shift 2 ;;
    --asset-dir) asset_dir="${2:-}"; shift 2 ;;
    --source-commit) source_commit="${2:-}"; shift 2 ;;
    --packaging-commit) packaging_commit="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$mode" && -d "$asset_dir" && -n "$source_commit" && -n "$packaging_commit" && -n "$output" ]] || usage
[[ "$mode" == "release" || "$mode" == "development" ]] || usage
root_dir="$(git rev-parse --show-toplevel)"
version="$(python3 "$root_dir/scripts/product-version.py" --mode "$mode" --source-sha "$source_commit")"
python3 - "$version" "$asset_dir" "$source_commit" "$packaging_commit" "$output" <<'PY'
import hashlib, json, os, platform, subprocess, sys
version, asset_dir, source, packaging, output = sys.argv[1:]
names = sorted(name for name in os.listdir(asset_dir) if name.endswith(('.dmg', '.tar.gz')))
assets = []
for name in names:
    path = os.path.join(asset_dir, name)
    with open(path, 'rb') as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    assets.append({'name': name, 'sha256': digest, 'bytes': os.path.getsize(path)})
with open(os.path.join(asset_dir, 'SHA256SUMS'), 'w', encoding='utf-8') as handle:
    for asset in assets:
        handle.write(f"{asset['sha256']}  {asset['name']}\n")

helper_binary = os.path.join(
    asset_dir,
    'TelevyBackup.app',
    'Contents/Library/LoginItems/TelevyBackup Snapshot Access.app',
    'Contents/MacOS/televybackup-snapshot-access',
)
helper = {
    'bundle_id': 'com.ivan.televybackup.snapshot-access',
    'relative_path': 'Contents/Library/LoginItems/TelevyBackup Snapshot Access.app',
    'binary': 'Contents/MacOS/televybackup-snapshot-access',
    'component_version': '0.2.0',
    'protocol_version': 2,
    'source': 'fresh-rc1-build' if version.endswith('-rc.1') else 'rc1-universal-artifact',
    'reuse_policy': 'byte-identical-no-rebuild-no-lipo-no-resign',
    'sha256': None,
    'cdhash': None,
    'designated_requirement': None,
}
if os.path.isfile(helper_binary):
    with open(helper_binary, 'rb') as handle:
        helper['sha256'] = hashlib.sha256(handle.read()).hexdigest()
    bundle = os.path.dirname(os.path.dirname(os.path.dirname(helper_binary)))
    details = subprocess.run(['codesign', '-dvvv', bundle], capture_output=True, text=True)
    for line in details.stderr.splitlines():
        if line.startswith('CDHash='):
            helper['cdhash'] = line.split('=', 1)[1]
    requirement = subprocess.run(['codesign', '-d', '-r-', bundle], capture_output=True, text=True)
    helper['designated_requirement'] = next(
        (line for line in requirement.stdout.splitlines() if 'designated =>' in line), None
    )

components = {
    'snapshot_access': helper,
    'snapshot_mount_helper': {
        'label': 'com.ivan.televybackup.snapshot-mount-helper',
        'install_path': '/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper',
        'component_version': '0.1.0',
        'protocol_version': 1,
        'update_policy': 'compatibility-check-only',
    },
}
manifest = {
    'schema_version': 1,
    'product': 'TelevyBackup',
    'release_version': version,
    'source_commit': source,
    'packaging_commit': packaging,
    'toolchain': os.environ.get('RUST_TOOLCHAIN', '1.91.0'),
    'runner': platform.platform(),
    'architectures': ['arm64', 'x86_64', 'universal2'],
    'signing': 'ad-hoc',
    'components': components,
    'assets': assets,
}
with open(output, 'w', encoding='utf-8') as handle:
    json.dump(manifest, handle, indent=2, sort_keys=True)
    handle.write('\n')
PY
