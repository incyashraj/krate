#!/usr/bin/env bash
#
# Package the iOS player for the simulator: a bare .app folder with the
# binary and a minimal Info.plist, ad-hoc signed. Simulator apps need no
# certificate; a device build is a different, Apple-account-shaped step.
#
# Run scripts (build gram + wall via check-app, then):
#   RUSTC=$HOME/.rustup/toolchains/<pin>/bin/rustc cargo build --release \
#     -p krate-player-ios --target aarch64-apple-ios-sim
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=target/aarch64-apple-ios-sim/release/krate-player-ios
OUT=target/ios-sim/KratePlayer.app
[ -f "$BIN" ] || { echo "error: build the player first" >&2; exit 1; }
rm -rf "$OUT" && mkdir -p "$OUT"
cp "$BIN" "$OUT/KratePlayer"
cat > "$OUT/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>tech.krate.player</string>
  <key>CFBundleName</key><string>Krate</string>
  <key>CFBundleDisplayName</key><string>Krate</string>
  <key>CFBundleExecutable</key><string>KratePlayer</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleShortVersionString</key><string>0.1.9</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>MinimumOSVersion</key><string>15.0</string>
  <key>UIDeviceFamily</key><array><integer>1</integer></array>
  <!-- Modern full-screen launch without a storyboard file. -->
  <key>UILaunchScreen</key><dict/>
  <!-- App-delegate lifecycle: no scene manifest on purpose. -->
</dict></plist>
PLIST
codesign -s - --force "$OUT" > /dev/null 2>&1 || true
echo "packaged: $OUT"
