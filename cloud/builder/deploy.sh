#!/usr/bin/env bash
#
# Put the build box live on Fly.
#
# Run it as often as you like. Creating the app and the volume only happens
# when they are missing, so this is the first-time setup and the redeploy.
#
# It deliberately does NOT set an AI key. A box with no key is live, healthy
# and honest -- it answers /health and refuses builds with a sentence that
# says it is switched off. Switching it on is one command, printed at the
# end, and is the moment the spending starts.

set -euo pipefail

cd "$(dirname "$0")"

APP="krate-builder"
VOLUME="krate_build_cache"
REGION="${FLY_REGION:-iad}"

if ! command -v flyctl >/dev/null 2>&1; then
  cat >&2 <<'MSG'
error: flyctl is not installed.

    brew install flyctl

then sign in with:

    flyctl auth login
MSG
  exit 1
fi

if ! flyctl auth whoami >/dev/null 2>&1; then
  echo "error: not signed in to Fly. Run: flyctl auth login" >&2
  exit 1
fi

echo "==> the app"
if flyctl apps list 2>/dev/null | grep -qE "^${APP}[[:space:]]"; then
  echo "    ${APP} is already there"
else
  flyctl apps create "$APP" --yes
fi

echo "==> the build cache volume"
# The volume is what makes a second build faster than a first: the cargo
# registry and the compiled dependency cache live on it and survive the
# machine stopping. Without it every wake re-downloads crates.io.
if flyctl volumes list --app "$APP" 2>/dev/null | grep -q "$VOLUME"; then
  echo "    ${VOLUME} is already there"
else
  flyctl volumes create "$VOLUME" --app "$APP" --region "$REGION" --size 20 --yes
fi

echo "==> deploying"
# Remote build: the image needs a ~2 GB Rust toolchain and this laptop does
# not need to be the machine that assembles it, or to have Docker running.
flyctl deploy --app "$APP" --remote-only

echo "==> checking it answers"
# Fly's own hostname first. The custom domain is a separate step below and
# may not exist yet on a first run, so checking it here would fail for a
# reason that is not a fault.
curl -fsS --max-time 30 "https://${APP}.fly.dev/health" && echo

cat <<MSG

Live at https://${APP}.fly.dev

Two things left:

1. Point build.krate.tech at it. Fly issues the certificate:

     flyctl certs add build.krate.tech --app ${APP}

   then add the CNAME it prints, at Cloudflare, DNS only (grey cloud).
   Proxied (orange) breaks it: Cloudflare's 100-second edge timeout would
   cut every build's first poll, and Fly cannot see the real client.

2. When you want browser builds switched ON -- this is the moment it
   starts costing money, one build per account:

     flyctl secrets set ANTHROPIC_API_KEY=sk-... --app ${APP}

   Check which state it is in any time:

     curl https://build.krate.tech/health
     # "authoring":"off"  -> live, refusing builds, spending nothing
     # "authoring":"on"   -> making apps, spending on each one
MSG
