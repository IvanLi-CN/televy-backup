#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

bash -n \
  "$root_dir/scripts/macos/build-app.sh" \
  "$root_dir/scripts/macos/package-release.sh" \
  "$root_dir/scripts/macos/assemble-universal.sh" \
  "$root_dir/scripts/macos/build-dmg.sh" \
  "$root_dir/scripts/macos/extract-snapshot-access-helper.sh" \
  "$root_dir/scripts/macos/generate-release-manifest.sh" \
  "$root_dir/scripts/macos/finder-dmg-acceptance.sh" \
  "$root_dir/scripts/macos/verify-release-assets.sh" \
  "$root_dir/scripts/macos/verify-dmg-layout.sh" \
  "$root_dir/scripts/macos/verify-component-identity.sh" \
  "$root_dir/scripts/macos/verify-webdav-snapshot-browsing.sh" \
  "$root_dir/scripts/macos/generate-brand-variants.sh" \
  "$root_dir/scripts/macos/verify-brand-assets.sh" \
  "$root_dir/scripts/macos/generate-app-icon-assets.sh" \
  "$root_dir/scripts/macos/generate-app-icon-previews.sh" \
  "$root_dir/scripts/macos/verify-app-icon-assets.sh"
python3 -m py_compile \
  "$root_dir/scripts/macos/normalize-designated-requirement.py" \
  "$root_dir/scripts/macos/verify-dmg-metadata.py"
normalized_requirement="$(printf '%s\n' \
  'codesign: warning: blah' \
  'designated => identifier "com.example.helper" and (cdhash H"2222222222222222222222222222222222222222" or' \
  '  cdhash H"1111111111111111111111111111111111111111")' \
  | python3 "$root_dir/scripts/macos/normalize-designated-requirement.py")"
[[ "$normalized_requirement" == 'designated => identifier "com.example.helper" and (cdhash H"1111111111111111111111111111111111111111" or cdhash H"2222222222222222222222222222222222222222")' ]] || {
  echo "designated requirement normalizer must reconstruct wrapped codesign output" >&2
  exit 1
}

build_text="$(<"$root_dir/scripts/macos/build-app.sh")"
verify_brand_text="$(<"$root_dir/scripts/macos/verify-brand-assets.sh")"
[[ "$build_text" == *'verify-brand-assets.sh'* && "$verify_brand_text" == *'shared geometry'* ]] || {
  echo "brand asset verification is not wired into the build contract" >&2
  exit 1
}
[[ "$build_text" == *'CFBundleIconFile'* && "$build_text" == *'CFBundleIconName'* && "$build_text" == *'Assets.xcassets'* ]] || {
  echo "AppIcon bundle contract is not wired into build-app.sh" >&2
  exit 1
}
[[ "$build_text" == *'LSUIElement'* && "$build_text" == *'<true/>'* ]] || {
  echo "LSUIElement menu-bar agent contract is missing from build-app.sh" >&2
  exit 1
}
[[ "$build_text" == *'TelevyBackupReleaseVersion'* && "$build_text" == *'TelevyBackupSourceCommit'* ]] || {
  echo "Snapshot Access bundle is missing the full product identity metadata" >&2
  exit 1
}
grep -F 'mkdir -p "$out_root"' <<<"$build_text" >/dev/null || {
  echo "build-app.sh must initialize the output root before validating a reusable bundle" >&2
  exit 1
}
[[ "$build_text" == *'bundle_id.daemon'* && "$build_text" == *'televybackupd'* ]] || {
  echo "The daemon must be signed before the outer app bundle" >&2
  exit 1
}
for binary in TelevyBackup televybackup-cli televybackupd televybackup-mtproto-helper televybackup-snapshot-mount-helper; do
  grep -F "\"\$macos_dir/$binary\"" <<<"$build_text" >/dev/null || {
    echo "build-app.sh must preserve executable modes for every main binary" >&2
    exit 1
  }
done
grep -F 'workspace_build+=(-p televybackup -p televybackupd)' <<<"$build_text" >/dev/null || {
  echo "build-app.sh must build CLI and daemon in one Cargo invocation" >&2
  exit 1
}
grep -F 'if [[ -z "${TELEVYBACKUP_SNAPSHOT_ACCESS_BUNDLE:-}" ]]' <<<"$build_text" >/dev/null || {
  echo "build-app.sh must not rebuild the reusable Snapshot Access bundle" >&2
  exit 1
}
grep -F 'snapshot_build=(cargo build --locked --release -p televybackup-snapshot-access --bin televybackup-snapshot-mount-helper)' <<<"$build_text" >/dev/null || {
  echo "build-app.sh must build the main-app Snapshot mount helper on the reuse path" >&2
  exit 1
}
if grep -F 'workspace_build+=(--bin' <<<"$build_text" >/dev/null; then
  echo "build-app.sh must not filter the shared workspace build to one binary" >&2
  exit 1
