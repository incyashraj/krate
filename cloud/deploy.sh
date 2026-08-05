#!/usr/bin/env bash
#
# Put the Krate hub live on Cloudflare.
#
# Run it as often as you like: creating the bucket and the namespace is only
# done when they are missing, so this is a deploy script and a first-time
# setup script at once.
#
# Needs CLOUDFLARE_API_TOKEN in the environment. Nothing here reads it, prints
# it, or writes it anywhere -- wrangler takes it straight from the environment.

set -euo pipefail

cd "$(dirname "$0")/worker"

if [ -z "${CLOUDFLARE_API_TOKEN:-}" ]; then
  cat >&2 <<'MSG'
error: CLOUDFLARE_API_TOKEN is not set.

Make one at dash.cloudflare.com -> My Profile -> API Tokens, using the
"Edit Cloudflare Workers" template, then:

    export CLOUDFLARE_API_TOKEN=your_token_here

and run this again.
MSG
  exit 1
fi

WRANGLER="npx --yes wrangler@3"

echo "==> R2 bucket for the bundles"
# Already-exists is the normal case on every run after the first, so it is not
# an error worth stopping for.
$WRANGLER r2 bucket create krate-bundles 2>/dev/null \
  || echo "    krate-bundles is already there"

echo "==> KV namespace for the metadata"
# Ask for the namespace list first and only create when it is genuinely
# absent. The earlier version created-then-fell-back-to-listing, and on a
# second run wrangler printed no new id, so a loose grep picked the account id
# out of surrounding text and wrote that in as the namespace.
kv_lookup() {
  $WRANGLER kv namespace list 2>/dev/null | python3 -c '
import json, sys
try:
    for ns in json.load(sys.stdin):
        # wrangler titles a namespace "<worker>-<binding>".
        if ns.get("title", "").endswith("APPS"):
            print(ns["id"])
            break
except Exception:
    pass'
}

# A good id already in the config wins. It is the one thing here known to be
# correct, and re-deriving it every run is what went wrong twice.
KV_ID="$(grep -oE '^id = "[0-9a-f]{32}"' wrangler.toml | grep -oE '[0-9a-f]{32}' || true)"
if [ -z "$KV_ID" ]; then
  KV_ID="$(kv_lookup || true)"
fi
if [ -z "$KV_ID" ]; then
  $WRANGLER kv namespace create APPS >/dev/null 2>&1 || true
  KV_ID="$(kv_lookup || true)"
fi

# A namespace id is 32 hex characters, and so is an account id -- so shape
# alone is not enough. Refusing to proceed when it equals the account id is
# what stops the exact mix-up that broke two deploys.
ACCOUNT_ID="$(grep -oE '^account_id = "[0-9a-f]{32}"' wrangler.toml | grep -oE '[0-9a-f]{32}')"
if [ -z "$KV_ID" ] || [ "$KV_ID" = "$ACCOUNT_ID" ]; then
  echo "error: could not find the APPS KV namespace." >&2
  echo "Look it up with: npx wrangler@3 kv namespace list" >&2
  echo "then set it as the id under [[kv_namespaces]] in cloud/worker/wrangler.toml" >&2
  exit 1
fi
echo "    namespace: $KV_ID"

# Write the real id into wrangler.toml so the deploy binds to it.
#
# Anchored to a line that is exactly `id = "..."`. The first version matched
# `id = "..."` anywhere, which hit `account_id` -- the line above it -- and
# replaced the account with the namespace, so the deploy authenticated against
# an account that does not exist.
python3 - "$KV_ID" <<'PY'
import re, sys
kv_id = sys.argv[1]
path = "wrangler.toml"
text = open(path).read()
text = re.sub(r'^id = "[^"]*"$', f'id = "{kv_id}"', text, count=1, flags=re.M)
open(path, "w").write(text)
PY

echo "==> deploying the worker"
$WRANGLER deploy

cat <<'MSG'

Done. Two things left, both in the Cloudflare dashboard:

1. Workers & Pages -> krate-hub -> Settings -> Domains & Routes
   Add a custom domain:  hub.krate.tech

2. Check it:
     curl https://hub.krate.tech/health
   It should print: ok

Then publishing goes live with:
     export KRATE_HUB_URL=https://hub.krate.tech
MSG
