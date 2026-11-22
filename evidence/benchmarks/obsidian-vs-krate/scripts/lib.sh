#!/usr/bin/env bash
# Shared by every script of the Obsidian-versus-Krate kit (IC-999).
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
REPO=$(CDPATH= cd -- "$ROOT/../../.." && pwd)
CONFIG=${BENCH_CONFIG:-"$ROOT/config.env"}
BUILD="$ROOT/build"
APPS=(vault obsidian)

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

note() { printf '%s %s\n' "$(/bin/date '+%H:%M:%S')" "$*"; }

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

load_config() {
  [[ -f "$CONFIG" ]] || die "copy $ROOT/config.example.env to $CONFIG and set the paths"
  # shellcheck disable=SC1090
  source "$CONFIG"
  : "${OBSIDIAN_DMG:?}" "${OBSIDIAN_DMG_SHA256:?}" "${OBSIDIAN_VERSION:?}" "${OBSIDIAN_APP:?}"
  : "${KRATE_BIN:?}" "${KRATE_VERSION:?}" "${KRATE_VAULT_BUNDLE:?}" "${KRATE_VAULT_GRANTS:?}"
  : "${KRATE_INSTALLER:=}" "${KRATE_INSTALLED_PATHS:=}"
  : "${BENCH_WORK_DIR:=$HOME/krate-bench/work}"
  : "${BENCH_LEGS:=size startup resources network interaction growth}"
  : "${BENCH_LOADS:=light medium heavy big-50000 big-200000}"
  : "${BENCH_WARMUPS:=2}" "${BENCH_WARM_RUNS:=10}" "${BENCH_COLD_RUNS:=5}" "${BENCH_RESOURCE_RUNS:=3}"
  : "${BENCH_SETTLE_SECONDS:=30}" "${BENCH_IDLE_SECONDS:=120}" "${BENCH_NETWORK_SECONDS:=60}"
  : "${BENCH_GROWTH_MINUTES:=60}" "${BENCH_INTERACT_LOADS:=light heavy}" "${BENCH_INTERACT_RUNS:=10}"
  : "${BENCH_TYPING_RUNS:=3}" "${BENCH_TYPING_KEYS:=30}" "${BENCH_SCROLL_RUNS:=3}"
  : "${BENCH_SCROLL_SECONDS:=20}" "${BENCH_SCROLL_HZ:=125}" "${BENCH_BATTERY_MINUTES:=30}"
  : "${BENCH_WINDOW_W:=1280}" "${BENCH_WINDOW_H:=800}"
}

