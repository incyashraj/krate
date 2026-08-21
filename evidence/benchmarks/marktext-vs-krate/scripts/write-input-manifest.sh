#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

output=${1:?usage: write-input-manifest.sh OUTPUT.tsv}
MARKTEXT_BIN=$(marktext_binary)
krate_version=$($KRATE_BIN version 2>/dev/null || $KRATE_BIN --version)
krate_commit=$(printf '%s\n' "$krate_version" | /usr/bin/awk '$1 == "commit" {print $2; exit}')
{
  printf 'kind\tpath\tbytes_or_kib\tsha256\tversion_or_arch\n'
  printf 'marktext_zip_or_app\t%s\t%s KiB\t-\t%s\n' \
    "$(canonical_path "$MARKTEXT_APP")" "$(app_kib "$MARKTEXT_APP")" \
    "$(/usr/bin/defaults read "$MARKTEXT_APP/Contents/Info.plist" CFBundleShortVersionString)"
  printf 'marktext_binary\t%s\t%s bytes\t%s\t%s\n' \
    "$(canonical_path "$MARKTEXT_BIN")" "$(file_bytes "$MARKTEXT_BIN")" \
    "$(sha256_file "$MARKTEXT_BIN")" "$(/usr/bin/file "$MARKTEXT_BIN")"
  printf 'krate_bundle\t%s\t%s bytes\t%s\tbundle\n' \
    "$(canonical_path "$KRATE_BUNDLE")" "$(file_bytes "$KRATE_BUNDLE")" \
    "$(sha256_file "$KRATE_BUNDLE")"
  printf 'krate_binary\t%s\t%s bytes\t%s\t%s\n' \
    "$(canonical_path "$KRATE_BIN")" "$(file_bytes "$KRATE_BIN")" \
    "$(sha256_file "$KRATE_BIN")" "commit=$krate_commit; $(/usr/bin/file "$KRATE_BIN")"
  if [[ -n "${KRATE_STUDIO_APP:-}" && -d "$KRATE_STUDIO_APP" ]]; then
    printf 'krate_studio\t%s\t%s KiB\t-\t%s\n' \
      "$(canonical_path "$KRATE_STUDIO_APP")" "$(app_kib "$KRATE_STUDIO_APP")" \
      "$(/usr/bin/defaults read "$KRATE_STUDIO_APP/Contents/Info.plist" CFBundleShortVersionString 2>/dev/null || printf unknown)"
  fi
  for fixture in "$ROOT/fixtures/notes-5000.md" "$ROOT/fixtures/notes-50000.md"; do
    printf 'fixture\t%s\t%s bytes\t%s\tUTF-8 Markdown\n' \
      "$(canonical_path "$fixture")" "$(file_bytes "$fixture")" "$(sha256_file "$fixture")"
  done
} >"$output"