fi
grep -F "chmod 755 \\" <<<"$build_text" >/dev/null || {
  echo "build-app.sh must preserve executable modes for every main binary" >&2
  exit 1
}
icon_text="$(<"$root_dir/scripts/macos/generate-app-icon-assets.sh")"
[[ "$icon_text" == *'icon_512x512@2x.png:1024'* && "$icon_text" == *'iconutil -c icns'* && "$icon_text" == *'AppIcon-dark-'* ]] || {
  echo "AppIcon generation contract is incomplete" >&2
  exit 1
}
verify_icon_text="$(<"$root_dir/scripts/macos/verify-app-icon-assets.sh")"
[[ "$verify_icon_text" == *'icon_16x16.png:16'* && "$verify_icon_text" == *'iconutil -c iconset'* && "$verify_icon_text" == *'Assets.car'* ]] || {
  echo "AppIcon verification contract is incomplete" >&2
  exit 1
}
verify_release_text="$(<"$root_dir/scripts/macos/verify-release-assets.sh")"
[[ "$verify_release_text" == *'0$access_mode & 022'* ]] || {
  echo "Snapshot Access mode check must parse stat output as octal" >&2
  exit 1
}
grep -F 'checksum_records' <<<"$verify_release_text" >/dev/null || {
  echo "release asset verification must bind SHA256SUMS to the manifest asset set" >&2
  exit 1
}
grep -F 'hdiutil attach -plist' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must consume machine-readable attach output" >&2
  exit 1
}
grep -F 'diskutil verifyVolume "$attached_device"' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must verify the attached filesystem" >&2
  exit 1
}
grep -F 'detach_dmg_exact' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must detach the exact plist-resolved device" >&2
  exit 1
}
grep -F 'hdiutil imageinfo -plist' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must inspect the actual image format" >&2
  exit 1
}
grep -F 'verify-dmg-metadata.py' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must inspect attached filesystem metadata" >&2
  exit 1
}
grep -F 'verify_nested_helper_path' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must reject symlinked or escaping nested helpers" >&2
  exit 1
}
python3 - "$root_dir/scripts/macos/verify-dmg-metadata.py" "$tmp_dir" <<'PY'
import plistlib
import subprocess
import sys
from pathlib import Path

verifier = Path(sys.argv[1])
root = Path(sys.argv[2])

def run_case(image_format, filesystem_type, expected):
    image_info = root / f"image-{image_format}.plist"
    filesystem_info = root / f"filesystem-{filesystem_type}.plist"
    with image_info.open("wb") as handle:
        plistlib.dump({"Format": image_format}, handle)
    with filesystem_info.open("wb") as handle:
        plistlib.dump({"FilesystemType": filesystem_type}, handle)
    result = subprocess.run(
        [
            sys.executable,
            str(verifier),
            "--image-info",
            str(image_info),
            "--filesystem-info",
            str(filesystem_info),
        ],
        capture_output=True,
        text=True,
    )
    if (result.returncode == 0) != expected:
        raise SystemExit(
            f"unexpected DMG metadata result for {image_format}/{filesystem_type}: "
            f"{result.stdout}{result.stderr}"
        )

run_case("UDZO", "hfs", True)
run_case("UDRO", "hfs", False)
run_case("UDZO", "apfs", False)
run_case("UDZO", "not-hfs", False)
PY
identity_text="$(<"$root_dir/scripts/macos/verify-component-identity.sh")"
grep -F 'reference_artifact_sha="$(artifact_sha "$reference" canonical)"' <<<"$identity_text" >/dev/null || {
  echo "component identity verification must compare canonical reference and candidate bundle digests" >&2
  exit 1
}
grep -F 'reference_legacy_artifact_sha="$(artifact_sha "$reference" raw)"' <<<"$identity_text" >/dev/null || {
  echo "component identity verification must retain legacy manifest compatibility" >&2
  exit 1
}
grep -F 'component["artifact_sha256"] in {sys.argv[3], sys.argv[4]}' <<<"$identity_text" >/dev/null || {
  echo "component identity verification must bind canonical or legacy bundle digest to the source manifest" >&2
  exit 1
}
if grep -E '(^|[[:space:]])assert[[:space:]]' <<<"$identity_text" >/dev/null; then
  echo "component identity verification must not use optimizable Python assertions" >&2
  exit 1
