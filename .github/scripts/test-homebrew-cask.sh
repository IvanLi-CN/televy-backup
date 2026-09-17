#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

ruby -c "$root_dir/Casks/televybackup.rb"
python3 -c 'from pathlib import Path; compile(Path(__import__("sys").argv[1]).read_text(encoding="utf-8"), __import__("sys").argv[1], "exec")' \
  "$root_dir/scripts/homebrew/cask_release.py"

version="1.2.3"
dmg_name="TelevyBackup-${version}.dmg"
digest="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
printf '%s  %s\n' "$digest" "$dmg_name" > "$tmp_dir/SHA256SUMS"
cat > "$tmp_dir/BUILD-MANIFEST.json" <<JSON
{
  "architectures": ["arm64", "x86_64", "universal2"],
  "assets": [{"name": "$dmg_name", "sha256": "$digest", "bytes": 123}],
  "product": "TelevyBackup",
  "release_version": "$version",
  "schema_version": 1
}
JSON

python3 "$root_dir/scripts/homebrew/cask_release.py" render \
  --version "$version" \
  --checksums "$tmp_dir/SHA256SUMS" \
  --manifest "$tmp_dir/BUILD-MANIFEST.json" \
  --output "$tmp_dir/televybackup.rb"
python3 "$root_dir/scripts/homebrew/cask_release.py" verify-cask \
  --cask "$tmp_dir/televybackup.rb" \
  --version "$version" \
  --checksums "$tmp_dir/SHA256SUMS" \
  --manifest "$tmp_dir/BUILD-MANIFEST.json"

python3 - "$root_dir" "$tmp_dir" "$dmg_name" "$version" <<'PY'
import hashlib
import importlib.util
import json
import plistlib
import sys
from pathlib import Path

root_dir, tmp_dir, dmg_name, version = sys.argv[1:]
module_spec = importlib.util.spec_from_file_location(
    "cask_release", Path(root_dir) / "scripts/homebrew/cask_release.py"
)
assert module_spec and module_spec.loader
cask_release = importlib.util.module_from_spec(module_spec)
module_spec.loader.exec_module(cask_release)

dmg = Path(tmp_dir) / dmg_name
dmg.write_bytes(b"fixture")
fixture_digest = hashlib.sha256(dmg.read_bytes()).hexdigest()
checksums = Path(tmp_dir) / "fixture-SHA256SUMS"
checksums.write_text(f"{fixture_digest}  {dmg.name}\n", encoding="utf-8")
manifest = Path(tmp_dir) / "fixture-BUILD-MANIFEST.json"
manifest.write_text(
    json.dumps(
        {
            "architectures": ["arm64", "x86_64", "universal2"],
            "assets": [{"name": dmg.name, "sha256": fixture_digest, "bytes": 7}],
            "product": "TelevyBackup",
            "release_version": version,
        }
    ),
    encoding="utf-8",
)

def fake_command_output(args):
    if args[:2] == ["hdiutil", "verify"]:
        return ""
    if args[:2] == ["hdiutil", "attach"]:
        mount_point = Path(args[args.index("-mountpoint") + 1])
        app = mount_point / "TelevyBackup.app"
        (app / "Contents/MacOS").mkdir(parents=True)
        (app / "Contents/Info.plist").write_bytes(
            plistlib.dumps(
                {
                    "CFBundleIdentifier": "com.ivan.televybackup",
                    "CFBundleExecutable": "TelevyBackup",
                    "LSMinimumSystemVersion": "15.0",
                }
            )
        )
        (app / "Contents/MacOS/TelevyBackup").touch()
        alias_mount_point = mount_point / ".." / mount_point.name
        return plistlib.dumps(
            {
                "system-entities": [
                    {"dev-entry": "/dev/disk-test", "mount-point": str(alias_mount_point)}
                ]
            }
        ).decode("utf-8")
    if args[:2] == ["hdiutil", "detach"]:
        return ""
    if args[:2] == ["lipo", "-info"]:
        return "Architectures in the fat file: arm64 x86_64"
    raise AssertionError(args)

cask_release.command_output = fake_command_output
cask_release.verify_dmg(dmg, version, checksums, manifest)
PY

