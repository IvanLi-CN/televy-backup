#!/usr/bin/env bash
set -euo pipefail

usage() { echo "usage: generate-release-manifest.sh --version VERSION --asset-dir DIR --source-commit SHA --packaging-commit SHA --output FILE" >&2; exit 2; }
version=""; asset_dir=""; source_commit=""; packaging_commit=""; output=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) version="${2:-}"; shift 2 ;;
    --asset-dir) asset_dir="${2:-}"; shift 2 ;;
    --source-commit) source_commit="${2:-}"; shift 2 ;;
    --packaging-commit) packaging_commit="${2:-}"; shift 2 ;;
    --output) output="${2:-}"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$version" && -d "$asset_dir" && -n "$source_commit" && -n "$packaging_commit" && -n "$output" ]] || usage
python3 - "$version" "$asset_dir" "$source_commit" "$packaging_commit" "$output" <<'PY'
import hashlib, json, os, platform, sys
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
    'assets': assets,
}
with open(output, 'w', encoding='utf-8') as handle:
    json.dump(manifest, handle, indent=2, sort_keys=True)
    handle.write('\n')
PY