fi
[[ "$verify_release_text" == *'one-time-bootstrap-universal-build'* ]] || {
  echo "release asset verifier must recognize the explicit helper bootstrap source" >&2
  exit 1
}
grep -F '[[ -x "$app/Contents/MacOS/$binary" ]]' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must reject non-executable main binaries" >&2
  exit 1
}
main_binary_mode_checks="$(grep -Fc '[[ -x "$app/Contents/MacOS/$binary" ]]' <<<"$verify_release_text")"
[[ "$main_binary_mode_checks" -ge 2 ]] || {
  echo "app and DMG verification must reject non-executable main binaries" >&2
  exit 1
}
grep -F '[[ -x "$tools_dir/TelevyBackup Tools/bin/$binary" ]]' <<<"$verify_release_text" >/dev/null || {
  echo "tools archive verification must reject non-executable binaries" >&2
  exit 1
}
release_bundle_id_checks="$(grep -Fc 'com.ivan.televybackup" ]]' <<<"$verify_release_text")"
[[ "$release_bundle_id_checks" -ge 2 ]] || {
  echo "app and DMG verification must reject non-prod bundle ids" >&2
  exit 1
}
grep -F "trap 'rm -rf \"\$tools_dir\"' EXIT" <<<"$verify_release_text" >/dev/null || {
  echo "tools archive verification must clean extracted temporary files on failure" >&2
  exit 1
}
grep -F '  (' <<<"$verify_release_text" >/dev/null || {
  echo "tools archive verification must isolate cleanup for each archive" >&2
  exit 1
}
webdav_text="$(<"$root_dir/scripts/macos/verify-webdav-snapshot-browsing.sh")"
[[ "$webdav_text" == *'cargo test --manifest-path "$root_dir/Cargo.toml" -p televybackupd webdav_service -- --list'* ]]
[[ "$webdav_text" != *'http.server'* ]]
[[ "$webdav_text" == *'TELEVYBACKUP_RUN_WEBDAV_MOUNT_ACCEPTANCE'* ]]
[[ "$webdav_text" == *'cargo test --manifest-path "$root_dir/Cargo.toml" -p televybackupd snapshot_browse::tests -- --list'* ]]
[[ "$webdav_text" == *'--exact --ignored --nocapture'* ]]

package_text="$(<"$root_dir/scripts/macos/package-release.sh")"
[[ "$package_text" == *'--mode release|development'* ]]
[[ "$package_text" == *'product-version.py'* ]]
[[ "$package_text" == *'app_dest="$output_dir/TelevyBackup.app"'* ]]
[[ "$package_text" != *'access_dest="$output_dir/TelevyBackup Snapshot Access.app"'* ]]
[[ "$package_text" != *'REPLACE_WITH_SNAPSHOT_ACCESS_APP'* ]]
[[ "$package_text" != *'--version'* ]]
grep -F 'verify-dmg-layout.sh' <<<"$package_text" >/dev/null || {
  echo "native package creation must run the shared DMG layout verifier" >&2
  exit 1
}
build_dmg_text="$(<"$root_dir/scripts/macos/build-dmg.sh")"
[[ "$build_dmg_text" == *'dmgbuild-requirements.txt'* && "$build_dmg_text" == *'dmgbuild.__version__'* && "$build_dmg_text" == *'1.6.7'* ]] || {
  echo "DMG builder must use the pinned dmgbuild dependency and version check" >&2
  exit 1
}
[[ "$build_dmg_text" == *'generated-overlay.png'* && "$build_dmg_text" == *'overlay_asset'* && "$build_dmg_text" == *'expected_overlay_digest'* && "$build_dmg_text" == *'generated-background-composed.png'* && "$build_dmg_text" == *'composed_background_asset'* && "$build_dmg_text" == *'expected_background_digest'* ]] || {
  echo "DMG builder must validate the checked-in schema-driven bitmap digests" >&2
  exit 1
}
settings_text="$(<"$root_dir/scripts/macos/dmgbuild-settings.py")"
[[ "$settings_text" == *'layout.json'* && "$settings_text" == *'background'* && "$settings_text" == *'icon_locations'* ]] || {
  echo "dmgbuild settings must consume the shared layout schema" >&2
  exit 1
}
TELEVYBACKUP_ROOT_DIR="$root_dir" \
TELEVYBACKUP_DMG_SOURCE_DIR="$root_dir" \
TELEVYBACKUP_DMG_VOLUME_NAME=fixture \
TELEVYBACKUP_DMG_OUTPUT="$tmp_dir/fixture.dmg" \
  python3 - "$root_dir/scripts/macos/dmgbuild-settings.py" <<'PY'
