#!/bin/sh
# Assemble Krate.app, the macOS double-click entry point (P3-OPEN-03).
#
# The bundle declares the .krate document type, so Finder routes a
# double-clicked .krate here. Launch Services starts the shim executable,
# which execs `krate open-app`; that waits for the open-document Apple event,
# then runs the bundle behind the native consent wall. Same binary, same
# enforcement — only the entry gesture is new.
#
# Usage: scripts/make-macos-app.sh [output-dir] [--release]
#   output-dir defaults to dist/. Pass --release to package the release build.
set -eu

case "$(uname -s)" in
  Darwin) ;;
  *) echo "Krate.app can only be assembled on macOS" >&2; exit 1 ;;
esac

OUT_DIR="dist"
PROFILE="debug"
for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release" ;;
    *) OUT_DIR="$arg" ;;
  esac
done

BINARY="target/$PROFILE/krate"
if [ ! -x "$BINARY" ]; then
  echo "missing $BINARY — build it first (cargo build -p krate-cli${PROFILE:+ --$PROFILE})" >&2
  exit 1
fi

APP="$OUT_DIR/Krate.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Named krate-cli, not krate: the shim below is "Krate", and macOS filesystems
# are case-insensitive by default, so "krate" and "Krate" would be one file.
cp "$BINARY" "$APP/Contents/MacOS/krate-cli"

# Launch Services invokes CFBundleExecutable with no arguments, so a shim
# execs the real binary in open-app mode. exec keeps the PID, so the Apple
# event still reaches the process Launch Services launched.
cat > "$APP/Contents/MacOS/Krate" << 'SHIM'
#!/bin/sh
exec "$(dirname "$0")/krate-cli" open-app
SHIM
chmod +x "$APP/Contents/MacOS/Krate"

VERSION="$("$BINARY" --version | awk '{print $2}')"

cat > "$APP/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Krate</string>
    <key>CFBundleDisplayName</key>
    <string>Krate</string>
    <key>CFBundleIdentifier</key>
    <string>dev.krate.app</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleExecutable</key>
    <string>Krate</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Krate App Bundle</string>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>dev.krate.bundle</string>
            </array>
            <key>LSHandlerRank</key>
            <string>Owner</string>
        </dict>
    </array>
    <key>UTExportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key>
            <string>dev.krate.bundle</string>
            <key>UTTypeDescription</key>
            <string>Krate App Bundle</string>
            <key>UTTypeConformsTo</key>
            <array>
                <string>public.data</string>
            </array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array>
                    <string>krate</string>
                </array>
            </dict>
        </dict>
    </array>
</dict>
</plist>
PLIST

# Tell Launch Services about the bundle so Finder associates .krate with it.
LSREGISTER="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
if [ -x "$LSREGISTER" ]; then
  "$LSREGISTER" -f "$APP" >/dev/null 2>&1 || true
fi

echo "assembled $APP (profile: $PROFILE, version: $VERSION)"
echo "test: open a .krate with it — e.g."
echo "  open -a \"\$PWD/$APP\" path/to/app.krate"
