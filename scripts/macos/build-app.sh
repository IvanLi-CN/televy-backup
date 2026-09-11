#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

variant="${TELEVYBACKUP_APP_VARIANT:-prod}"
case "$variant" in
  prod)
    bundle_display_name="TelevyBackup"
    bundle_id="com.ivan.televybackup"
    ;;
  dev)
    bundle_display_name="TelevyBackup Dev"
    bundle_id="com.ivan.televybackup.dev"
    # Dev default: avoid prompting for signing identities (ad-hoc signing).
    if [ -z "${TELEVYBACKUP_CODESIGN_IDENTITY:-}" ]; then
      export TELEVYBACKUP_CODESIGN_IDENTITY="-"
    fi
    ;;
  *)
    echo "ERROR: invalid TELEVYBACKUP_APP_VARIANT=$variant (expected: dev|prod)" >&2
    exit 2
    ;;
esac

executable_name="TelevyBackup"
src_dir="$root_dir/macos/TelevyBackupApp"
out_root="$root_dir/target/macos-app"
source_commit="${TELEVYBACKUP_SOURCE_COMMIT:-$(git rev-parse HEAD)}"
build_mode="${TELEVYBACKUP_BUILD_MODE:-development}"
case "$build_mode" in
  development|release) ;;
  *) echo "ERROR: invalid TELEVYBACKUP_BUILD_MODE=$build_mode (expected: development|release)" >&2; exit 2 ;;
esac
if [[ "$variant" == "dev" && "$build_mode" != "development" ]]; then
  echo "ERROR: the dev app must use the development product identity (got $build_mode)" >&2
  exit 2
