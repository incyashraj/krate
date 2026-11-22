#!/usr/bin/env bash
# Download Obsidian 1.15.1's official DMG for a reproduction and verify its
# pinned sha256. The founder's copy came from the same URL on 2026-11-22.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

url=https://github.com/obsidianmd/obsidian-releases/releases/download/v1.15.1/Obsidian-1.15.1.dmg
[[ -f "$OBSIDIAN_DMG" ]] && { printf 'already present: %s\n' "$OBSIDIAN_DMG"; exec "$ROOT/scripts/install-obsidian.sh" --check; }
/bin/mkdir -p "$(dirname "$OBSIDIAN_DMG")"
/usr/bin/curl --fail --location --proto '=https' --tlsv1.2 "$url" --output "$OBSIDIAN_DMG.partial"
actual=$(sha256_file "$OBSIDIAN_DMG.partial")
[[ "$actual" == "$OBSIDIAN_DMG_SHA256" ]] || die "download sha256 $actual, expected $OBSIDIAN_DMG_SHA256"
/bin/mv "$OBSIDIAN_DMG.partial" "$OBSIDIAN_DMG"
printf 'downloaded %s (sha256 %s)\n' "$OBSIDIAN_DMG" "$actual"
