#!/bin/bash
# Build a styled .dmg from an already-signed app: the app on the left, an
# Applications alias on the right, icons large enough that "drag this there"
# is legible without instructions.
#
# hdiutil alone produces a plain small-icon window -- functional, but the
# install gesture is invisible. Finder is scripted to lay the window out on a
# writable image, which is then converted to the compressed read-only one
# people download. The app is COPIED, never rebuilt, so its signature and
# notarization ticket ride through untouched.
#
# Usage: make-styled-dmg.sh "<App.app path>" "<volume name>" "<output.dmg>"
set -euo pipefail
APP="$1"; VOL="$2"; OUT="$3"
STAGE="$(mktemp -d)/rw.dmg"
ROOT="$(mktemp -d)"
cp -R "$APP" "$ROOT/"
ln -s /Applications "$ROOT/Applications"
hdiutil create -volname "$VOL" -srcfolder "$ROOT" -ov -format UDRW "$STAGE" >/dev/null
MP=$(hdiutil attach "$STAGE" -nobrowse | tail -1 | sed 's/^.*\(\/Volumes\/.*\)$/\1/')
APPNAME="$(basename "$APP")"
# Finder scripting fails on some headless runners; the layout is cosmetic, so
# a failure falls through to the plain window rather than failing the build.
osascript >/dev/null 2>&1 <<OSA || echo "(finder layout skipped)"
tell application "Finder"
  tell disk "$VOL"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 120, 860, 480}
    set viewOptions to the icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 128
    set position of item "$APPNAME" of container window to {165, 175}
    set position of item "Applications" of container window to {495, 175}
    close
  end tell
end tell
OSA
sync
hdiutil detach "$MP" >/dev/null
rm -f "$OUT"
hdiutil convert "$STAGE" -format UDZO -o "$OUT" >/dev/null
echo "styled dmg: $OUT"
