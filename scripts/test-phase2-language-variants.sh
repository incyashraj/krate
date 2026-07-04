#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
MODE="${KRATE_LANGUAGE_VARIANTS_MODE:-optional}"

set_if_exists() {
  key="$1"
  path="$2"
  eval "current=\${$key:-}"
  if [ -n "$current" ]; then
    return
  fi
  if [ -f "$ROOT/$path" ]; then
    eval "$key=\$ROOT/$path"
    export "$key"
  fi
}

set_if_exists "KRATE_GO_CLOCK_WASM" "test/integration/language-variants/krate_go_clock.wasm"
set_if_exists "KRATE_GO_CAT_WASM" "test/integration/language-variants/krate_go_cat.wasm"
set_if_exists "KRATE_GO_CURL_WASM" "test/integration/language-variants/krate_go_curl.wasm"
set_if_exists "KRATE_TS_CLOCK_WASM" "test/integration/language-variants/krate_ts_clock.wasm"
set_if_exists "KRATE_TS_CAT_WASM" "test/integration/language-variants/krate_ts_cat.wasm"
set_if_exists "KRATE_TS_CURL_WASM" "test/integration/language-variants/krate_ts_curl.wasm"

require_existing_path_for_var() {
  key="$1"
  eval "value=\${$key:-}"
  if [ -z "$value" ]; then
    return
  fi
  if [ ! -f "$value" ]; then
    echo "Phase 2 language-variant setup error: $key points to a missing file: $value" >&2
    exit 1
  fi
}

is_set() {
  key="$1"
  eval "value=\${$key:-}"
  [ -n "$value" ]
}

count_set_vars() {
  count=0
  for key in "$@"; do
    if is_set "$key"; then
      count=$((count + 1))
    fi
  done
  printf '%s' "$count"
}

require_all_or_none() {
  language="$1"
  count="$2"
  total="$3"
  shift 3
  if [ "$count" -eq 0 ] || [ "$count" -eq "$total" ]; then
    return
  fi

  echo "Phase 2 language-variant setup error: $language fixtures are partial ($count/$total)." >&2
  echo "Set all or none of these variables:" >&2
  for key in "$@"; do
    echo "  - $key" >&2
  done
  exit 1
}

for key in \
  KRATE_GO_CLOCK_WASM \
  KRATE_GO_CAT_WASM \
  KRATE_GO_CURL_WASM \
  KRATE_TS_CLOCK_WASM \
  KRATE_TS_CAT_WASM \
  KRATE_TS_CURL_WASM
do
  require_existing_path_for_var "$key"
done

go_count="$(count_set_vars \
  KRATE_GO_CLOCK_WASM \
  KRATE_GO_CAT_WASM \
  KRATE_GO_CURL_WASM)"
ts_count="$(count_set_vars \
  KRATE_TS_CLOCK_WASM \
  KRATE_TS_CAT_WASM \
  KRATE_TS_CURL_WASM)"

require_all_or_none "Go" "$go_count" 3 \
  KRATE_GO_CLOCK_WASM \
  KRATE_GO_CAT_WASM \
  KRATE_GO_CURL_WASM
require_all_or_none "TypeScript" "$ts_count" 3 \
  KRATE_TS_CLOCK_WASM \
  KRATE_TS_CAT_WASM \
  KRATE_TS_CURL_WASM

case "$MODE" in
  optional|any|both|go|ts)
    ;;
  *)
    echo "Phase 2 language-variant setup error: unknown KRATE_LANGUAGE_VARIANTS_MODE='$MODE'." >&2
    echo "Allowed values: optional, any, both, go, ts" >&2
    exit 1
    ;;
esac

case "$MODE" in
  any)
    if [ "$go_count" -eq 0 ] && [ "$ts_count" -eq 0 ]; then
      echo "Phase 2 language-variant setup error: mode '$MODE' requires at least one complete language fixture set." >&2
      exit 1
    fi
    ;;
  both)
    if [ "$go_count" -ne 3 ] || [ "$ts_count" -ne 3 ]; then
      echo "Phase 2 language-variant setup error: mode '$MODE' requires complete Go and TypeScript fixture sets." >&2
      exit 1
    fi
    ;;
  go)
    if [ "$go_count" -ne 3 ]; then
      echo "Phase 2 language-variant setup error: mode '$MODE' requires a complete Go fixture set." >&2
      exit 1
    fi
    ;;
  ts)
    if [ "$ts_count" -ne 3 ]; then
      echo "Phase 2 language-variant setup error: mode '$MODE' requires a complete TypeScript fixture set." >&2
      exit 1
    fi
    ;;
esac

if [ "$MODE" = "optional" ] && [ "$go_count" -eq 0 ] && [ "$ts_count" -eq 0 ]; then
  echo "Skipping Phase 2 language-variant runtime tests (no KRATE_GO_* or KRATE_TS_* vars set, and no test/integration/language-variants/*.wasm fixtures found)."
  exit 0
fi

echo "Running Phase 2 language-variant runtime tests (mode: $MODE)"
cd "$ROOT"

if [ "$go_count" -eq 3 ]; then
  echo "Checking Go language-variant component imports"
  scripts/check-component-imports.sh \
    "$KRATE_GO_CLOCK_WASM" \
    "$KRATE_GO_CAT_WASM" \
    "$KRATE_GO_CURL_WASM"

  echo "Running Go language-variant runtime tests"
  cargo test -p krate-cli --test cli configured_krate_go_
fi

if [ "$ts_count" -eq 3 ]; then
  echo "Checking TypeScript language-variant component imports"
  scripts/check-component-imports.sh \
    "$KRATE_TS_CLOCK_WASM" \
    "$KRATE_TS_CAT_WASM" \
    "$KRATE_TS_CURL_WASM"

  echo "Running TypeScript language-variant runtime tests"
  cargo test -p krate-cli --test cli configured_krate_ts_
fi

if [ "$go_count" -eq 3 ] && [ "$ts_count" -eq 3 ]; then
  echo "Running cross-language parity tests for Rust, Go, and TypeScript fixtures"
  cargo test -p krate-cli --test cli language_variants_
fi
