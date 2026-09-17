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

fake_bin="$tmp_dir/fake-bin"
mkdir -p "$fake_bin"
cat > "$fake_bin/hdiutil" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  verify)
    exit 0
    ;;
  attach)
    mount_point=""
    while (($#)); do
      case "$1" in
        -mountpoint) mount_point="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    mkdir -p "$mount_point/TelevyBackup.app/Contents/MacOS"
    cat > "$mount_point/TelevyBackup.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.ivan.televybackup</string>
<key>CFBundleExecutable</key><string>TelevyBackup</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
</dict></plist>
PLIST
    touch "$mount_point/TelevyBackup.app/Contents/MacOS/TelevyBackup"
    alias_mount_point="$mount_point/../$(basename "$mount_point")"
    printf '%s\n' "<?xml version=\"1.0\" encoding=\"UTF-8\"?><plist version=\"1.0\"><dict><key>system-entities</key><array><dict><key>dev-entry</key><string>/dev/disk-test</string><key>mount-point</key><string>$alias_mount_point</string></dict></array></dict></plist>"
    ;;
  detach)
    exit 0
    ;;
  *)
    exit 2
    ;;
esac
SH
chmod +x "$fake_bin/hdiutil"
cat > "$fake_bin/lipo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "-info" ]]
printf 'Architectures in the fat file: arm64 x86_64\n'
SH
chmod +x "$fake_bin/lipo"
printf 'fixture' > "$tmp_dir/$dmg_name"
fixture_digest="$(python3 - "$tmp_dir/$dmg_name" <<'PY'
import hashlib
import pathlib
import sys

print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
)"
printf '%s  %s\n' "$fixture_digest" "$dmg_name" > "$tmp_dir/fixture-SHA256SUMS"
printf '%s\n' "{\"architectures\":[\"arm64\",\"x86_64\",\"universal2\"],\"assets\":[{\"name\":\"$dmg_name\",\"sha256\":\"$fixture_digest\",\"bytes\":7}],\"product\":\"TelevyBackup\",\"release_version\":\"$version\"}" > "$tmp_dir/fixture-BUILD-MANIFEST.json"
PATH="$fake_bin:$PATH" python3 "$root_dir/scripts/homebrew/cask_release.py" verify-dmg \
  --dmg "$tmp_dir/$dmg_name" \
  --version "$version" \
  --checksums "$tmp_dir/fixture-SHA256SUMS" \
  --manifest "$tmp_dir/fixture-BUILD-MANIFEST.json"

printf '56180c32798b74be199c3bcbbef0f025107fd93859651f81aef80d1770a7ced8  TelevyBackup-0.9.8.dmg\n' > "$tmp_dir/stable-SHA256SUMS"
printf '%s\n' '{"architectures":["arm64","x86_64","universal2"],"assets":[{"name":"TelevyBackup-0.9.8.dmg","sha256":"56180c32798b74be199c3bcbbef0f025107fd93859651f81aef80d1770a7ced8","bytes":1}],"product":"TelevyBackup","release_version":"0.9.8"}' > "$tmp_dir/stable-BUILD-MANIFEST.json"
python3 "$root_dir/scripts/homebrew/cask_release.py" render \
  --version 0.9.8 \
  --checksums "$tmp_dir/stable-SHA256SUMS" \
  --manifest "$tmp_dir/stable-BUILD-MANIFEST.json" \
  --output "$tmp_dir/stable.rb"
cmp "$root_dir/Casks/televybackup.rb" "$tmp_dir/stable.rb"

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
