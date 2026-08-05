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
KV_OUTPUT="$($WRANGLER kv namespace create APPS 2>&1 || true)"
# wrangler prints the new id in a TOML snippet; an existing namespace has to be
# looked up instead.
KV_ID="$(printf '%s' "$KV_OUTPUT" | grep -oE '[0-9a-f]{32}' | head -1 || true)"
if [ -z "$KV_ID" ]; then
  KV_ID="$($WRANGLER kv namespace list 2>/dev/null \
    | python3 -c 'import json,sys
try:
    for ns in json.load(sys.stdin):
        if ns.get("title","").endswith("APPS"):
            print(ns["id"]); break
except Exception:
    pass' || true)"
fi

if [ -z "$KV_ID" ]; then
  echo "error: could not create or find the APPS namespace." >&2
  echo "$KV_OUTPUT" >&2
  exit 1
fi
echo "    namespace: $KV_ID"

# Write the real id into wrangler.toml so the deploy binds to it.
python3 - "$KV_ID" <<'PY'
import re, sys
kv_id = sys.argv[1]
path = "wrangler.toml"
text = open(path).read()
text = re.sub(r'id = "[^"]*"', f'id = "{kv_id}"', text, count=1)
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
