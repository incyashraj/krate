#!/usr/bin/env bash
#
# Package and install the iOS player on a real, personally-owned iPhone,
# signed with a free personal team. No paid account needed; the signature
# lasts 7 days and then the app must be reinstalled.
#
# One-time setup (the human's part -- these steps need an Apple ID and a
# password, so no script can do them):
#
#   1. sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
#   2. Xcode > Settings > Accounts > add your Apple ID (a "Personal Team"
#      appears).
#   3. Make a throwaway iOS App project in Xcode with bundle identifier
#      tech.krate.player, team = your Personal Team, plug in the iPhone,
#      run it once on the device. That single run makes Xcode mint the
#      signing certificate, register the device, and write a provisioning
#      profile -- the two artifacts this script reuses.
#   4. On the iPhone: Settings > Privacy & Security > Developer Mode > on
#      (reboots), and trust the computer when asked.
#
# Then:
#   IDENTITY="Apple Development: you@example.com (TEAMID123)" \
#   PROFILE=/path/to/embedded.mobileprovision \
#   scripts/package-ios-device.sh
#
# Find the identity name:  security find-identity -v -p codesigning
# Find the profile: ~/Library/Developer/Xcode/DerivedData/<throwaway>/
#   Build/Products/Debug-iphoneos/<app>.app/embedded.mobileprovision
# (or ~/Library/MobileDevice/Provisioning Profiles/, newest file)

set -euo pipefail
cd "$(dirname "$0")/.."

BIN=target/aarch64-apple-ios/release/krate-player-ios
OUT=target/ios-device/KratePlayer.app
: "${IDENTITY:?set IDENTITY to your Apple Development signing identity}"
: "${PROFILE:?set PROFILE to the .mobileprovision path}"
[ -f "$BIN" ] || { echo "error: build the device binary first:
  cargo build --release -p krate-player-ios --target aarch64-apple-ios" >&2; exit 1; }

rm -rf "$OUT" && mkdir -p "$OUT"
cp "$BIN" "$OUT/KratePlayer"
cp "$PROFILE" "$OUT/embedded.mobileprovision"
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
  <key>UILaunchScreen</key><dict/>
</dict></plist>
PLIST

# The entitlements must match what the provisioning profile grants; the
# minimal personal-team set is the application identifier and team id,
# extracted straight from the profile so they cannot disagree.
security cms -D -i "$PROFILE" > /tmp/krate-profile.plist
/usr/libexec/PlistBuddy -x -c 'Print :Entitlements' /tmp/krate-profile.plist \
  > /tmp/krate-entitlements.plist
codesign --force --sign "$IDENTITY" \
  --entitlements /tmp/krate-entitlements.plist "$OUT"
echo "signed: $OUT"

# Install over USB (or trusted Wi-Fi). devicectl ships with Xcode 15+.
DEVICE=$(xcrun devicectl list devices 2>/dev/null | awk '/iPhone/ {print $NF; exit}')
if [ -n "${DEVICE:-}" ]; then
  xcrun devicectl device install app --device "$DEVICE" "$OUT"
  echo "installed on $DEVICE -- the Krate icon is on your home screen"
else
  echo "no iPhone found; plug it in and rerun, or install manually:"
  echo "  xcrun devicectl device install app --device <id> $OUT"
fi
