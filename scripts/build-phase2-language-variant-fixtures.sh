#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
MODE="${LAYER36_LANGUAGE_VARIANTS_MODE:-optional}"
OUT_DIR="$ROOT/test/integration/language-variants"
SRC_DIR="$ROOT/test/integration/language-variants-src"
TMP_DIR="$OUT_DIR/.tmp-ts-build"

has_file() {
  [ -f "$1" ]
}

has_complete_set() {
  prefix="$1"
  has_file "$OUT_DIR/${prefix}_clock.wasm" \
    && has_file "$OUT_DIR/${prefix}_cat.wasm" \
    && has_file "$OUT_DIR/${prefix}_curl.wasm"
}

remove_set() {
  prefix="$1"
  rm -f \
    "$OUT_DIR/${prefix}_clock.wasm" \
    "$OUT_DIR/${prefix}_cat.wasm" \
    "$OUT_DIR/${prefix}_curl.wasm"
}

set_imports_are_pure() {
  prefix="$1"
  scripts/check-component-imports.sh \
    "$OUT_DIR/${prefix}_clock.wasm" \
    "$OUT_DIR/${prefix}_cat.wasm" \
    "$OUT_DIR/${prefix}_curl.wasm" >/dev/null 2>&1
}

jco_runner() {
  jco_npx_package="${LAYER36_JCO_NPX_PACKAGE:-@bytecodealliance/jco@1.14.0}"

  if command -v jco >/dev/null 2>&1; then
    XDG_CACHE_HOME="$ROOT/.cache" jco "$@"
    return
  fi

  if command -v npx >/dev/null 2>&1; then
    if [ "${LAYER36_ALLOW_NPX_INSTALL:-0}" = "1" ]; then
      XDG_CACHE_HOME="$ROOT/.cache" npx --yes "$jco_npx_package" "$@"
      return
    fi
    XDG_CACHE_HOME="$ROOT/.cache" npx --no-install jco "$@"
    return
  fi

  return 127
}

can_build_ts() {
  command -v node >/dev/null 2>&1 || return 1
  jco_runner --version >/dev/null 2>&1 || return 1
  return 0
}

can_build_go_runtime_fixtures() {
  command -v go >/dev/null 2>&1 || return 1
  command -v tinygo >/dev/null 2>&1 || return 1
  command -v wasm-tools >/dev/null 2>&1 || return 1
  return 0
}

build_ts_fixtures() {
  rm -rf "$TMP_DIR"
  mkdir -p "$TMP_DIR"

  jco_runner componentize \
    "$SRC_DIR/layer36-ts-clock.mjs" \
    --wit "$ROOT/wit/layer36/phase2" \
    --world-name cli \
    --disable all \
    --out "$TMP_DIR/layer36_ts_clock.wasm"

  jco_runner componentize \
    "$SRC_DIR/layer36-ts-cat.mjs" \
    --wit "$ROOT/wit/layer36/phase2" \
    --world-name cli \
    --disable all \
    --out "$TMP_DIR/layer36_ts_cat.wasm"

  jco_runner componentize \
    "$SRC_DIR/layer36-ts-curl.mjs" \
    --wit "$ROOT/wit/layer36/phase2" \
    --world-name cli \
    --disable all \
    --out "$TMP_DIR/layer36_ts_curl.wasm"

  for file in \
    "$TMP_DIR/layer36_ts_clock.wasm" \
    "$TMP_DIR/layer36_ts_cat.wasm" \
    "$TMP_DIR/layer36_ts_curl.wasm"
  do
    if [ ! -f "$file" ]; then
      echo "TypeScript language-variant fixture build failed: expected output was not created: $file" >&2
      rm -rf "$TMP_DIR"
      return 1
    fi
  done

  if scripts/check-component-imports.sh \
    "$TMP_DIR/layer36_ts_clock.wasm" \
    "$TMP_DIR/layer36_ts_cat.wasm" \
    "$TMP_DIR/layer36_ts_curl.wasm"
  then
    mv "$TMP_DIR/layer36_ts_clock.wasm" "$OUT_DIR/layer36_ts_clock.wasm"
    mv "$TMP_DIR/layer36_ts_cat.wasm" "$OUT_DIR/layer36_ts_cat.wasm"
    mv "$TMP_DIR/layer36_ts_curl.wasm" "$OUT_DIR/layer36_ts_curl.wasm"
    rm -rf "$TMP_DIR"
    return 0
  fi

  echo "TypeScript language-variant fixtures failed Layer36 import purity checks and were not activated." >&2
  rm -rf "$TMP_DIR"
  return 1
}

