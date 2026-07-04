#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
MODE="${KRATE_GO_RUNTIME_FIXTURE_MODE:-optional}"
SMOKE_DIR="$ROOT/test/integration/language-variants-go-smoke"
RUNTIME_DIR="$ROOT/test/integration/language-variants"

case "$MODE" in
  optional|required)
    ;;
  *)
    echo "Go runtime fixture promotion error: unknown KRATE_GO_RUNTIME_FIXTURE_MODE='$MODE'." >&2
    echo "Allowed values: optional, required" >&2
    exit 1
    ;;
esac

if [ "$MODE" = "required" ]; then
  export KRATE_GO_VARIANT_SMOKE_MODE="required"
else
  export KRATE_GO_VARIANT_SMOKE_MODE="optional"
fi
scripts/build-phase2-go-variant-smoke.sh

clock_src="$SMOKE_DIR/krate_go_clock_wasip2.wasm"
cat_src="$SMOKE_DIR/krate_go_cat_wasip2.wasm"
curl_src="$SMOKE_DIR/krate_go_curl_wasip2.wasm"

if [ ! -f "$clock_src" ] || [ ! -f "$cat_src" ] || [ ! -f "$curl_src" ]; then
  if [ "$MODE" = "optional" ]; then
    echo "Skipping Go runtime fixture promotion: TinyGo smoke artifacts are not available."
    exit 0
  fi
  echo "Go runtime fixture promotion error: required TinyGo smoke artifacts are missing." >&2
  exit 1
fi

if scripts/check-component-imports.sh "$clock_src" "$cat_src" "$curl_src"; then
  mkdir -p "$RUNTIME_DIR"
  cp "$clock_src" "$RUNTIME_DIR/krate_go_clock.wasm"
  cp "$cat_src" "$RUNTIME_DIR/krate_go_cat.wasm"
  cp "$curl_src" "$RUNTIME_DIR/krate_go_curl.wasm"
  echo "Promoted Go runtime fixtures to:"
  echo "  $RUNTIME_DIR/krate_go_clock.wasm"
  echo "  $RUNTIME_DIR/krate_go_cat.wasm"
  echo "  $RUNTIME_DIR/krate_go_curl.wasm"
  exit 0
fi

rm -f \
  "$RUNTIME_DIR/krate_go_clock.wasm" \
  "$RUNTIME_DIR/krate_go_cat.wasm" \
  "$RUNTIME_DIR/krate_go_curl.wasm"

if [ "$MODE" = "optional" ]; then
  echo "Skipping Go runtime fixture promotion: current TinyGo outputs are not Krate import-pure yet."
  exit 0
fi

echo "Go runtime fixture promotion error: TinyGo outputs failed Krate import-purity checks." >&2
exit 1
