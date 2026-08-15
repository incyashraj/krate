#!/bin/sh
# What is wrong with Krate on this Mac.
#
# Written for the case that has cost the most time: someone cannot open Krate
# Studio, and cannot say why. "It doesn't work" is unactionable from a
# distance; this prints the handful of facts that actually distinguish the
# failures, so one paste answers it.
#
# Read-only. It installs nothing and changes nothing.
set -u

echo "Krate on this Mac"
echo "================="
echo

echo "This Mac"
echo "  macOS   : $(sw_vers -productVersion 2>/dev/null)"
echo "  Chip    : $(uname -m)   (arm64 = Apple silicon, x86_64 = Intel)"
echo

for app in "/Applications/Krate Studio.app" "/Applications/Krate.app"; do
  name=$(basename "$app")
  if [ ! -d "$app" ]; then
    echo "$name: NOT INSTALLED"
    echo
    continue
  fi
  echo "$name"
  exe="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist" 2>/dev/null)"
  echo "  Version : $(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist" 2>/dev/null)"
  echo "  Built for: $(lipo -archs "$exe" 2>/dev/null)"

  # The single most common silent failure: an app built for the other chip.
  case "$(lipo -archs "$exe" 2>/dev/null)" in
    *"$(uname -m)"*) ;;
    *) echo "  >>> PROBLEM: this build does not match this Mac's chip."
       echo "      It will not launch. Download the universal build from"
       echo "      https://krate.tech/studio/" ;;
  esac

  if codesign --verify --strict "$app" >/dev/null 2>&1; then
    echo "  Signature: valid"
  else
    echo "  >>> PROBLEM: the signature is broken. macOS will call this app"
    echo "      damaged. Delete it and download it again."
  fi

  verdict=$(spctl -a -t exec "$app" 2>&1)
  case "$verdict" in
    *accepted*|"") echo "  Gatekeeper: accepted" ;;
    *) echo "  >>> Gatekeeper: $verdict" ;;
  esac

  q=$(xattr -p com.apple.quarantine "$app" 2>/dev/null)
  [ -n "$q" ] && echo "  Quarantined: yes (normal for a fresh download)"
  echo
done

echo "The AI tools Krate can drive"
for tool in claude codex gemini copilot; do
  found=""
  for dir in $(echo "$PATH" | tr ':' ' ') "$HOME/.local/bin" "$HOME/bin" \
             /opt/homebrew/bin /usr/local/bin; do
    [ -x "$dir/$tool" ] && { found="$dir/$tool"; break; }
  done
  if [ -n "$found" ]; then echo "  $tool: $found"; else echo "  $tool: not found"; fi
done
echo

echo "Send this whole output to whoever is helping you."
