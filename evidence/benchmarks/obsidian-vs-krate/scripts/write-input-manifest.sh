#!/usr/bin/env bash
# Every input of a run: kind, path, bytes, sha256, version and
# architecture. Written at the start and again at the end of a run;
# audit.py refuses a run whose inputs changed in between.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

output=${1:?usage: write-input-manifest.sh OUTPUT.tsv}
ob=$(obsidian_bin)
res="$OBSIDIAN_APP/Contents/Resources"
krate_version=$("$KRATE_BIN" version 2>/dev/null | /usr/bin/tr '\n' ' ' | /usr/bin/sed 's/  */ /g')
vault_manifest=$(/usr/bin/unzip -p "$KRATE_VAULT_BUNDLE" manifest.toml 2>/dev/null | /usr/bin/awk -F'"' '/^id =|^version =/{printf "%s ", $2}')
row() { printf '%s\t%s\t%s\t%s\t%s\n' "$@"; }
{
  row kind path bytes sha256 version_or_arch
  row obsidian_dmg "$(canonical_path "$OBSIDIAN_DMG")" "$(file_bytes "$OBSIDIAN_DMG")" "$(sha256_file "$OBSIDIAN_DMG")" "$OBSIDIAN_VERSION"
  row obsidian_binary "$(canonical_path "$ob")" "$(file_bytes "$ob")" "$(sha256_file "$ob")" \
    "$(/usr/bin/defaults read "$OBSIDIAN_APP/Contents/Info.plist" CFBundleShortVersionString) $(/usr/bin/lipo -archs "$ob")"
  row obsidian_app_asar "$(canonical_path "$res/app.asar")" "$(file_bytes "$res/app.asar")" "$(sha256_file "$res/app.asar")" -
  row obsidian_asar "$(canonical_path "$res/obsidian.asar")" "$(file_bytes "$res/obsidian.asar")" "$(sha256_file "$res/obsidian.asar")" -
  row krate_binary "$(canonical_path "$KRATE_BIN")" "$(file_bytes "$KRATE_BIN")" "$(sha256_file "$KRATE_BIN")" \
    "$krate_version; $(/usr/bin/lipo -archs "$KRATE_BIN" 2>/dev/null || printf unknown)"
  row vault_krate "$(canonical_path "$KRATE_VAULT_BUNDLE")" "$(file_bytes "$KRATE_VAULT_BUNDLE")" "$(sha256_file "$KRATE_VAULT_BUNDLE")" "${vault_manifest:-unknown}"
  for m in "$BUILD"/vaults/*.manifest.json; do
    row vault_tree "$(basename "$m" .manifest.json)" \
      "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["bytes"])' "$m")" \
      "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["tree_sha256"])' "$m")" make-bench-vault/1
  done
  for t in "$BUILD"/targets/*.json; do
    row target "$(basename "$t" .json)" "$(file_bytes "$t")" "$(sha256_file "$t")" -
  done
  for tool in start-probe tree-sample interact; do
    row tool "$tool" "$(file_bytes "$BUILD/$tool")" "$(sha256_file "$BUILD/$tool")" "$(/usr/bin/lipo -archs "$BUILD/$tool")"
  done
} >"$output"
