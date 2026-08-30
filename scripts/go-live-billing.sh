#!/bin/bash
# Turn Krate billing live from a key file. Everything Stripe-side is
# created via API: the product, the three prices, the webhook endpoint
# (whose signing secret comes back in the response). Worker secrets are
# set via wrangler. Nothing is printed except ids and PASS/FAIL.
#
# Usage:
#   1. Put keys in ~/krate-keys.env (chmod 600):
#        STRIPE_SECRET_KEY=sk_test_...   (or sk_live_...)
#        RESEND_API_KEY=re_...           (optional: email login)
#        GOOGLE_CLIENT_ID=...            (optional: google login)
#        GOOGLE_CLIENT_SECRET=...
#   2. scripts/go-live-billing.sh
#
# Safe to re-run: existing product/prices/webhook are found, not duplicated.
set -euo pipefail
ENVFILE="${1:-$HOME/krate-keys.env}"
[ -f "$ENVFILE" ] || { echo "no key file at $ENVFILE"; exit 1; }
set -a; . "$ENVFILE"; set +a
: "${STRIPE_SECRET_KEY:?STRIPE_SECRET_KEY missing from $ENVFILE}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKER="$ROOT/cloud/worker"
api() { curl -sS -u "$STRIPE_SECRET_KEY:" "https://api.stripe.com/v1/$1" "${@:2}"; }
val() { python3 -c "import json,sys; d=json.load(sys.stdin); print(d$1)"; }

echo "== Stripe: product"
PRODUCT=$(api "products/search" --data-urlencode 'query=name:"Krate Studio"' -G | val "['data'][0]['id'] if d['data'] else ''")
if [ -z "$PRODUCT" ]; then
  PRODUCT=$(api products -d name="Krate Studio" -d description="Make an app. Send the file." | val "['id']")
fi
echo "   $PRODUCT"

find_price() { # amount interval nickname
  api "prices/search" --data-urlencode "query=product:\"$PRODUCT\" AND active:\"true\"" -G \
    | python3 -c "
import json,sys
d=json.load(sys.stdin)
for p in d['data']:
    if p['unit_amount']==$1 and p['recurring'] and p['recurring']['interval']=='$2':
        print(p['id']); break"
}
make_price() { # amount interval nickname
  api prices -d product="$PRODUCT" -d currency=usd -d unit_amount="$1" \
    -d "recurring[interval]=$2" -d nickname="$3" | val "['id']"
}
echo "== Stripe: prices"
P_MONTHLY=$(find_price 1200 month); [ -n "$P_MONTHLY" ] || P_MONTHLY=$(make_price 1200 month "Studio monthly")
P_YEARLY=$(find_price 9600 year); [ -n "$P_YEARLY" ] || P_YEARLY=$(make_price 9600 year "Studio yearly")
P_FOUNDING=$(find_price 7900 year); [ -n "$P_FOUNDING" ] || P_FOUNDING=$(make_price 7900 year "Founding 200")
echo "   monthly  $P_MONTHLY"
echo "   yearly   $P_YEARLY"
echo "   founding $P_FOUNDING"

echo "== Stripe: webhook endpoint"
HOOK_URL="https://hub.krate.tech/billing/webhook"
EXISTING=$(api webhook_endpoints -G | python3 -c "
import json,sys
d=json.load(sys.stdin)
for w in d['data']:
    if w['url']=='$HOOK_URL' and w['status']=='enabled': print(w['id']); break")
if [ -n "$EXISTING" ]; then
  echo "   exists: $EXISTING (its secret is only shown at creation;"
  echo "   if the worker's STRIPE_WEBHOOK_SECRET is unset, delete the"
  echo "   endpoint in the dashboard and re-run this script)"
  WEBHOOK_SECRET=""
else
  CREATED=$(api webhook_endpoints \
    -d url="$HOOK_URL" \
    -d "enabled_events[]=checkout.session.completed" \
    -d "enabled_events[]=customer.subscription.created" \
    -d "enabled_events[]=customer.subscription.updated" \
    -d "enabled_events[]=customer.subscription.deleted" \
    -d "enabled_events[]=invoice.payment_succeeded")
  WEBHOOK_SECRET=$(echo "$CREATED" | val "['secret']")
  echo "   created: $(echo "$CREATED" | val "['id']")"
fi

echo "== Worker secrets"
cd "$WORKER"
printf '%s' "$STRIPE_SECRET_KEY"   | npx wrangler secret put STRIPE_SECRET_KEY   >/dev/null
printf '%s' "$P_MONTHLY"           | npx wrangler secret put STRIPE_PRICE_MONTHLY >/dev/null
printf '%s' "$P_YEARLY"            | npx wrangler secret put STRIPE_PRICE_YEARLY  >/dev/null
printf '%s' "$P_FOUNDING"          | npx wrangler secret put STRIPE_PRICE_FOUNDING >/dev/null
[ -n "${WEBHOOK_SECRET:-}" ] && printf '%s' "$WEBHOOK_SECRET" | npx wrangler secret put STRIPE_WEBHOOK_SECRET >/dev/null
[ -n "${RESEND_API_KEY:-}" ] && printf '%s' "$RESEND_API_KEY" | npx wrangler secret put RESEND_API_KEY >/dev/null
[ -n "${GOOGLE_CLIENT_ID:-}" ] && printf '%s' "$GOOGLE_CLIENT_ID" | npx wrangler secret put GOOGLE_CLIENT_ID >/dev/null
[ -n "${GOOGLE_CLIENT_SECRET:-}" ] && printf '%s' "$GOOGLE_CLIENT_SECRET" | npx wrangler secret put GOOGLE_CLIENT_SECRET >/dev/null
echo "   set"

echo "== Verify"
sleep 3
curl -s https://hub.krate.tech/billing/config
echo
echo "Done. If live:true above, the paywall is armed and the pricing cards are doors."
echo "Test-mode card for the whole loop: 4242 4242 4242 4242, any future date, any CVC."
