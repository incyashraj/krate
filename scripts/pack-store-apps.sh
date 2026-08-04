#!/usr/bin/env bash
# Pack the curated, usable apps into shareable .krate bundles for Krate Cloud.
#
# These are the apps a person would actually want to download and run -- not the
# CLI dev tools or the internal limitation probes. Each is built (if needed) and
# packed into evidence/store/<name>.krate, which the Pages build serves at
# /cloud. Run this whenever an app's source changes; commit the resulting
# bundles the same way evidence/ported/*.krate are committed, so Pages does not
# have to compile twelve apps on every deploy.
#
#   scripts/pack-store-apps.sh
#
# Set KRATE_BIN to a prebuilt krate; otherwise this builds the release CLI.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

OUT="evidence/store"
mkdir -p "$OUT"

KRATE="${KRATE_BIN:-target/release/krate}"
if [ ! -x "$KRATE" ]; then
  echo "==> building the release CLI (set KRATE_BIN to skip)"
  cargo build --release -p krate-cli
  KRATE="target/release/krate"
fi

# The rustup toolchain, so cargo-component builds against the wasm target even
# when a Homebrew cargo shadows rustup on PATH (the documented trap).
RUSTUP_BIN="$(rustup which cargo 2>/dev/null | xargs -r dirname || true)"
build_env() {
  if [ -n "$RUSTUP_BIN" ]; then
    env -u RUSTC -u RUSTDOC PATH="$RUSTUP_BIN:$HOME/.cargo/bin:$PATH" "$@"
  else
    "$@"
  fi
}

# The curated set. Excluded on purpose: hello-gui (a demo), spriteproof (a
# pipeline probe), nova2 (its art only exists bundled, packed elsewhere), and
# the CLI dev tools cat/curl/clock/diceroll.
# bounce, chart, and cubes are already served from evidence/ported (they back
# the short krate.tech/cubes.krate URLs), and notes is served from its release
# as krate.tech/notes.krate -- none are repeated here.
APPS=(
  krate-checklist
  krate-paint
  krate-keyvault
  krate-contacts
  krate-clip
  krate-fetch
  krate-nova
  krate-fractal
  krate-savings
  krate-eo2
  krate-mdview
  krate-weather
)

for app in "${APPS[@]}"; do
  dir="apps/$app"
  snake="${app//-/_}"
  wasm="$dir/target/wasm32-wasip1/release/$snake.wasm"
  manifest="$dir/manifest.toml"

  if [ ! -f "$manifest" ]; then
    echo "!! $app: no manifest.toml, skipping" >&2
    continue
  fi

  echo "==> $app"
  ( cd "$dir" && build_env cargo component build --release >/dev/null )

  # Pack with entry rewritten to code.wasm (the name inside a bundle). The
  # source manifest points its entry at the target path for local runs; the
  # bundle always runs code.wasm.
  pack_manifest="$(mktemp)"
  sed 's#^entry = .*#entry = "code.wasm"#' "$manifest" > "$pack_manifest"

  "$KRATE" pack --manifest "$pack_manifest" --output "$OUT/$app.krate" "$wasm"
  rm -f "$pack_manifest"
done

echo "==> packed $(ls "$OUT"/*.krate | wc -l | tr -d ' ') apps into $OUT/"