import runpy
import sys

settings = runpy.run_path(sys.argv[1])
assert settings["icon_size"] == 128
assert settings["icon_locations"]["TelevyBackup.app"] == (210, 270)
assert settings["icon_locations"]["Applications"] == (550, 270)
PY
overlay_text="$(<"$root_dir/scripts/macos/generate-dmg-overlay.swift")"
[[ "$overlay_text" == *'--layout LAYOUT'* && "$overlay_text" == *'layout.overlay.instruction'* && "$overlay_text" == *'layout.overlay.arrowStart'* ]] || {
  echo "DMG overlay generation must consume the checked-in layout schema" >&2
  exit 1
}
assemble_text="$(<"$root_dir/scripts/macos/assemble-universal.sh")"
grep -F 'chmod 755 "$universal_app/Contents/MacOS/"*' <<<"$assemble_text" >/dev/null || {
  echo "Universal main binaries must remain executable after lipo" >&2
  exit 1
}
grep -F 'chmod 755 "$universal_access_binary"' <<<"$assemble_text" >/dev/null || {
  echo "Universal Snapshot Access binary must remain executable after lipo" >&2
  exit 1
}
grep -F 'chmod 755 "$native_app/Contents/MacOS/$binary"' <<<"$assemble_text" >/dev/null || {
  echo "Native DMG repackage must preserve executable modes for every main binary" >&2
  exit 1
}
grep -F '[[ -x "$native_access_app/Contents/MacOS/televybackup-snapshot-access" ]]' <<<"$assemble_text" >/dev/null || {
  echo "Native DMG repackage must preserve the nested helper executable mode" >&2
  exit 1
}
grep -F '[[ -x "$universal_access_binary" ]]' <<<"$assemble_text" >/dev/null || {
  echo "Universal assembly must preserve the nested helper executable mode" >&2
  exit 1
}
grep -F 'chmod 755 "$arm_access_binary" "$x86_access_binary" "$universal_access_binary"' <<<"$assemble_text" >/dev/null || {
  echo "Universal assembly must restore artifact-normalized helper modes" >&2
  exit 1
}
grep -F 'access_relative_path="Contents/Library/LoginItems/TelevyBackup Snapshot Access.app"' <<<"$assemble_text" >/dev/null || {
  echo "Universal assembly must keep Snapshot Access nested in the main app" >&2
  exit 1
}
grep -F 'repackage_native_app "$arm_app" arm64' <<<"$assemble_text" >/dev/null || {
  echo "Native arm64 DMG must reuse the Universal Snapshot Access identity" >&2
  exit 1
}
grep -F 'verify-dmg-layout.sh' <<<"$assemble_text" >/dev/null || {
  echo "Universal assembly must run the shared DMG layout verifier" >&2
  exit 1
}
finder_text="$(<"$root_dir/scripts/macos/finder-dmg-acceptance.sh")"
extract_helper_text="$(<"$root_dir/scripts/macos/extract-snapshot-access-helper.sh")"
grep -F 'hdiutil attach -plist' <<<"$extract_helper_text" >/dev/null || {
  echo "Snapshot Access extraction must use machine-readable attach output" >&2
  exit 1
}
grep -F 'device_from_plist' <<<"$extract_helper_text" >/dev/null || {
  echo "Snapshot Access extraction must resolve the exact attached device" >&2
  exit 1
}
grep -F 'trap cleanup EXIT' <<<"$extract_helper_text" >/dev/null || {
  echo "Snapshot Access extraction must clean up mounts on every exit path" >&2
  exit 1
}
grep -F 'helper_real' <<<"$extract_helper_text" >/dev/null || {
  echo "Snapshot Access extraction must enforce nested helper containment" >&2
  exit 1
}
grep -F 'gh release upload "$rc2_tag" "$finder_screenshot"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must upload newly captured screenshots to RC2" >&2
  exit 1
}
for attach_text in "$verify_release_text" "$finder_text"; do
  grep -F 'attach_status=0' <<<"$attach_text" >/dev/null || {
    echo "DMG attach paths must preserve cleanup when hdiutil attach fails" >&2
    exit 1
  }