sha256_file() { /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'; }
file_bytes() { /usr/bin/stat -f '%z' "$1"; }
ts_utc() { /bin/date -u '+%Y-%m-%dT%H:%M:%SZ'; }
epoch() { python3 -c 'import time; print(f"{time.time():.3f}")'; }

canonical_path() {
  python3 -c 'import sys; from pathlib import Path; print(Path(sys.argv[1]).expanduser().resolve())' "$1"
}

ensure_header() {
  [[ -s "$1" ]] || printf '%b\n' "$2" >"$1"
}

# One row, tab-separated, appended. Values must not contain tabs.
append_row() {
  local file=$1
  shift
  local IFS=$'\t'
  printf '%s\n' "$*" >>"$file"
}

process_tree() {
  local -a queue=("$1") seen=()
  local pid child
  while ((${#queue[@]})); do
    pid=${queue[0]}
    queue=("${queue[@]:1}")
    /bin/kill -0 "$pid" 2>/dev/null || continue
    seen+=("$pid")
    while IFS= read -r child; do
      [[ -n "$child" ]] && queue+=("$child")
    done < <(/usr/bin/pgrep -P "$pid" 2>/dev/null || true)
  done
  ((${#seen[@]})) && printf '%s\n' "${seen[@]}"
  return 0
}

# Stop a tree and wait until every process of it has gone (up to 15 s), so
# the next launch never overlaps the last one.
kill_tree() {
  local root=$1 pid i
  local -a pids=()
  while IFS= read -r pid; do [[ -n "$pid" ]] && pids+=("$pid"); done < <(process_tree "$root")
  ((${#pids[@]})) || return 0
  /bin/kill -TERM "${pids[@]}" 2>/dev/null || true
  for ((i = 0; i < 50; i++)); do
    local alive=0
    for pid in "${pids[@]}"; do /bin/kill -0 "$pid" 2>/dev/null && alive=1; done
    ((alive)) || return 0
    /bin/sleep 0.3
  done
  /bin/kill -KILL "${pids[@]}" 2>/dev/null || true
  /bin/sleep 0.5
}

obsidian_bin() { printf '%s/Contents/MacOS/Obsidian\n' "$OBSIDIAN_APP"; }

# Obsidian's command-line socket lives in the home folder, not the profile
# (PROTOCOL.md 4, assumption 5): never measure beside another Obsidian.
refuse_other_obsidian() {
  local others
  others=$(/usr/bin/pgrep -x Obsidian 2>/dev/null || true)
  [[ -z "$others" ]] || die "another Obsidian is running (pid $others); quit it, then rerun"
}

# The note each load opens, its phrases, and the interaction targets.
target_field() {
  python3 - "$BUILD/targets/$1.json" "$2" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
for k in sys.argv[2].split("."):
    d = d[k]
print(d)
PY
}

work_root() { printf '%s/%s.noindex\n' "$BENCH_WORK_DIR" "$(basename "$1")"; }

# A clone of the generated vault (APFS copy-on-write where possible).
copy_vault() {
  local load=$1 dest=$2
  /bin/mkdir -p "$(dirname "$dest")"
  /bin/cp -cR "$BUILD/vaults/$load" "$dest" 2>/dev/null || /bin/cp -R "$BUILD/vaults/$load" "$dest"
}

krate_profile_name() {
  printf 'bench-%s\n' "$(printf '%s' "$1" | /usr/bin/shasum | /usr/bin/cut -c1-20)"
}

# Sets LAUNCH (the command array) and PROFILE for APP, LOAD and LABEL. A
# profile that exists is reused (warm); a new label makes a new one (cold).
prepare_launch() {
  local run_dir=$1 app=$2 load=$3 label=$4
  PROFILE="$(work_root "$run_dir")/$label/$app/$load"
  local note
  note=$(target_field "$load" open_note)
  if [[ "$app" == obsidian ]]; then
    [[ -d "$PROFILE/vault" ]] || copy_vault "$load" "$PROFILE/vault"
    python3 "$ROOT/scripts/obsidian-profile.py" prepare --user-data "$PROFILE/user-data" \
      --vault "$PROFILE/vault" --note "$note" --width "$BENCH_WINDOW_W" --height "$BENCH_WINDOW_H" \
      >"$PROFILE.prepare.json"
    LAUNCH=("$(obsidian_bin)" "--user-data-dir=$PROFILE/user-data")
  elif [[ "$app" == vault ]]; then
    [[ -d "$PROFILE/root/vault" ]] || copy_vault "$load" "$PROFILE/root/vault"
    /bin/mkdir -p "$PROFILE/krate-cache"
    local -a grants=()
    local g
    set -f
    for g in $KRATE_VAULT_GRANTS; do grants+=(--grant "$g"); done
    set +f
    # Krate keeps a named profile's saved data under ~/.krate/profiles/;
    # the names are listed so they can be removed after the session.
    /bin/mkdir -p "$PROFILE"
    krate_profile_name "$PROFILE" >>"$(work_root "$run_dir")/krate-profiles.txt"
    LAUNCH=(/usr/bin/env "KRATE_CACHE_DIR=$PROFILE/krate-cache" "$KRATE_BIN" run "$KRATE_VAULT_BUNDLE"
      --profile "$(krate_profile_name "$PROFILE")" --sandbox-root "$PROFILE/root" "${grants[@]}"
      -- vault=./vault "open=$note" trace)
  else
    die "unknown app: $app"
  fi
}

# Exe prefixes that belong to APP, for helpers that are not its children.
app_exe_prefix() {
  if [[ "$1" == obsidian ]]; then canonical_path "$OBSIDIAN_APP"; else canonical_path "$KRATE_BIN"; fi
}

# ABBA: run 1 vault,obsidian; 2 obsidian,vault; 3 obsidian,vault; 4 vault,obsidian; ...
abba_order() {
  local phase=$((($1 - 1) % 4))
  if ((phase == 0 || phase == 3)); then printf 'vault obsidian\n'; else printf 'obsidian vault\n'; fi
}

timeout_for() {
  local load=$1 mode=$2
  if [[ "$mode" == cold && ("$load" == heavy || "$load" == big-200000) ]]; then printf '180\n'; else printf '60\n'; fi
}

# Launch through start-probe and leave the app running. Sets LAUNCH_JSON
# (the probe's output) and PROFILE; call it directly, not in $(...), or
# PROFILE is lost with the subshell.
launch_keep() {
  local run_dir=$1 app=$2 load=$3 label=$4 tag=$5 timeout=${6:-60}
  prepare_launch "$run_dir" "$app" "$load" "$label"
  if [[ "$app" == obsidian ]]; then refuse_other_obsidian; fi
  local json="$run_dir/raw/launch/$tag.json"
  /bin/mkdir -p "$run_dir/raw/launch" "$run_dir/raw/logs"
  "$BUILD/start-probe" --timeout "$timeout" --phrases "$BUILD/targets/$load.open.tsv" \
    --child-log "$run_dir/raw/logs/$tag.log" -- "${LAUNCH[@]}" >"$json" 2>"$json.stderr" || true
  LAUNCH_JSON=$json
}

json_get() {
  python3 - "$1" "$2" <<'PY'
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    d = {}
v = d.get(sys.argv[2], "")
print("yes" if v is True else "no" if v is False else v)
PY
}

swift_build() {
  local tool=$1
  /bin/mkdir -p "$BUILD"
  /usr/bin/swiftc -O -parse-as-library "$ROOT/scripts/swift/common.swift" "$ROOT/scripts/swift/$tool.swift" \
    -o "$BUILD/${tool//_/-}"
}
