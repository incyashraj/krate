#!/usr/bin/env bash
#
# Open a .krate on a connected Android phone or emulator, the way a tapped
# file opens it: install the Player built by scripts/package-android.sh (when
# it is newer than the one on the device), copy the bundle into the Player's
# own folder, and send it a VIEW intent for that file.
#
#   scripts/android-try.sh path/to/app.krate     open it
#   scripts/android-try.sh --shot out.png        save what the screen shows
#   scripts/android-try.sh --log                 the Player's lines from logcat
#
# With more than one device attached, say which: ANDROID_SERIAL=<id> (from
# `adb devices`).

set -euo pipefail

# Paths given are the caller's: make them whole before moving to the repo.
whole() { case "$1" in /*) echo "$1" ;; *) echo "$PWD/$1" ;; esac; }
ARG1=${1:-}
ARG2=${2:-}
case "$ARG1" in
  --*) [ -n "$ARG2" ] && [ "$ARG1" = --shot ] && ARG2=$(whole "$ARG2") ;;
  ?*) ARG1=$(whole "$ARG1") ;;
esac
set -- ${ARG1:+"$ARG1"} ${ARG2:+"$ARG2"}

cd "$(dirname "$0")/.."

APK=target/android/krate-player.apk
PKG=tech.krate.player
ACTIVITY=$PKG/.KrateActivity
DIR=/sdcard/Android/data/$PKG/files

if ! command -v adb >/dev/null; then
  echo "error: adb not found -- brew install android-platform-tools" >&2
  exit 1
fi
devices=$(adb devices | sed -n '2,$p' | awk '$2 == "device" {print $1}')
if [ -z "$devices" ]; then
  if adb devices | grep -q unauthorized; then
    echo "error: the phone has not allowed this computer -- unlock it and tap Allow on \"Allow USB debugging?\"" >&2
  else
    echo "error: no device -- plug the phone in with USB debugging on and allow this computer" >&2
  fi
  exit 1
fi
# A phone and an emulator at once: the phone, if there is just one.
if [ -z "${ANDROID_SERIAL:-}" ] && [ "$(echo "$devices" | wc -l | tr -d ' ')" != 1 ]; then
  phones=$(echo "$devices" | grep -v '^emulator-' || true)
  if [ "$(echo "$phones" | grep -c .)" = 1 ]; then
    export ANDROID_SERIAL=$phones
  else
    echo "error: more than one device -- say which: ANDROID_SERIAL=<id> $0 ..." >&2
    echo "$devices" >&2
    exit 1
  fi
fi

case "${1:-}" in
  --shot)
    adb exec-out screencap -p > "${2:?where to save the picture}"
    echo "saved: $2"
    exit 0
    ;;
  --log)
    adb logcat -d -v time | grep -E "krate|Krate|RustStdoutStderr|AndroidRuntime" | tail -${2:-80}
    exit 0
    ;;
  "")
    echo "usage: $0 app.krate | --shot out.png | --log [lines]" >&2
    exit 2
    ;;
esac

BUNDLE=$1
if [ ! -f "$BUNDLE" ]; then
  echo "error: $BUNDLE not found" >&2
  exit 1
fi

# Install when the device has no Player, or not this build of it: the
# build's digest is left in the Player's own folder, which goes with it.
if [ -f "$APK" ]; then
  built=$(shasum -a 256 "$APK" | cut -c1-64)
  on_device=$(adb shell cat "$DIR/.player-build" 2>/dev/null | tr -d '\r' || true)
  if [ "$built" != "$on_device" ]; then
    echo "installing $APK"
    out=$(adb install -r "$APK" 2>&1 || true)
    case "$out" in
      *INSTALL_FAILED_UPDATE_INCOMPATIBLE*)
        # A Player signed with another key (another Mac's test key, or a
        # release): Android refuses to update it in place. Replacing it
        # forgets what its apps kept on this device.
        echo "the Player on the device was signed with another key -- replacing it (its apps' data goes with it)"
        adb uninstall $PKG >/dev/null
        out=$(adb install "$APK" 2>&1 || true)
        ;;
    esac
    case "$out" in
      *Success*) ;;
      *) echo "error: install failed: $out" >&2; exit 1 ;;
    esac
    adb shell mkdir -p "$DIR" >/dev/null
    adb shell "echo $built > $DIR/.player-build"
  fi
elif ! adb shell pm path $PKG >/dev/null 2>&1; then
  echo "error: no Player on the device and $APK not built -- scripts/build-android.sh, then scripts/package-android.sh" >&2
  exit 1
fi

name=$(basename "$BUNDLE")
adb shell mkdir -p "$DIR" >/dev/null
adb push "$BUNDLE" "$DIR/$name" >/dev/null
adb shell am force-stop $PKG
adb logcat -c
adb shell am start -a android.intent.action.VIEW -t application/octet-stream \
  -d "file://$DIR/$name" -n $ACTIVITY >/dev/null
echo "opened $name -- the wall comes first; tap Open on the phone"
