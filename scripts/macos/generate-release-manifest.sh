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

def artifact_digest(path):
    digest = hashlib.sha256()
    if os.path.isfile(path):
        with open(path, 'rb') as handle:
            digest.update(handle.read())
        return digest.hexdigest()
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
    return digest.hexdigest()

def component_metadata(binary):
    result = subprocess.run([binary, '--component-metadata'], capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f'component metadata failed for {binary}: {result.stderr.strip()}')
    return json.loads(result.stdout)

def signing_identity(path, hash_path=None, artifact_path=None):
    details = subprocess.run(['codesign', '-dvvv', path], capture_output=True, text=True)
    cdhash = next(
        (line.split('=', 1)[1] for line in (details.stdout + details.stderr).splitlines()
         if line.startswith('CDHash=')),
        None,
    )
    requirement = subprocess.run(['codesign', '-d', '-r-', path], capture_output=True, text=True)
    designated_requirement = next(
        (
            line
            for line in (requirement.stdout + requirement.stderr).splitlines()
            if 'designated =>' in line
        ),
        None,
    )
    with open(hash_path or path, 'rb') as handle:
        sha256 = hashlib.sha256(handle.read()).hexdigest()
    return {
        'sha256': sha256,
        'artifact_sha256': artifact_digest(artifact_path or path),
        'cdhash': cdhash,
        'designated_requirement': designated_requirement,
    }

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
    'artifact_sha256': None,
    'cdhash': None,
    'designated_requirement': None,
}
if os.path.isfile(helper_binary):
    bundle = os.path.dirname(os.path.dirname(os.path.dirname(helper_binary)))
    metadata = component_metadata(helper_binary)
    helper['bundle_id'] = metadata['bundleId']
    helper['relative_path'] = metadata['relativePath']
    helper['component_version'] = metadata['componentVersion']
    helper['protocol_version'] = metadata['protocolVersion']
    helper.update(signing_identity(bundle, helper_binary, bundle))

root_helper_binary = os.path.join(
    asset_dir,
    'TelevyBackup.app',
    'Contents/MacOS/televybackup-snapshot-mount-helper',
)
root_helper = {
    'label': 'com.ivan.televybackup.snapshot-mount-helper',
    'install_path': '/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper',
    'binary': 'Contents/MacOS/televybackup-snapshot-mount-helper',
    'component_version': '0.1.0',
    'protocol_version': 1,
    'source': 'release-bundled-compatibility-reference',
    'identity_source': 'bundled-release-artifact',
    'installed_observation': 'manual-rc-acceptance-required',
    'update_policy': 'compatibility-check-only',
    'sha256': None,
    'artifact_sha256': None,
    'cdhash': None,
    'designated_requirement': None,
}
if os.path.isfile(root_helper_binary):
    root_helper.update(signing_identity(root_helper_binary, root_helper_binary, root_helper_binary))

components = {
    'snapshot_access': helper,
    'snapshot_mount_helper': root_helper,
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
