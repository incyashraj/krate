#!/usr/bin/env bash
# Size leg: what each side costs to download and to keep installed.
# du -sk is allocated KiB; du -A -sk is apparent size (bytes as listed).
set -euo pipefail
source "$(dirname "$0")/lib.sh"
load_config

run_dir=${1:?usage: measure-size.sh RUN_DIR}
out="$run_dir/raw/size.tsv"
/bin/mkdir -p "$run_dir/raw/size-breakdown"
ensure_header "$out" 'timestamp_utc\tapp\titem\tmeasurement\tvalue\tunit\tsha256\tpath'

row() { append_row "$out" "$(ts_utc)" "$@"; }
sized() {
  local app=$1 item=$2 path=$3
  if [[ -d "$path" ]]; then
    row "$app" "$item" allocated "$(/usr/bin/du -sk "$path" | /usr/bin/awk '{print $1}')" KiB - "$(canonical_path "$path")"
    row "$app" "$item" apparent "$(/usr/bin/du -A -sk "$path" | /usr/bin/awk '{print $1}')" KiB - "$(canonical_path "$path")"
    row "$app" "$item" files "$(/usr/bin/find "$path" -type f | /usr/bin/wc -l | /usr/bin/tr -d ' ')" count - "$(canonical_path "$path")"
  elif [[ -f "$path" ]]; then
    row "$app" "$item" file_size "$(file_bytes "$path")" bytes "$(sha256_file "$path")" "$(canonical_path "$path")"
    row "$app" "$item" allocated "$(/usr/bin/du -sk "$path" | /usr/bin/awk '{print $1}')" KiB - "$(canonical_path "$path")"
  else
    row "$app" "$item" missing - - - "$path"
  fi
}

sized obsidian download "$OBSIDIAN_DMG"
sized obsidian installed_app "$OBSIDIAN_APP"
sized vault app_bundle "$KRATE_VAULT_BUNDLE"
if [[ -n "$KRATE_INSTALLER" ]]; then sized vault krate_download "$KRATE_INSTALLER"; else row vault krate_download missing - - - "KRATE_INSTALLER not set"; fi
IFS=: read -r -a krate_paths <<<"$KRATE_INSTALLED_PATHS"
for p in "${krate_paths[@]}"; do
  if [[ -n "$p" ]]; then sized vault "krate_installed:$(basename "$p")" "$p"; fi
done

/usr/bin/du -sk "$OBSIDIAN_APP/Contents"/* "$OBSIDIAN_APP/Contents/Frameworks"/* >"$run_dir/raw/size-breakdown/obsidian-contents.tsv" 2>/dev/null || true
/usr/bin/unzip -l "$KRATE_VAULT_BUNDLE" >"$run_dir/raw/size-breakdown/vault-bundle-list.txt"