done
layout_verify_text="$(<"$root_dir/scripts/macos/verify-dmg-layout.sh")"
grep -F 'attach_completed=false' <<<"$layout_verify_text" >/dev/null || {
  echo "DMG layout verification must track attach completion separately from cleanup" >&2
  exit 1
}
grep -F 'if [[ "$mounted" == true || -n "$attached_device" ]]' <<<"$layout_verify_text" >/dev/null || {
  echo "DMG layout cleanup must only resolve devices while a mount may remain" >&2
  exit 1
}
grep -F 'if hdiutil detach "$attached_device"; then' <<<"$layout_verify_text" >/dev/null || {
  echo "DMG layout verification must update mount state only after detach succeeds" >&2
  exit 1
}
python3 - "$root_dir/scripts/macos/verify-dmg-layout.sh" <<'PY'
import pathlib
import sys

if 'mounted=false\nhdiutil detach "$attached_device"' in pathlib.Path(sys.argv[1]).read_text():
    raise SystemExit("DMG layout verification must not clear mount state before detach")
PY
for cleanup_text in "$verify_release_text" "$finder_text"; do
  grep -F 'if [[ "$mounted" == true' <<<"$cleanup_text" >/dev/null &&
    grep -F -- '-n "$attached_device"' <<<"$cleanup_text" >/dev/null || {
    echo "DMG cleanup must only resolve devices while a mount may remain" >&2
    exit 1
  }
done
python3 - "$root_dir/scripts/macos/verify-release-assets.sh" <<'PY'
import pathlib
import sys

if 'mounted=false\n  detach_dmg_exact' in pathlib.Path(sys.argv[1]).read_text():
    raise SystemExit("release verification must not clear mount state before detach")
PY
grep -F 'TELEVYBACKUP_RUN_FINDER_ACCEPTANCE' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must require explicit controlled-session opt-in" >&2
  exit 1
}
grep -F 'screencapture -x -l "$window_id"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must capture only the verified Finder window" >&2
  exit 1
}
grep -F 'defaults write com.apple.finder AppleShowAllFiles' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must restore the AppleShowAllFiles preference" >&2
  exit 1
}
grep -F 'defaults read-type com.apple.finder AppleShowAllFiles' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must preserve the AppleShowAllFiles preference type" >&2
  exit 1
}
grep -F 'rmdir "$mount_point"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must cleanly reject unsupported preference types" >&2
  exit 1
}
grep -F 'prepare_evidence_path()' <<<"$finder_text" >/dev/null &&
grep -F 'prepare_evidence_path "$finder_json"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must safely discard stale observation evidence" >&2
  exit 1
}
grep -F 'os.path.islink(store_path)' <<<"$layout_verify_text" >/dev/null &&
  grep -F 'os.path.islink(store_path)' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verifiers must reject symlinked .DS_Store resources" >&2
  exit 1
}
grep -F 'prepare_evidence_path "$DMG_EVIDENCE_FILE"' <<<"$layout_verify_text" >/dev/null &&
grep -F 'prepare_evidence_path "$DMG_EVIDENCE_FILE"' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verifiers must safely initialize evidence paths" >&2
  exit 1
}
grep -F 'json.load(open(sys.argv[12]' <<<"$finder_text" >/dev/null &&
  grep -F 'json.load(open(sys.argv[13]' <<<"$finder_text" >/dev/null &&
  grep -F 'json.loads(sys.argv[14])' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must preserve Python evidence argument mapping" >&2
  exit 1
}
grep -F 'source_checksums_path="$source_asset_dir/SHA256SUMS"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must bind evidence to adjacent SHA256SUMS" >&2
  exit 1
}
grep -F '/usr/bin/lockf -s -t 0 9' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must serialize access to the Finder session and evidence directory" >&2
  exit 1
}
grep -F 'snapshot_dir="$(mktemp -d' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must use an immutable DMG input snapshot" >&2
  exit 1
}
grep -F 'verify-dmg-layout.sh" --dmg "$dmg"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must verify the exact DMG before GUI evidence" >&2
  exit 1
}
grep -F '"dmg_sha256": sys.argv[4]' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance evidence must record the DMG digest" >&2
  exit 1
}
grep -F '"semantic_layout_digest": semantic_layout_digest' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance evidence must record the semantic layout digest" >&2
  exit 1
}
grep -F 'TELEVYBACKUP_FINDER_VISUAL_REVIEW' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must require explicit scoped visual review" >&2
  exit 1
}
grep -F 'method": "scoped-human-review"' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must record the scoped visual review method" >&2
  exit 1
}
grep -F 'get("window_id")' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must capture the observed Finder window id" >&2
  exit 1
}
grep -F 'hdiutil attach -plist' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must use machine-readable attach output" >&2
  exit 1
}
grep -F 'expected_app = tuple(layout["icon_locations"]["TelevyBackup.app"])' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must compare the app position with the layout schema" >&2
  exit 1
}
grep -F 'expected_applications = tuple(layout["icon_locations"]["Applications"])' <<<"$finder_text" >/dev/null || {
  echo "Finder acceptance must compare the Applications position with the layout schema" >&2
  exit 1
}
grep -F 'expected_background = layout["asset_digests"][layout["composed_background"]]' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must compare the mounted background digest with the layout resource" >&2
  exit 1
}
grep -F 'if entity.get("mount-point") == expected_mount' <<<"$verify_release_text" >/dev/null || {
  echo "DMG attach verification must resolve the exact mounted device" >&2
  exit 1
}
if grep -F 'fallback = ""' <<<"$verify_release_text" >/dev/null; then
  echo "DMG attach verification must not retain an unrelated fallback device" >&2
  exit 1