printf '56180c32798b74be199c3bcbbef0f025107fd93859651f81aef80d1770a7ced8  TelevyBackup-0.9.8.dmg\n' > "$tmp_dir/stable-SHA256SUMS"
printf '%s\n' '{"architectures":["arm64","x86_64","universal2"],"assets":[{"name":"TelevyBackup-0.9.8.dmg","sha256":"56180c32798b74be199c3bcbbef0f025107fd93859651f81aef80d1770a7ced8","bytes":1}],"product":"TelevyBackup","release_version":"0.9.8"}' > "$tmp_dir/stable-BUILD-MANIFEST.json"
python3 "$root_dir/scripts/homebrew/cask_release.py" render \
  --version 0.9.8 \
  --checksums "$tmp_dir/stable-SHA256SUMS" \
  --manifest "$tmp_dir/stable-BUILD-MANIFEST.json" \
  --output "$tmp_dir/stable.rb"
if ! cmp "$root_dir/Casks/televybackup.rb" "$tmp_dir/stable.rb"; then
  diff -u "$root_dir/Casks/televybackup.rb" "$tmp_dir/stable.rb" >&2
  exit 1
fi

if python3 "$root_dir/scripts/homebrew/cask_release.py" render \
  --version 1.2.3-rc.1 \
  --checksums "$tmp_dir/SHA256SUMS" \
  --manifest "$tmp_dir/BUILD-MANIFEST.json" \
  --output "$tmp_dir/invalid.rb" >/dev/null 2>&1; then
  echo "prerelease Cask version was accepted" >&2
  exit 1
fi

[[ ! -e "$root_dir/packaging/homebrew/televybackup.rb" ]]
[[ -e "$root_dir/Casks/televybackup.rb" ]]
for workflow in homebrew-cask.yml homebrew-cask-update.yml; do
  ruby -ryaml -e 'YAML.parse_file(ARGV.fetch(0))' "$root_dir/.github/workflows/$workflow"
done
workflow_text="$(<"$root_dir/.github/workflows/homebrew-cask.yml")"
[[ "$workflow_text" == *'contents: read'* ]]
[[ "$workflow_text" != *'contents: write'* ]]
[[ "$workflow_text" != *'pull-requests: write'* ]]
[[ "$workflow_text" != *'actions: write'* ]]
[[ "$workflow_text" != *'secrets.'* ]]
[[ "$workflow_text" != *'gh pr create'* ]]
[[ "$workflow_text" != *'gh workflow run'* ]]
update_workflow_text="$(<"$root_dir/.github/workflows/homebrew-cask-update.yml")"
[[ "$update_workflow_text" == *'workflow_run:'* ]]
[[ "$update_workflow_text" == *'workflows: [Release Product]'* ]]
[[ "$update_workflow_text" == *'actions: write'* ]]
[[ "$update_workflow_text" == *'contents: write'* ]]
[[ "$update_workflow_text" == *'issues: write'* ]]
[[ "$update_workflow_text" == *'pull-requests: write'* ]]
[[ "$update_workflow_text" == *'GITHUB_REPOSITORY'* ]]
[[ "$update_workflow_text" == *'github.token'* ]]
[[ "$update_workflow_text" == *'gh pr create'* ]]
[[ "$update_workflow_text" == *'gh workflow run ci-pr.yml'* ]]
[[ "$update_workflow_text" == *'gh workflow run package-ci.yml'* ]]
[[ "$update_workflow_text" == *'gh workflow run label-gate.yml'* ]]
[[ "$update_workflow_text" == *'gh workflow run release-completion.yml'* ]]
[[ "$update_workflow_text" == *'gh workflow run homebrew-cask.yml'* ]]
[[ "$update_workflow_text" == *'gh pr merge'* ]]
[[ "$update_workflow_text" == *'--match-head-commit'* ]]
[[ "$update_workflow_text" == *'Homebrew Cask audit'* ]]
[[ "$update_workflow_text" != *'homebrew-cask.git'* ]]
[[ "$update_workflow_text" != *'secrets.'* ]]
[[ "$update_workflow_text" != *'PAT'* ]]

echo "Homebrew Cask contract tests passed"
