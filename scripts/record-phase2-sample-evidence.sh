#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

OUTPUT="${1:-target/phase2-sample-evidence/sample-evidence.md}"

cargo build -p krate-cli

absolute_path() {
  case "$1" in
    /*|[A-Za-z]:/*|[A-Za-z]:\\*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

resolve_krate_binary() {
  if [ -n "${KRATE_BIN:-}" ]; then
    printf '%s\n' "$KRATE_BIN"
    return 0
  fi

  case "$(uname -s 2>/dev/null || printf unknown)" in
    MINGW*|MSYS*|CYGWIN*)
      printf '%s\n' "$ROOT/target/debug/krate.exe"
      return 0
      ;;
  esac

  if [ -f "$ROOT/target/debug/krate" ]; then
    printf '%s\n' "$ROOT/target/debug/krate"
    return 0
  fi

  if [ -f "$ROOT/target/debug/krate.exe" ]; then
    printf '%s\n' "$ROOT/target/debug/krate.exe"
    return 0
  fi

  printf '%s\n' "$ROOT/target/debug/krate"
}

CLOCK_WASM="${KRATE_CLOCK_WASM:-apps/krate-clock/target/wasm32-wasip1/release/krate_clock.wasm}"
CAT_WASM="${KRATE_CAT_WASM:-apps/krate-cat/target/wasm32-wasip1/release/krate_cat.wasm}"
CURL_WASM="${KRATE_CURL_WASM:-apps/krate-curl/target/wasm32-wasip1/release/krate_curl.wasm}"

if [ ! -f "$CLOCK_WASM" ]; then
  CLOCK_WASM="$(scripts/build-krate-clock-component.sh | tail -n 1)"
fi
if [ ! -f "$CAT_WASM" ]; then
  CAT_WASM="$(scripts/build-krate-cat-component.sh | tail -n 1)"
fi
if [ ! -f "$CURL_WASM" ]; then
  CURL_WASM="$(scripts/build-krate-curl-component.sh | tail -n 1)"
fi

CLOCK_WASM="$(absolute_path "$CLOCK_WASM")"
CAT_WASM="$(absolute_path "$CAT_WASM")"
CURL_WASM="$(absolute_path "$CURL_WASM")"
KRATE_BIN="$(absolute_path "$(resolve_krate_binary)")"

cargo run -p krate-tools --bin record-phase2-sample-evidence -- \
  --krate "$KRATE_BIN" \
  --clock "$CLOCK_WASM" \
  --cat "$CAT_WASM" \
  --curl "$CURL_WASM" \
  --output "$OUTPUT"