fi
grep -F -- '--expected-source-commit' <<<"$verify_release_text" >/dev/null || {
  echo "release asset verification must bind the manifest source commit" >&2
  exit 1
}
grep -F 'tarfile' <<<"$verify_release_text" >/dev/null || {
  echo "tools archive verification must reject unsafe member paths before extraction" >&2
  exit 1
}
grep -F 'member.isfifo()' <<<"$verify_release_text" >/dev/null || {
  echo "tools archive verification must reject FIFO members before extraction" >&2
  exit 1
}
grep -F 'os.path.islink(background_path)' <<<"$verify_release_text" >/dev/null || {
  echo "DMG verification must reject symlinked background resources" >&2
  exit 1
}
grep -F 'read-ds-store-layout.py' <<<"$verify_release_text" >/dev/null || {
  echo "release DMG verification must read back Finder geometry from .DS_Store" >&2
  exit 1
}
grep -F 'read-ds-store-layout.py' <<<"$layout_verify_text" >/dev/null || {
  echo "DMG layout verification must read back Finder geometry from .DS_Store" >&2
  exit 1
}
grep -F 'DMG_EVIDENCE_FILE' <<<"$verify_release_text" >/dev/null || {
  echo "release DMG verification must support persisted machine-readable evidence" >&2
  exit 1
}
grep -F 'DMG_EVIDENCE_FILE' <<<"$layout_verify_text" >/dev/null || {
  echo "DMG layout verification must support persisted machine-readable evidence" >&2
  exit 1
}
for event_name in dmg_verify dmg_attach dmg_filesystem_verify dmg_detach; do
  grep -F "emit_dmg_event $event_name" <<<"$layout_verify_text" >/dev/null || {
    echo "DMG layout verification must persist $event_name evidence" >&2
    exit 1
  }
  grep -F "emit_dmg_event $event_name" <<<"$verify_release_text" >/dev/null || {
    echo "release DMG verification must persist $event_name evidence" >&2
    exit 1
  }
done
package_workflow_text="$(<"$root_dir/.github/workflows/package-ci.yml")"
[[ "$(grep -Fc 'verify-dmg-layout.sh' <<<"$package_workflow_text")" -ge 2 ]] || {
  echo "native package CI jobs must expose the shared DMG layout verifier" >&2
  exit 1
}
[[ "$(grep -Fc 'timeout-minutes: 15' <<<"$package_workflow_text")" -eq 2 ]] || {
  echo "native package CI jobs must have a 15-minute timeout" >&2
  exit 1
}
[[ "$(grep -Fc 'uses: actions/cache@' <<<"$package_workflow_text")" -ge 2 ]] || {
  echo "native package CI jobs must cache build dependencies" >&2
  exit 1
}
[[ "$(grep -Fc 'uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830' <<<"$package_workflow_text")" -eq 2 ]] || {
  echo "package dependency cache action must be pinned to a full commit SHA" >&2
  exit 1
}
grep -F 'runner.arch' <<<"$package_workflow_text" >/dev/null || {
  echo "package dependency cache must be architecture-specific" >&2
  exit 1
}
[[ "$(grep -Fc 'if-no-files-found: error' <<<"$package_workflow_text")" -ge 3 ]] || {
  echo "package matrix must upload persisted DMG verification evidence" >&2
  exit 1
}
[[ "$(grep -Fc '${{ runner.os }}-ARM64-cargo-macos-' <<<"$package_workflow_text")" -eq 0 && "$(grep -Fc '${{ runner.os }}-X64-cargo-macos-' <<<"$package_workflow_text")" -eq 0 ]] || {
  echo "package dependency cache must not restore host-specific artifacts across architectures" >&2
  exit 1
}
[[ "$(grep -Fc 'timeout-minutes: 10' <<<"$package_workflow_text")" -eq 1 ]] || {
  echo "Universal package CI must have a 10-minute timeout" >&2
  exit 1
}
[[ "$(grep -Fc 'Build arm64 package (up to 2 attempts)' <<<"$package_workflow_text")" -eq 1 && "$(grep -Fc 'Build x86_64 package (up to 2 attempts)' <<<"$package_workflow_text")" -eq 1 ]] || {
  echo "native package CI must retry each build at most once" >&2
  exit 1
}
grep -F 'source_sha: ${{ steps.classify.outputs.source_sha }}' <<<"$package_workflow_text" >/dev/null || {
  echo "package classification must expose an immutable source SHA" >&2
  exit 1
}
grep -F -- '--expected-source-commit "$(git rev-parse HEAD)"' <<<"$package_workflow_text" >/dev/null || {
  echo "package verification must bind the manifest source commit" >&2
  exit 1
}
grep -F -- '--expected-packaging-commit "$GITHUB_SHA"' <<<"$package_workflow_text" >/dev/null || {
  echo "package verification must bind the manifest packaging commit" >&2
  exit 1
}
grep -F 'echo "source_sha=$(git rev-parse HEAD)"' <<<"$package_workflow_text" >/dev/null || {
  echo "package classification must record the checked-out source SHA" >&2
  exit 1
}
[[ "$(grep -Fc 'ref: ${{ needs.classify.outputs.source_sha }}' <<<"$package_workflow_text")" -eq 3 ]] || {
  echo "all package jobs must checkout the classified immutable source SHA" >&2
  exit 1
}
grep -F 'repackage_native_app "$x86_app" x86_64' <<<"$assemble_text" >/dev/null || {
  echo "Native x86_64 DMG must reuse the Universal Snapshot Access identity" >&2
  exit 1
}

