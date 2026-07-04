#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
MODE="${KRATE_GO_VARIANT_SMOKE_MODE:-optional}"
OUT_DIR="${KRATE_GO_VARIANT_SMOKE_OUT_DIR:-$ROOT/test/integration/language-variants-go-smoke}"
if [ -d "$HOME/.cargo/bin" ]; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi

case "$MODE" in
  optional|required)
    ;;
  *)
    echo "Go variant smoke error: unknown KRATE_GO_VARIANT_SMOKE_MODE='$MODE'." >&2
    echo "Allowed values: optional, required" >&2
    exit 1
    ;;
esac

missing=""
for tool in go tinygo wasm-tools; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    missing="$missing $tool"
  fi
done

if [ -n "$missing" ]; then
  if [ "$MODE" = "optional" ]; then
    echo "Skipping Go variant smoke build: missing toolchain pieces:$missing"
    exit 0
  fi
  echo "Go variant smoke error: required toolchain pieces are missing:$missing" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"

build_one() {
  name="$1"
  src="$2"
  out="$OUT_DIR/${name}.wasm"
  tinygo build -target wasip2 -o "$out" "$src"
  if [ ! -f "$out" ]; then
    echo "Go variant smoke error: TinyGo did not produce $out" >&2
    exit 1
  fi

  if ! wasm-tools component wit "$out" | grep -q "export wasi:cli/run@0.2.0;"; then
    echo "Go variant smoke error: component shape check failed for $out (missing wasi:cli/run export)." >&2
    exit 1
  fi
}

cd "$ROOT/packages/sdk-go"
build_one "krate_go_clock_wasip2" "./examples/krate-clock"
build_one "krate_go_cat_wasip2" "./examples/krate-cat"
build_one "krate_go_curl_wasip2" "./examples/krate-curl"

echo "Built Go variant smoke components:"
echo "  $OUT_DIR/krate_go_clock_wasip2.wasm"
echo "  $OUT_DIR/krate_go_cat_wasip2.wasm"
echo "  $OUT_DIR/krate_go_curl_wasip2.wasm"
