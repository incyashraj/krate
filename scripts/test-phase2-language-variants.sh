#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

has_variant=""
for key in \
  LAYER36_GO_CLOCK_WASM \
  LAYER36_GO_CAT_WASM \
  LAYER36_GO_CURL_WASM \
  LAYER36_TS_CLOCK_WASM \
  LAYER36_TS_CAT_WASM \
  LAYER36_TS_CURL_WASM
do
  eval "value=\${$key:-}"
  if [ -n "$value" ]; then
    has_variant="yes"
    break
  fi
done

if [ -z "$has_variant" ]; then
  echo "Skipping Phase 2 language-variant runtime tests (no LAYER36_GO_* or LAYER36_TS_* WASM paths set)."
  exit 0
fi

echo "Running Phase 2 language-variant runtime tests"
cd "$ROOT"

cargo test -p layer36-cli --test cli configured_layer36_go_
cargo test -p layer36-cli --test cli configured_layer36_ts_