fi
release_version="$(python3 "$root_dir/scripts/product-version.py" --mode "$build_mode" --source-sha "$source_commit")"
build_number="${TELEVYBACKUP_BUILD_NUMBER:-$(git rev-list --count "$source_commit" 2>/dev/null || printf '0')}"
cargo_target="${TELEVYBACKUP_CARGO_TARGET:-}"
short_version="${release_version%%-*}"
if [[ ! "$short_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "ERROR: release version must contain numeric semver base: $release_version" >&2
  exit 2
fi
export TELEVYBACKUP_BUILD_MODE="$build_mode"
export TELEVYBACKUP_BUILD_COMMIT="$source_commit"
export TELEVYBACKUP_BUILD_NUMBER="$build_number"
app_dir="$out_root/${bundle_display_name}.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
launch_agents_dir="$contents_dir/Library/LaunchAgents"
login_items_dir="$contents_dir/Library/LoginItems"
access_app_dir="$login_items_dir/TelevyBackup Snapshot Access.app"
access_contents_dir="$access_app_dir/Contents"
access_macos_dir="$access_contents_dir/MacOS"
access_agent_plist="$launch_agents_dir/com.ivan.televybackup.snapshot-access.plist"

rm -rf "$app_dir" "$out_root/TelevyBackup Snapshot Access.app"
mkdir -p "$macos_dir"
mkdir -p "$resources_dir"

brand_source_dir="$root_dir/assets/brand"
app_icon_source="$root_dir/macos/TelevyBackupApp/Resources/TelevyBackup.icns"
asset_catalog_source="$brand_source_dir/macos/Assets.xcassets"
bash "$root_dir/scripts/macos/verify-brand-assets.sh" "$brand_source_dir"
[[ -s "$app_icon_source" ]] || {
  echo "ERROR: missing app icon: $app_icon_source" >&2
  exit 1
}
[[ -s "$asset_catalog_source/AppIcon.appiconset/Contents.json" ]] || {
  echo "ERROR: missing AppIcon asset catalog: $asset_catalog_source" >&2
  exit 1
}
for brand_asset in \
  televybackup-logo-ui.svg \
  televybackup-logo-ui-compact.svg \
  televybackup-logo-dark.svg \
  televybackup-logo-dark-compact.svg \
  televybackup-logo-template.svg; do
  [[ -s "$brand_source_dir/$brand_asset" ]] || {
    echo "ERROR: missing brand asset: $brand_source_dir/$brand_asset" >&2
    exit 1
  }
done
mkdir -p "$resources_dir/Brand"
cp "$app_icon_source" "$resources_dir/TelevyBackup.icns"
cp "$brand_source_dir/televybackup-logo-ui.svg" "$resources_dir/Brand/televybackup-logo-ui.svg"
cp "$brand_source_dir/televybackup-logo-ui-compact.svg" "$resources_dir/Brand/televybackup-logo-ui-compact.svg"
cp "$brand_source_dir/televybackup-logo-dark.svg" "$resources_dir/Brand/televybackup-logo-dark.svg"
cp "$brand_source_dir/televybackup-logo-dark-compact.svg" "$resources_dir/Brand/televybackup-logo-dark-compact.svg"
cp "$brand_source_dir/televybackup-logo-template.svg" "$resources_dir/Brand/televybackup-logo-template.svg"

rm -f "$resources_dir/televybackup" "$resources_dir/televybackup-mtproto-helper" \
  "$macos_dir/televybackup-snapshot-helper" \
  "$macos_dir/televybackup-snapshot-mount-helper" 2>/dev/null || true

binary_dir="$root_dir/target/release"
if [[ -n "$cargo_target" ]]; then
  binary_dir="$root_dir/target/$cargo_target/release"
fi

echo "Building CLI ($release_version, $cargo_target)..."
if [[ -n "$cargo_target" ]]; then cargo build -p televybackup --release --target "$cargo_target"; else cargo build -p televybackup --release; fi
cp "$binary_dir/televybackup" "$macos_dir/televybackup-cli"

echo "Building daemon..."
if [[ -n "$cargo_target" ]]; then cargo build -p televybackupd --release --target "$cargo_target"; else cargo build -p televybackupd --release; fi
cp "$binary_dir/televybackupd" "$macos_dir/televybackupd"

echo "Building APFS Snapshot Access..."
if [[ -n "$cargo_target" ]]; then
  if [[ -n "${TELEVYBACKUP_SNAPSHOT_ACCESS_BUNDLE:-}" ]]; then
    cargo build -p televybackup-snapshot-access --bin televybackup-snapshot-mount-helper --release --target "$cargo_target"
  else
    cargo build -p televybackup-snapshot-access --release --target "$cargo_target"
  fi
else
  if [[ -n "${TELEVYBACKUP_SNAPSHOT_ACCESS_BUNDLE:-}" ]]; then
    cargo build -p televybackup-snapshot-access --bin televybackup-snapshot-mount-helper --release
  else
    cargo build -p televybackup-snapshot-access --release
  fi
fi
snapshot_access_binary="$binary_dir/televybackup-snapshot-access"
snapshot_mount_helper_binary="$binary_dir/televybackup-snapshot-mount-helper"
cp "$snapshot_mount_helper_binary" "$macos_dir/televybackup-snapshot-mount-helper"
chmod 755 "$macos_dir/televybackup-snapshot-mount-helper"

echo "Building MTProto helper..."
if [[ -n "$cargo_target" ]]; then cargo build --manifest-path "$root_dir/crates/mtproto-helper/Cargo.toml" --release --target "$cargo_target"; else cargo build --manifest-path "$root_dir/crates/mtproto-helper/Cargo.toml" --release; fi
helper_binary_dir="$root_dir/crates/mtproto-helper/target/release"
if [[ -n "$cargo_target" ]]; then
  helper_binary_dir="$root_dir/crates/mtproto-helper/target/$cargo_target/release"
fi
cp "$helper_binary_dir/televybackup-mtproto-helper" "$macos_dir/televybackup-mtproto-helper"

sdk_path="$(xcrun --sdk macosx --show-sdk-path)"

swiftc_args=(
  -sdk "$sdk_path" \
  -parse-as-library \
  -O \
  -framework SwiftUI \
  -framework AppKit \
  -framework ServiceManagement \
)
if [[ "${TELEVYBACKUP_GUI_LIFECYCLE_TESTING:-0}" == "1" ]]; then
  swiftc_args+=(-D TELEVYBACKUP_GUI_LIFECYCLE_TESTING)
fi

xcrun swiftc "${swiftc_args[@]}" \
  -o "$macos_dir/$executable_name" \
  "$src_dir"/*.swift

cat > "$contents_dir/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>$bundle_display_name</string>
  <key>CFBundleDisplayName</key>
  <string>$bundle_display_name</string>
  <key>CFBundleIdentifier</key>
  <string>$bundle_id</string>
  <key>CFBundleVersion</key>
  <string>$build_number</string>
  <key>CFBundleShortVersionString</key>
  <string>$short_version</string>
  <key>TelevyBackupReleaseVersion</key>
  <string>$release_version</string>
  <key>TelevyBackupSourceCommit</key>
  <string>$source_commit</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleExecutable</key>
  <string>$executable_name</string>
  <key>CFBundleIconFile</key>
  <string>TelevyBackup.icns</string>
  <key>CFBundleIconName</key>
  <string>AppIcon</string>
  <key>LSMinimumSystemVersion</key>
  <string>15.0</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
PLIST

actool_partial_plist="$contents_dir/actool-partial.plist"
echo "Compiling AppIcon asset catalog..."
xcrun actool \
  --compile "$resources_dir" \
  --platform macosx \
  --minimum-deployment-target 15.0 \
  --app-icon AppIcon \
  --output-partial-info-plist "$actool_partial_plist" \
  "$asset_catalog_source"
actool_icon_name="$(/usr/bin/plutil -extract CFBundleIconName raw -o - "$actool_partial_plist")"
[[ "$actool_icon_name" == "AppIcon" ]] || {
  echo "ERROR: actool did not emit CFBundleIconName=AppIcon" >&2
  exit 1
}
/usr/bin/plutil -replace CFBundleIconName -string "$actool_icon_name" "$contents_dir/Info.plist"
rm -f "$actool_partial_plist" "$resources_dir/AppIcon.icns"

codesign_identity="${TELEVYBACKUP_CODESIGN_IDENTITY:--}"

# Snapshot Access owns the independent FDA identity. The RC reuse path is a
# previously verified Universal bundle and must remain byte-for-byte unchanged.
if [[ -n "${TELEVYBACKUP_SNAPSHOT_ACCESS_BUNDLE:-}" ]]; then
  reuse_bundle="${TELEVYBACKUP_SNAPSHOT_ACCESS_BUNDLE}"
  [[ -d "$reuse_bundle" ]] || { echo "missing reusable Snapshot Access bundle: $reuse_bundle" >&2; exit 1; }
  mkdir -p "$login_items_dir"
  cp -R "$reuse_bundle" "$access_app_dir"
  codesign --verify --strict "$access_app_dir"
else
  mkdir -p "$access_macos_dir" "$launch_agents_dir"
  cp "$snapshot_access_binary" "$access_macos_dir/televybackup-snapshot-access"
  chmod 755 "$access_macos_dir/televybackup-snapshot-access"
  cat > "$access_contents_dir/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>TelevyBackup Snapshot Access</string>
  <key>CFBundleDisplayName</key><string>TelevyBackup Snapshot Access</string>
  <key>CFBundleIdentifier</key><string>com.ivan.televybackup.snapshot-access</string>
  <key>CFBundleVersion</key><string>$build_number</string>
  <key>CFBundleShortVersionString</key><string>$short_version</string>
  <key>TelevyBackupReleaseVersion</key><string>$release_version</string>
  <key>TelevyBackupSourceCommit</key><string>$source_commit</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>televybackup-snapshot-access</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
</dict></plist>
PLIST
  codesign --force --sign "$codesign_identity" -i "com.ivan.televybackup.snapshot-access" "$access_app_dir" \
    || echo "WARN: codesign Snapshot Access app failed"
  codesign --verify --strict "$access_app_dir" \
    || echo "WARN: codesign verification failed for Snapshot Access app"
fi

mkdir -p "$launch_agents_dir"
cat > "$access_agent_plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.ivan.televybackup.snapshot-access</string>
  <key>BundleProgram</key><string>Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access</string>
  <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>
PLIST

if [[ -n "$codesign_identity" ]]; then
  echo "Codesigning main app with controlled identity: $codesign_identity"
  codesign --force --sign "$codesign_identity" -i "$bundle_id.cli" "$macos_dir/televybackup-cli" \
    || echo "WARN: codesign CLI failed"
  codesign --force --sign "$codesign_identity" -i "$bundle_id.mtproto-helper" "$macos_dir/televybackup-mtproto-helper" \
    || echo "WARN: codesign helper failed"
  codesign --force --sign "$codesign_identity" -i "$bundle_id.snapshot-mount-helper" "$macos_dir/televybackup-snapshot-mount-helper" \
    || echo "WARN: codesign snapshot mount helper failed"
  codesign --force --sign "$codesign_identity" "$app_dir" \
    || echo "WARN: codesign app failed"
else
  echo "No codesign identity found; applying ad-hoc signature for local runs"
  codesign --force --sign - -i "$bundle_id.snapshot-mount-helper" "$macos_dir/televybackup-snapshot-mount-helper" \
    || echo "WARN: ad-hoc codesign snapshot mount helper failed"
  codesign --force --sign - "$app_dir" \
    || echo "WARN: ad-hoc codesign app failed"
fi

codesign -vvv --deep --strict "$app_dir" >/dev/null 2>&1 \
  || echo "WARN: codesign verification failed (embedded CLI may be killed by macOS)"

echo "Built ($variant): $app_dir"
echo "Embedded Snapshot Access: $access_app_dir"