case "$MODE" in
  optional|any|both|go|ts)
    ;;
  *)
    echo "Phase 2 language-variant build error: unknown LAYER36_LANGUAGE_VARIANTS_MODE='$MODE'." >&2
    echo "Allowed values: optional, any, both, go, ts" >&2
    exit 1
    ;;
esac

mkdir -p "$OUT_DIR"

ts_ready=0
go_ready=0
ts_reason=""
go_reason=""

if has_complete_set "layer36_ts"; then
  if set_imports_are_pure "layer36_ts"; then
    ts_ready=1
  else
    echo "Existing TypeScript fixtures failed Layer36 import purity checks; removing stale files."
    remove_set "layer36_ts"
  fi
fi
if has_complete_set "layer36_go"; then
  if set_imports_are_pure "layer36_go"; then
    go_ready=1
  else
    echo "Existing Go fixtures failed Layer36 import purity checks; removing stale files."
    remove_set "layer36_go"
  fi
fi

if [ "$ts_ready" -eq 0 ]; then
  if can_build_ts; then
    echo "Building TypeScript language-variant fixtures with jco"
    if build_ts_fixtures; then
      ts_ready=1
      ts_reason="fixtures built and import-pure"
    else
      ts_ready=0
      ts_reason="fixture build or import-purity check failed"
    fi
  else
    echo "TypeScript language-variant fixtures not built: jco path is unavailable."
    ts_reason="jco build path unavailable"
  fi
else
  ts_reason="existing fixtures are import-pure"
fi

if [ "$go_ready" -eq 0 ]; then
  go_fixture_mode="optional"
  case "$MODE" in
    go|both)
      go_fixture_mode="required"
      ;;
  esac

  if can_build_go_runtime_fixtures; then
    echo "Building and promoting Go language-variant runtime fixtures with TinyGo"
    if LAYER36_GO_RUNTIME_FIXTURE_MODE="$go_fixture_mode" scripts/promote-phase2-go-runtime-fixtures.sh; then
      if has_complete_set "layer36_go" && set_imports_are_pure "layer36_go"; then
        go_ready=1
        go_reason="fixtures built/promoted and import-pure"
      else
        go_reason="promotion completed but fixture set was incomplete or not import-pure"
      fi
    else
      go_reason="promotion step failed"
    fi
  else
    echo "Go language-variant runtime fixtures not built: TinyGo toolchain path is unavailable (mode: $go_fixture_mode)."
    go_reason="TinyGo toolchain path unavailable"
  fi
else
  go_reason="existing fixtures are import-pure"
fi

case "$MODE" in
  optional)
    ;;
  any)
    if [ "$go_ready" -eq 0 ] && [ "$ts_ready" -eq 0 ]; then
      echo "Phase 2 language-variant build error: mode '$MODE' requires at least one complete language fixture set." >&2
      exit 1
    fi
    ;;
  both)
    if [ "$go_ready" -eq 0 ] || [ "$ts_ready" -eq 0 ]; then
      echo "Phase 2 language-variant build error: mode '$MODE' requires complete Go and TypeScript fixture sets." >&2
      exit 1
    fi
    ;;
  go)
    if [ "$go_ready" -eq 0 ]; then
      echo "Phase 2 language-variant build error: mode '$MODE' requires a complete Go fixture set. Reason: ${go_reason:-unknown}." >&2
      exit 1
    fi
    ;;
  ts)
    if [ "$ts_ready" -eq 0 ]; then
      echo "Phase 2 language-variant build error: mode '$MODE' requires a complete TypeScript fixture set. Reason: ${ts_reason:-unknown}." >&2
      exit 1
    fi
    ;;
esac

echo "Language fixture availability after build step:"
echo "  Go: ${go_ready}"
echo "  TypeScript: ${ts_ready}"
echo "  Go reason: ${go_reason:-unknown}"
echo "  TypeScript reason: ${ts_reason:-unknown}"
