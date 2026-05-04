#!/usr/bin/env sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cd "$ROOT"

CARGO_BIN="${CARGO:-cargo}"
RUSTC_BIN="${RUSTC:-}"
if [ -z "${CARGO:-}" ] && command -v rustup >/dev/null 2>&1; then
  CARGO_BIN="$(rustup which cargo)"
fi
if [ -z "${RUSTC:-}" ] && command -v rustup >/dev/null 2>&1; then
  RUSTC_BIN="$(rustup which rustc)"
fi

run_cargo() {
  if [ -n "$RUSTC_BIN" ]; then
    RUSTC="$RUSTC_BIN" "$CARGO_BIN" "$@"
  else
    "$CARGO_BIN" "$@"
  fi
}

if [ "${LAYER36_OFFLINE:-}" = "1" ]; then
  run_cargo package -p layer36 --offline
else
  run_cargo package -p layer36
fi

package_dir="$(
  find "$ROOT/target/package" -maxdepth 1 -type d -name 'layer36-*' \
    | sort \
    | tail -n 1
)"

if [ -z "$package_dir" ] || [ ! -f "$package_dir/Cargo.toml" ]; then
  echo "could not find packaged layer36 SDK under target/package" >&2
  exit 1
fi

smoke_root="${TMPDIR:-/tmp}/layer36-rust-sdk-smoke-$$"
trap 'rm -rf "$smoke_root"' EXIT INT TERM

mkdir -p "$smoke_root/src"

cat > "$smoke_root/Cargo.toml" <<EOF
[package]
name = "layer36-rust-sdk-smoke"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
layer36 = { path = "$package_dir" }

[lib]
crate-type = ["cdylib"]

[profile.release]
panic = "abort"
EOF

cat > "$smoke_root/src/lib.rs" <<'EOF'
use layer36::{io::stdio, Guest};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        if stdio::println("Layer36 Rust SDK smoke").is_err() {
            return 20;
        }

        0
    }
}

layer36::export!(Component);
EOF

if [ "${LAYER36_OFFLINE:-}" = "1" ]; then
  run_cargo check --manifest-path "$smoke_root/Cargo.toml" --target wasm32-wasip1 --offline
else
  run_cargo check --manifest-path "$smoke_root/Cargo.toml" --target wasm32-wasip1
fi

echo "Layer36 Rust SDK external smoke passed"
