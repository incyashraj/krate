#!/usr/bin/env bash
# Every input present, version-pinned and native, or a named refusal.
# Launches nothing.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

require_command python3
require_command swiftc
require_command footprint
host=$(/usr/bin/uname -m)

[[ -d "$OBSIDIAN_APP" ]] || die "Obsidian not installed at $OBSIDIAN_APP (run scripts/install-obsidian.sh)"
ob=$(obsidian_bin)
version=$(/usr/bin/defaults read "$OBSIDIAN_APP/Contents/Info.plist" CFBundleShortVersionString)
[[ "$version" == "$OBSIDIAN_VERSION" ]] || die "Obsidian at $OBSIDIAN_APP is $version, expected $OBSIDIAN_VERSION"
[[ " $(/usr/bin/lipo -archs "$ob") " == *" $host "* ]] || die "Obsidian has no $host slice: refusing a Rosetta run"
[[ "$(sha256_file "$OBSIDIAN_DMG")" == "$OBSIDIAN_DMG_SHA256" ]] || die "Obsidian DMG sha256 differs from $OBSIDIAN_DMG_SHA256"

[[ -x "$KRATE_BIN" ]] || die "Krate binary is not executable: $KRATE_BIN"
[[ " $(/usr/bin/lipo -archs "$KRATE_BIN" 2>/dev/null) " == *" $host "* ]] || die "Krate at $KRATE_BIN has no $host slice"
kv=$("$KRATE_BIN" version 2>/dev/null | /usr/bin/awk '$1 == "krate" {print $2; exit}')
[[ "${kv#v}" == "${KRATE_VERSION#v}" ]] || die "Krate at $KRATE_BIN reports version '$kv', expected $KRATE_VERSION (the binary on PATH is often not the one you meant: give an absolute path)"
[[ -f "$KRATE_VAULT_BUNDLE" ]] || die "Vault bundle not found: $KRATE_VAULT_BUNDLE"
id=$(/usr/bin/unzip -p "$KRATE_VAULT_BUNDLE" manifest.toml 2>/dev/null | /usr/bin/awk -F'"' '/^id =/{print $2; exit}')
[[ "$id" == dev.krate.vault ]] || die "$KRATE_VAULT_BUNDLE is not the Vault app (manifest id '$id')"

for load in $BENCH_LOADS $BENCH_INTERACT_LOADS; do
  [[ -d "$BUILD/vaults/$load" && -f "$BUILD/targets/$load.json" ]] || die "vault $load not prepared (run scripts/prepare.sh)"
done
for tool in start-probe tree-sample interact; do
  [[ -x "$BUILD/$tool" ]] || die "missing $BUILD/$tool (run scripts/prepare.sh)"
done

printf 'PASS\tinputs present, pinned and %s-native\n' "$host"
printf 'Obsidian\t%s\t%s\n' "$version" "$(/usr/bin/lipo -archs "$ob")"
printf 'Krate\t%s\t%s\n' "$kv" "$(sha256_file "$KRATE_BIN")"
printf 'Vault\t%s\t%s bytes\n' "$(sha256_file "$KRATE_VAULT_BUNDLE")" "$(file_bytes "$KRATE_VAULT_BUNDLE")"
