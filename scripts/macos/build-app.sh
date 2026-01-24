#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

app_name="TelevyBackup"
src_dir="$root_dir/macos/TelevyBackupApp"
out_root="$root_dir/target/macos-app"
app_dir="$out_root/${app_name}.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"

mkdir -p "$macos_dir"
mkdir -p "$resources_dir"

rm -f "$resources_dir/televybackup" "$resources_dir/televybackup-mtproto-helper" 2>/dev/null || true

echo "Building CLI..."
cargo build -p televybackup --release
cp "$root_dir/target/release/televybackup" "$macos_dir/televybackup-cli"

echo "Building MTProto helper..."
cargo build --manifest-path "$root_dir/crates/mtproto-helper/Cargo.toml" --release
cp "$root_dir/crates/mtproto-helper/target/release/televybackup-mtproto-helper" "$macos_dir/televybackup-mtproto-helper"

sdk_path="$(xcrun --sdk macosx --show-sdk-path)"

xcrun swiftc \
  -sdk "$sdk_path" \
  -parse-as-library \
  -O \
  -framework SwiftUI \
  -framework AppKit \
  -o "$macos_dir/$app_name" \
  "$src_dir"/*.swift

cat > "$contents_dir/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>TelevyBackup</string>
  <key>CFBundleDisplayName</key>
  <string>TelevyBackup</string>
  <key>CFBundleIdentifier</key>
  <string>com.ivan.televybackup</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleExecutable</key>
  <string>TelevyBackup</string>
  <key>LSMinimumSystemVersion</key>
  <string>15.0</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
PLIST

codesign_identity="${TELEVYBACKUP_CODESIGN_IDENTITY:-}"
if [[ -z "$codesign_identity" ]]; then
  codesign_identity="$(
    security find-identity -v -p codesigning 2>/dev/null \
      | awk -F'"' '/Apple Development|Developer ID Application/ {print $2; exit}'
  )"
fi

if [[ -n "$codesign_identity" ]]; then
  echo "Codesigning with: $codesign_identity"
  codesign --force --sign "$codesign_identity" -i com.ivan.televybackup.cli "$macos_dir/televybackup-cli" \
    || echo "WARN: codesign CLI failed (Keychain prompts may repeat)"
  codesign --force --sign "$codesign_identity" -i com.ivan.televybackup.mtproto-helper "$macos_dir/televybackup-mtproto-helper" \
    || echo "WARN: codesign helper failed (Keychain prompts may repeat)"
  codesign --force --deep --sign "$codesign_identity" "$app_dir" \
    || echo "WARN: codesign app failed"
else
  echo "No codesign identity found; applying ad-hoc signature for local runs"
  codesign --force --deep --sign - "$app_dir" \
    || echo "WARN: ad-hoc codesign app failed"
fi

codesign -vvv --deep --strict "$app_dir" >/dev/null 2>&1 \
  || echo "WARN: codesign verification failed (embedded CLI may be killed by macOS)"

echo "Built: $app_dir"
