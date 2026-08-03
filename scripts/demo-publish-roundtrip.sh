#!/usr/bin/env bash
# Proof that one-click share works end to end:
#   pack/take a .krate -> `krate publish` -> capture the URL -> `krate run <url>`
#
# It starts a local krate-hub, publishes a real bundle, then runs the app back
# from the URL the hub handed out and checks it actually ran. If any step fails
# the script exits non-zero, so this doubles as a smoke test for the loop.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

KRATE_BIN="${KRATE_BIN:-target/debug/krate}"
HUB_BIN="${HUB_BIN:-target/debug/krate-hub}"
BUNDLE="${BUNDLE:-evidence/ported/cubes.krate}"
HUB_ADDR="127.0.0.1:8791"
HUB_URL="http://${HUB_ADDR}"

# Everything the hub stores goes in a temp dir that we delete on exit, so a run
# leaves nothing behind.
HUB_DIR="$(mktemp -d)"
HUB_PID=""

cleanup() {
  if [[ -n "$HUB_PID" ]] && kill -0 "$HUB_PID" 2>/dev/null; then
    kill "$HUB_PID" 2>/dev/null || true
    wait "$HUB_PID" 2>/dev/null || true
  fi
  rm -rf "$HUB_DIR"
}
trap cleanup EXIT

fail() { echo "FAIL: $*" >&2; exit 1; }

# --- preconditions -----------------------------------------------------------
[[ -x "$KRATE_BIN" ]] || fail "krate binary not found at $KRATE_BIN (run: cargo build -p krate-cli)"
[[ -x "$HUB_BIN" ]]   || fail "krate-hub binary not found at $HUB_BIN (run: cargo build -p krate-hub)"
[[ -f "$BUNDLE" ]]    || fail "test bundle not found at $BUNDLE"

echo "== 1. start the hub =========================================="
KRATE_HUB_ADDR="$HUB_ADDR" \
KRATE_HUB_DIR="$HUB_DIR" \
KRATE_HUB_PUBLIC_URL="$HUB_URL" \
  "$HUB_BIN" &
HUB_PID=$!

# Wait for /health to answer rather than sleeping a fixed amount.
for _ in $(seq 1 50); do
  if curl -fsS "${HUB_URL}/health" >/dev/null 2>&1; then break; fi
  sleep 0.1
done
curl -fsS "${HUB_URL}/health" >/dev/null 2>&1 || fail "hub never became healthy"
echo "hub healthy at ${HUB_URL}"
echo

echo "== 2. publish the bundle ====================================="
PUBLISH_OUT="$(KRATE_HUB_URL="$HUB_URL" "$KRATE_BIN" publish "$BUNDLE")"
echo "$PUBLISH_OUT"
echo

# The publish command prints "  krate run <url>"; pull the URL back out of it.
URL="$(printf '%s\n' "$PUBLISH_OUT" | grep -oE 'https?://[^ ]+/a/[0-9a-f]+' | head -n1)"
[[ -n "$URL" ]] || fail "could not find a published URL in the output above"
echo "captured URL: $URL"
echo

echo "== 3. run the app straight from the URL ======================"
# --insecure-http because the local hub speaks plain http; a real hub would be
# https and this flag would not be needed. --headless so a GUI app (cubes)
# renders without trying to open a window in this non-interactive run; the app
# still runs and still prints its verification output.
RUN_OUT="$("$KRATE_BIN" run "$URL" --insecure-http --headless --auto-grant -- quick 2>&1)" || \
  fail "krate run <url> exited non-zero. Output:\n$RUN_OUT"
echo "$RUN_OUT"
echo

# The app has to have actually produced output. cubes prints frames/ascii; any
# non-empty stdout from a URL-fetched run proves the whole loop closed.
[[ -n "${RUN_OUT// /}" ]] || fail "the app produced no output -- did it really run?"

echo "== PASS ======================================================"
echo "Published $BUNDLE and ran it back from $URL"