version="$(tr -d '\n' < "$root_dir/VERSION")"
for asset in \
  "TelevyBackup-${version}.dmg" \
  "TelevyBackup-${version}-arm64.dmg" \
  "TelevyBackup-${version}-x86_64.dmg" \
  "televybackup-tools-${version}-arm64.tar.gz" \
  "televybackup-tools-${version}-x86_64.tar.gz"; do
  printf 'fixture %s\n' "$asset" > "$tmp_dir/$asset"
done

mkdir -p "$tmp_dir/scripts"
cp "$root_dir/scripts/product-version.py" "$tmp_dir/scripts/product-version.py"
git -C "$tmp_dir" init -q
git -C "$tmp_dir" config user.name test
git -C "$tmp_dir" config user.email test@example.com
printf '%s\n' "$version" > "$tmp_dir/VERSION"
git -C "$tmp_dir" add VERSION scripts
git -C "$tmp_dir" commit -qm fixture
fixture_source_commit="$(git -C "$tmp_dir" rev-parse HEAD)"
fixture_packaging_commit="$(git -C "$root_dir" rev-parse HEAD)"

TELEVYBACKUP_SNAPSHOT_ACCESS_SOURCE=one-time-bootstrap-universal-build \
  bash "$root_dir/scripts/macos/generate-release-manifest.sh" \
  --mode release \
  --asset-dir "$tmp_dir" \
  --source-commit "$(git -C "$tmp_dir" rev-parse HEAD)" \
  --packaging-commit "$fixture_packaging_commit" \
  --output "$tmp_dir/BUILD-MANIFEST.json"
# This contract fixture runs on Linux and deliberately verifies metadata only;
# macOS package/release jobs run the default bundle checks.
bash "$root_dir/scripts/macos/verify-release-assets.sh" --mode release --asset-dir "$tmp_dir" --expected-source-commit "$fixture_source_commit" --expected-packaging-commit "$fixture_packaging_commit" --skip-bundle-checks

python3 - "$tmp_dir/BUILD-MANIFEST.json" "$version" <<'PY'
import json
import sys

payload = json.load(open(sys.argv[1], encoding="utf-8"))
assert payload["release_version"] == sys.argv[2]
assert payload["signing"] == "ad-hoc"
assert len(payload["assets"]) == 5
assert payload["components"]["snapshot_access"]["source"] == "one-time-bootstrap-universal-build"
assert payload["components"]["snapshot_mount_helper"]["compatible_component_versions"] == ["0.1.0", "0.9.8"]
layout = payload["dmg_layout"]
assert layout["builder"] == {"name": "dmgbuild", "version": "1.6.7"}
assert layout["format"] == "UDZO"
assert layout["window"] == {"height": 520, "origin": [100, 100], "width": 760}
assert layout["icon_locations"]["TelevyBackup.app"] == [210, 270]
assert layout["icon_locations"]["Applications"] == [550, 270]
assert layout["hidden_resource_allowlist"] == [".DS_Store", ".background"]
assert len(layout["semantic_layout_digest"]) == 64
assert all("dmg_layout_digest" in asset for asset in payload["assets"] if asset["name"].endswith(".dmg"))
PY

