#!/usr/bin/env bash
# Build what the hub needs to open Krate apps in a browser tab (IC-956):
#
#   cloud/worker/src/transpiler.wasm   crates/web-transpile (js-component-bindgen
#                                      2.12.0, the engine of jco 1.35.0) for the
#                                      publish door, which makes each release's
#                                      web pack
#   cloud/worker/player-dist/          the Player page the hub serves at
#                                      /run/<hash>: crates/adapter-web compiled to
#                                      wasm32, the validator the hub runs, the
#                                      page's scripts, the site's font and mark
#
#   scripts/build-hub-validator.sh && scripts/build-web-player.sh
#
# Run both before `wrangler deploy` in cloud/worker and before the worker
# tests. Nothing they write is committed.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$(pwd)

if [ -x "$HOME/.cargo/bin/cargo" ]; then
  PATH="$HOME/.cargo/bin:$PATH"
  export PATH
fi

validator=cloud/worker/src/validator.wasm
if [ ! -f "$validator" ]; then
  echo "build-web-player: $validator is missing; run scripts/build-hub-validator.sh first" >&2
  exit 1
fi

# 1. The transpiler, in its own workspace and on its own pinned Rust (its
#    rust-toolchain.toml), so the main lock file never sees wasmtime-environ 49.
(cd crates/web-transpile && cargo build --release --locked --target wasm32-unknown-unknown)
cp crates/web-transpile/target/wasm32-unknown-unknown/release/krate_web_transpile.wasm cloud/worker/src/transpiler.wasm

# 2. The Player module: the desktop's lowering, painter, manifest and policy
#    crates for wasm32, with wasm-bindgen's JavaScript face. The CLI must be
#    the version Cargo.lock pins for the crate.
cargo build --release --locked --target wasm32-unknown-unknown -p krate-adapter-web
want=$(awk '/^name = "wasm-bindgen"$/{getline; gsub(/version = |"/, ""); print; exit}' Cargo.lock)
have=$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')
if [ "$want" != "$have" ]; then
  echo "build-web-player: wasm-bindgen $have is installed, Cargo.lock pins $want:" >&2
  echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
  exit 1
fi
out=cloud/worker/player-dist
rm -rf "$out"
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-name krate_web --out-dir "$out" \
  target/wasm32-unknown-unknown/release/krate_adapter_web.wasm

# 3. The page, the validator the hub itself runs, and the site's own font
#    and mark (a cross-origin-isolated page cannot load krate.tech's).
web=crates/adapter-web/player/web
cp "$web"/run.html "$web"/player.js "$web"/player.css "$web"/app-worker.js \
  "$web"/hosts.js "$web"/bridge.js "$web"/ring.js "$web"/audio.js "$web"/audio-worklet.js \
  "$web"/org-data.js "$web"/krate-card.png "$out"/
# The browser as an organisation's device: the console's own file, so a tab
# that keeps an app's team data registers and holds the key the same way.
cp docs/landing/kit/org-device.js "$out"/
cp "$validator" "$out"/validator.wasm
cp docs/landing/fonts/geist-var-latin.woff2 docs/landing/krate-mark-3d-96.png docs/landing/krate-favicon.png \
  docs/landing/krate-app-icon.png "$out"/

printf 'wrote %s (%s bytes) and %s:\n' cloud/worker/src/transpiler.wasm \
  "$(wc -c < cloud/worker/src/transpiler.wasm | tr -d ' ')" "$out"
ls -l "$out" | awk 'NR>1 {printf "  %-24s %9s bytes\n", $9, $5}'
