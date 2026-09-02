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
release_version="${TELEVYBACKUP_RELEASE_VERSION:-0.1.0}"
source_commit="${TELEVYBACKUP_SOURCE_COMMIT:-$(git rev-parse HEAD)}"
build_number="${TELEVYBACKUP_BUILD_NUMBER:-$(git rev-list --count "$source_commit" 2>/dev/null || printf '0')}"
cargo_target="${TELEVYBACKUP_CARGO_TARGET:-}"
short_version="${release_version%%-*}"
if [[ ! "$short_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "ERROR: release version must contain numeric semver base: $release_version" >&2
  exit 2
fi
export TELEVYBACKUP_BUILD_VERSION="$release_version"
export TELEVYBACKUP_BUILD_COMMIT="$source_commit"
export TELEVYBACKUP_BUILD_NUMBER="$build_number"
app_dir="$out_root/${bundle_display_name}.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"

mkdir -p "$macos_dir"
mkdir -p "$resources_dir"

rm -f "$resources_dir/televybackup" "$resources_dir/televybackup-mtproto-helper" 2>/dev/null || true

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
  <key>LSMinimumSystemVersion</key>
  <string>15.0</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
PLIST

codesign_identity="${TELEVYBACKUP_CODESIGN_IDENTITY:--}"

if [[ -n "$codesign_identity" ]]; then
  echo "Codesigning with controlled identity: $codesign_identity"
  codesign --force --sign "$codesign_identity" -i "$bundle_id.cli" "$macos_dir/televybackup-cli" \
    || echo "WARN: codesign CLI failed"
  codesign --force --sign "$codesign_identity" -i "$bundle_id.mtproto-helper" "$macos_dir/televybackup-mtproto-helper" \
    || echo "WARN: codesign helper failed"
  codesign --force --deep --sign "$codesign_identity" "$app_dir" \
    || echo "WARN: codesign app failed"
else
  echo "No codesign identity found; applying ad-hoc signature for local runs"
  codesign --force --deep --sign - "$app_dir" \
    || echo "WARN: ad-hoc codesign app failed"
fi

codesign -vvv --deep --strict "$app_dir" >/dev/null 2>&1 \
  || echo "WARN: codesign verification failed (embedded CLI may be killed by macOS)"

echo "Built ($variant): $app_dir"