cp "$tmp_dir/BUILD-MANIFEST.json" "$tmp_dir/BUILD-MANIFEST.original.json"
python3 - "$tmp_dir/BUILD-MANIFEST.json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    payload = json.load(handle)
payload["source_commit"] = "0" * 40
with open(path, "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY
if bash "$root_dir/scripts/macos/verify-release-assets.sh" \
  --mode release --asset-dir "$tmp_dir" --expected-source-commit "$fixture_source_commit" --expected-packaging-commit "$fixture_packaging_commit" --skip-bundle-checks >/dev/null 2>&1; then
  echo "release asset verifier accepted a mismatched manifest source commit" >&2
  exit 1
fi
cp "$tmp_dir/BUILD-MANIFEST.original.json" "$tmp_dir/BUILD-MANIFEST.json"

python3 - "$tmp_dir/BUILD-MANIFEST.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
payload["dmg_layout"]["semantic_layout_digest"] = "0" * 64
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY
if bash "$root_dir/scripts/macos/verify-release-assets.sh" \
  --mode release --asset-dir "$tmp_dir" --expected-source-commit "$fixture_source_commit" --expected-packaging-commit "$fixture_packaging_commit" --skip-bundle-checks >/dev/null 2>&1; then
  echo "release asset verifier accepted a mismatched DMG layout digest" >&2
  exit 1
fi

python3 - "$tmp_dir/BUILD-MANIFEST.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    payload = json.load(handle)
payload["release_version"] = "0.0.0"
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    json.dump(payload, handle)
PY
if PYTHONOPTIMIZE=1 bash "$root_dir/scripts/macos/verify-release-assets.sh" \
  --mode release --asset-dir "$tmp_dir" --expected-source-commit "$fixture_source_commit" --expected-packaging-commit "$fixture_packaging_commit" --skip-bundle-checks >/dev/null 2>&1; then
  echo "release asset verifier accepted a mismatched manifest under optimized Python" >&2
  exit 1
fi

printf 'not-a-dmg' > "$tmp_dir/not-a-dmg"
mkdir -p "$tmp_dir/finder-evidence"
if TELEVYBACKUP_RUN_FINDER_ACCEPTANCE=0 bash "$root_dir/scripts/macos/finder-dmg-acceptance.sh" \
  --dmg "$tmp_dir/not-a-dmg" --evidence-dir "$tmp_dir/finder-evidence" >/dev/null 2>&1; then
  echo "Finder acceptance must refuse an uncontrolled session" >&2
  exit 1
fi

python3 - "$root_dir/packaging/macos/snapshot-components.lock.json" <<'PY'
import json
import sys

lock = json.load(open(sys.argv[1], encoding="utf-8"))
access = lock["components"]["snapshot_access"]
assert lock["signing"] == "ad-hoc"
assert lock["bootstrap_release_tag"] == "v0.9.8-rc.1"
assert access["bundle_id"] == "com.ivan.televybackup.snapshot-access"
assert access["component_version"] == "0.2.0"
assert access["protocol_version"] == 2
assert access["reuse_policy"] == "byte-identical-no-rebuild-no-lipo-no-resign"
identity = access["identity"]
assert identity["sha256"].startswith("BUILD-MANIFEST.json#/")
assert identity["artifact_sha256"].startswith("BUILD-MANIFEST.json#/")
assert identity["cdhash"].startswith("BUILD-MANIFEST.json#/")
assert identity["designated_requirement"].startswith("BUILD-MANIFEST.json#/")
mount_identity = lock["components"]["snapshot_mount_helper"]["identity"]
assert mount_identity["sha256"].startswith("BUILD-MANIFEST.json#/")
assert mount_identity["artifact_sha256"].startswith("BUILD-MANIFEST.json#/")
assert mount_identity["cdhash"].startswith("BUILD-MANIFEST.json#/")
assert mount_identity["designated_requirement"].startswith("BUILD-MANIFEST.json#/")
mount = lock["components"]["snapshot_mount_helper"]
assert mount["identity_source"] == "bundled-release-artifact"
assert mount["installed_observation"] == "manual-rc-acceptance-required"
assert mount["update_policy"] == "compatibility-check-only"
PY

echo "package script contract tests passed"
