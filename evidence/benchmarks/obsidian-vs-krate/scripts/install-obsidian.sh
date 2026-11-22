#!/usr/bin/env bash
# Install Obsidian for the benchmark without opening it and without any
# dialog (IC-999): verify the DMG's sha256, mount it read-only and hidden,
# check the app's version, architectures and signature, copy it with ditto
# to OBSIDIAN_APP (under ~/krate-bench/apps, never /Applications), detach.
#
#   install-obsidian.sh            install (refuses to overwrite)
#   install-obsidian.sh --check    verify the DMG only; copies nothing
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

check_only=0
[[ "${1:-}" == --check ]] && check_only=1

[[ -f "$OBSIDIAN_DMG" ]] || die "no DMG at $OBSIDIAN_DMG (scripts/fetch-obsidian.sh downloads it)"
actual=$(sha256_file "$OBSIDIAN_DMG")
[[ "$actual" == "$OBSIDIAN_DMG_SHA256" ]] || die "DMG sha256 $actual, expected $OBSIDIAN_DMG_SHA256"

mnt=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/obsidian-dmg.XXXXXX")
detach() { /usr/bin/hdiutil detach "$mnt" -quiet 2>/dev/null || /usr/bin/hdiutil detach "$mnt" -force -quiet 2>/dev/null || true; /bin/rmdir "$mnt" 2>/dev/null || true; }
trap detach EXIT
/usr/bin/hdiutil attach -nobrowse -readonly -noautoopen -mountpoint "$mnt" "$OBSIDIAN_DMG" >/dev/null 2>&1 \
  || die "could not mount $OBSIDIAN_DMG read-only"
src="$mnt/Obsidian.app"
[[ -x "$src/Contents/MacOS/Obsidian" ]] || die "no Obsidian.app in the DMG"

version=$(/usr/bin/defaults read "$src/Contents/Info.plist" CFBundleShortVersionString)
archs=$(/usr/bin/lipo -archs "$src/Contents/MacOS/Obsidian")
team=$(/usr/bin/codesign -dv "$src" 2>&1 | /usr/bin/awk -F= '/^TeamIdentifier/{print $2}')
[[ "$version" == "$OBSIDIAN_VERSION" ]] || die "DMG holds Obsidian $version, expected $OBSIDIAN_VERSION"
[[ " $archs " == *" $(/usr/bin/uname -m) "* ]] || die "Obsidian is $archs; this Mac is $(/usr/bin/uname -m) -- refusing a Rosetta run"
/usr/bin/codesign --verify --deep --strict "$src" 2>/dev/null || die "Obsidian's signature does not verify"
printf 'DMG\t%s\t%s bytes\tsha256 %s\n' "$OBSIDIAN_DMG" "$(file_bytes "$OBSIDIAN_DMG")" "$actual"
printf 'Obsidian\t%s\tarchs: %s\tteam %s\tsignature verifies\n' "$version" "$archs" "$team"
if /usr/bin/xattr "$OBSIDIAN_DMG" 2>/dev/null | /usr/bin/grep -q com.apple.quarantine; then
  printf 'WARN\tthe DMG is quarantined; the copied app would show the Gatekeeper first-open dialog\n'
fi
((check_only)) && exit 0

[[ ! -e "$OBSIDIAN_APP" ]] || die "$OBSIDIAN_APP exists; remove it yourself to reinstall"
case "$(canonical_path "$(dirname "$OBSIDIAN_APP")")" in
  /Applications*|/System*|/Library*) die "OBSIDIAN_APP must not be a system folder" ;;
esac
/bin/mkdir -p "$(dirname "$OBSIDIAN_APP")"
/usr/bin/ditto "$src" "$OBSIDIAN_APP"
if /usr/bin/xattr -r "$OBSIDIAN_APP" 2>/dev/null | /usr/bin/grep -q com.apple.quarantine; then
  die "the installed copy is quarantined, so its first launch would show a Gatekeeper dialog. Not clearing it silently: decide, then run: xattr -dr com.apple.quarantine \"$OBSIDIAN_APP\""
fi
/usr/bin/codesign --verify --deep --strict "$OBSIDIAN_APP" 2>/dev/null || die "the installed copy's signature does not verify"
printf 'installed\t%s\t%s KiB (du -sk)\n' "$OBSIDIAN_APP" "$(/usr/bin/du -sk "$OBSIDIAN_APP" | /usr/bin/awk '{print $1}')"
