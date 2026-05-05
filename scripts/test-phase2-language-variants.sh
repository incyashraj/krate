#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

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

set_if_exists "LAYER36_GO_CLOCK_WASM" "test/integration/language-variants/layer36_go_clock.wasm"
set_if_exists "LAYER36_GO_CAT_WASM" "test/integration/language-variants/layer36_go_cat.wasm"
set_if_exists "LAYER36_GO_CURL_WASM" "test/integration/language-variants/layer36_go_curl.wasm"
set_if_exists "LAYER36_TS_CLOCK_WASM" "test/integration/language-variants/layer36_ts_clock.wasm"
set_if_exists "LAYER36_TS_CAT_WASM" "test/integration/language-variants/layer36_ts_cat.wasm"
set_if_exists "LAYER36_TS_CURL_WASM" "test/integration/language-variants/layer36_ts_curl.wasm"

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
  LAYER36_GO_CLOCK_WASM \
  LAYER36_GO_CAT_WASM \
  LAYER36_GO_CURL_WASM \
  LAYER36_TS_CLOCK_WASM \
  LAYER36_TS_CAT_WASM \
  LAYER36_TS_CURL_WASM
do
  require_existing_path_for_var "$key"
done

go_count="$(count_set_vars \
  LAYER36_GO_CLOCK_WASM \
  LAYER36_GO_CAT_WASM \
  LAYER36_GO_CURL_WASM)"
ts_count="$(count_set_vars \
  LAYER36_TS_CLOCK_WASM \
  LAYER36_TS_CAT_WASM \
  LAYER36_TS_CURL_WASM)"

require_all_or_none "Go" "$go_count" 3 \
  LAYER36_GO_CLOCK_WASM \
  LAYER36_GO_CAT_WASM \
  LAYER36_GO_CURL_WASM
require_all_or_none "TypeScript" "$ts_count" 3 \
  LAYER36_TS_CLOCK_WASM \
  LAYER36_TS_CAT_WASM \
  LAYER36_TS_CURL_WASM

if [ "$go_count" -eq 0 ] && [ "$ts_count" -eq 0 ]; then
  echo "Skipping Phase 2 language-variant runtime tests (no LAYER36_GO_* or LAYER36_TS_* vars set, and no test/integration/language-variants/*.wasm fixtures found)."
  exit 0
fi

echo "Running Phase 2 language-variant runtime tests"
cd "$ROOT"

if [ "$go_count" -eq 3 ]; then
  echo "Running Go language-variant runtime tests"
  cargo test -p layer36-cli --test cli configured_layer36_go_
fi

if [ "$ts_count" -eq 3 ]; then
  echo "Running TypeScript language-variant runtime tests"
  cargo test -p layer36-cli --test cli configured_layer36_ts_
fi
