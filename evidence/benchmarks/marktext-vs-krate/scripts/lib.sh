#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
CONFIG=${BENCH_CONFIG:-"$ROOT/config.env"}

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

load_config() {
  [[ -f "$CONFIG" ]] || die "copy $ROOT/config.example.env to $CONFIG and set the paths"
  # This is a local, user-authored shell configuration file.
  # shellcheck disable=SC1090
  source "$CONFIG"
  : "${MARKTEXT_APP:?MARKTEXT_APP is required}"
  : "${KRATE_BUNDLE:?KRATE_BUNDLE is required}"
  : "${KRATE_BIN:?KRATE_BIN is required}"
  : "${BENCH_EXPECTED_KRATE_COMMIT:?BENCH_EXPECTED_KRATE_COMMIT is required}"
  : "${BENCH_START_RUNS:=10}"
  : "${BENCH_SETTLE_SECONDS:=30}"
  : "${BENCH_CPU_SAMPLES:=12}"
  : "${BENCH_CPU_INTERVAL_SECONDS:=5}"
  : "${BENCH_SCROLL_SECONDS:=20}"
  : "${BENCH_SCROLL_HZ:=125}"
  : "${BENCH_START_MODE:=warm}"
}

sha256_file() {
  /usr/bin/shasum -a 256 "$1" | /usr/bin/awk '{print $1}'
}

file_bytes() {
  /usr/bin/stat -f '%z' "$1"
}

app_kib() {
  /usr/bin/du -sk "$1" | /usr/bin/awk '{print $1}'
}

canonical_path() {
  python3 - "$1" <<'PY'
from pathlib import Path
import sys
print(Path(sys.argv[1]).expanduser().resolve())
PY
}

process_tree() {
  local root=$1
  local -a queue=("$root")
  local -a seen=()
  local pid child known
  while ((${#queue[@]})); do
    pid=${queue[0]}
    queue=("${queue[@]:1}")
    known=0
    for child in "${seen[@]:-}"; do
      [[ "$child" == "$pid" ]] && known=1 && break
    done
    ((known)) && continue
    /bin/kill -0 "$pid" 2>/dev/null || continue
    seen+=("$pid")
    while IFS= read -r child; do
      [[ -n "$child" ]] && queue+=("$child")
    done < <(/usr/bin/pgrep -P "$pid" 2>/dev/null || true)
  done
  printf '%s\n' "${seen[@]}"
}

kill_tree() {
  local root=$1
  local -a pids=()
  while IFS= read -r pid; do
    [[ -n "$pid" ]] && pids+=("$pid")
  done < <(process_tree "$root")
  if ((${#pids[@]})); then
    /bin/kill -TERM "${pids[@]}" 2>/dev/null || true
    /bin/sleep 1
    /bin/kill -KILL "${pids[@]}" 2>/dev/null || true
  fi
}

instant_cpu_tree() {
  local root=$1
  local -a pids=()
  local -a args=()
  local pid
  while IFS= read -r pid; do
    [[ -n "$pid" ]] && pids+=("$pid")
  done < <(process_tree "$root")
  ((${#pids[@]})) || { printf '0.000000\n'; return; }
  for pid in "${pids[@]}"; do args+=(-pid "$pid"); done
  /usr/bin/top -l 2 -s 1 "${args[@]}" -stats pid,cpu -o pid | /usr/bin/awk '
    /^PID[[:space:]]+%CPU/ { block += 1; next }
    block == 2 && $1 ~ /^[0-9]+$/ { sum += $2 }
    END { printf "%.6f\n", sum + 0 }
  '
}

ts_utc() {
  /bin/date -u '+%Y-%m-%dT%H:%M:%SZ'
}

ensure_header() {
  local file=$1
  local header=$2
  if [[ ! -s "$file" ]]; then
    printf '%b\n' "$header" >"$file"
  fi
}

seed_krate_store() {
  local home=$1
  local fixture=$2
  local store="$home/.krate/store/dev.krate.markdown_editor.kv"
  /bin/mkdir -p "$(dirname "$store")"
  python3 - "$fixture" "$store" <<'PY'
import base64
from pathlib import Path
import sys
fixture, store = map(Path, sys.argv[1:])
encoded = base64.b64encode(fixture.read_bytes())
store.write_bytes(b"document\t" + encoded + b"\n")
PY
}

marktext_binary() {
  printf '%s/Contents/MacOS/MarkText\n' "$MARKTEXT_APP"
}

swift_build() {
  local source=$1
  local output=$2
  /bin/mkdir -p "$(dirname "$output")"
  /usr/bin/swiftc -O "$source" -o "$output"
}
