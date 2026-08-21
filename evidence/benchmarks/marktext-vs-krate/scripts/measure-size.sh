#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-size.sh RUN_DIR}
output="$run_dir/raw/size.tsv"
/bin/mkdir -p "$run_dir/raw/size-breakdown"
ensure_header "$output" 'timestamp_utc\tartifact\tmeasurement\tvalue\tunit\tsha256\tpath'

MARKTEXT_BIN=$(marktext_binary)
printf '%s\tMarkText_0.17.1_ARM64\tinstalled_size\t%s\tKiB\t-\t%s\n' \
  "$(ts_utc)" "$(app_kib "$MARKTEXT_APP")" "$(canonical_path "$MARKTEXT_APP")" >>"$output"
printf '%s\tMarkText_binary\tfile_size\t%s\tbytes\t%s\t%s\n' \
  "$(ts_utc)" "$(file_bytes "$MARKTEXT_BIN")" "$(sha256_file "$MARKTEXT_BIN")" \
  "$(canonical_path "$MARKTEXT_BIN")" >>"$output"
printf '%s\tKrate_notes_bundle\tfile_size\t%s\tbytes\t%s\t%s\n' \
  "$(ts_utc)" "$(file_bytes "$KRATE_BUNDLE")" "$(sha256_file "$KRATE_BUNDLE")" \
  "$(canonical_path "$KRATE_BUNDLE")" >>"$output"

if [[ -n "${KRATE_STUDIO_APP:-}" && -d "$KRATE_STUDIO_APP" ]]; then
  printf '%s\tKrate_Studio_shared_runtime\tinstalled_size\t%s\tKiB\t-\t%s\n' \
    "$(ts_utc)" "$(app_kib "$KRATE_STUDIO_APP")" "$(canonical_path "$KRATE_STUDIO_APP")" >>"$output"
fi

/usr/bin/du -sk "$MARKTEXT_APP/Contents"/* >"$run_dir/raw/size-breakdown/marktext-contents.tsv"
unzip -l "$KRATE_BUNDLE" >"$run_dir/raw/size-breakdown/krate-bundle-list.txt"
