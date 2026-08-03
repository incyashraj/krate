#!/usr/bin/env sh
# Shoot every built GUI app to a PNG and flag any that come up blank.
#
# This is the pixel-verification harness the probe loop runs on: an app that
# exits 0 can still show a blank or half-built window, and only looking at the
# frame catches it. A blank frame is a near-empty PNG, so a byte-size floor
# separates "drew something" from "drew nothing" without decoding the image.
#
# Usage: scripts/shoot-all-apps.sh [OUT_DIR]
#   OUT_DIR defaults to ./app-shots. Build the CLI and the apps first.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
OUT="${1:-$ROOT/app-shots}"
KRATE="$ROOT/target/debug/krate"
mkdir -p "$OUT"

if [ ! -x "$KRATE" ]; then
  echo "build the CLI first: cargo build -p krate-cli" >&2
  exit 1
fi

# Below this many bytes a 2x-scaled PNG is almost certainly a flat blank frame.
BLANK_FLOOR=1800

# app:needs-manifest. Most apps run fine manifestless in quick mode; only the
# ones whose quick path truly needs a capability (persistence) get their
# manifest passed. An app manifest can carry a stale entry path that a raw
# --shoot would be denied on, so this list is deliberate, not automatic.
APPS="
checklist:no
notes:no
cubes:no
bounce:no
chart:no
calc:no
bigscroll:no
settings:no
filetree:no
convert:no
keyvault:yes
"

fails=0
for entry in $APPS; do
  app="${entry%%:*}"
  needs_manifest="${entry##*:}"
  wasm="$ROOT/apps/krate-$app/target/wasm32-wasip1/release/krate_$app.wasm"
  if [ ! -f "$wasm" ]; then
    echo "SKIP $app (not built)"
    continue
  fi
  shot="$OUT/$app.png"
  manifest_flag=""
  if [ "$needs_manifest" = "yes" ] && [ -f "$ROOT/apps/krate-$app/manifest.toml" ]; then
    manifest_flag="--manifest $ROOT/apps/krate-$app/manifest.toml"
  fi
  # shellcheck disable=SC2086
  "$KRATE" run "$wasm" $manifest_flag --shoot "$shot" --auto-grant -- quick >/dev/null 2>&1 || true
  if [ ! -f "$shot" ]; then
    echo "FAIL $app (no shot produced)"
    fails=$((fails + 1))
    continue
  fi
  size=$(wc -c < "$shot" | tr -d ' ')
  if [ "$size" -lt "$BLANK_FLOOR" ]; then
    echo "BLANK $app ($size bytes) -- likely an empty frame, look at $shot"
    fails=$((fails + 1))
  else
    echo "OK   $app ($size bytes)"
  fi
done

echo "---"
if [ "$fails" -eq 0 ]; then
  echo "all apps drew a frame. shots in $OUT"
else
  echo "$fails app(s) came up blank or missing -- open the shots and look"
  exit 1
fi
