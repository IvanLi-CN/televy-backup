#!/usr/bin/env bash
set -euo pipefail

root_dir="$(git rev-parse --show-toplevel)"

app_name="TelevyBackup"
src="$root_dir/macos/TelevyBackupApp/TelevyBackupApp.swift"
out_root="$root_dir/target/macos-app"
app_dir="$out_root/${app_name}.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"

mkdir -p "$macos_dir"
mkdir -p "$resources_dir"

echo "Building CLI..."
cargo build -p televybackup --release
cp "$root_dir/target/release/televybackup" "$resources_dir/televybackup"

sdk_path="$(xcrun --sdk macosx --show-sdk-path)"

xcrun swiftc \
  -sdk "$sdk_path" \
  -parse-as-library \
  -O \
  -framework SwiftUI \
  -framework AppKit \
  -o "$macos_dir/$app_name" \
  "$src"

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
</dict>
</plist>
PLIST

echo "Built: $app_dir"
