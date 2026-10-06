#!/bin/sh
# Tell IndexNow (Bing, Yandex, Naver and the other participants) about every
# URL in the sitemap. Google does not use IndexNow; submit the sitemap in
# Search Console for Google.
#
#   sh scripts/indexnow-submit.sh --dry-run                 print the request, send nothing
#   sh scripts/indexnow-submit.sh --dry-run _site/sitemap.xml
#   sh scripts/indexnow-submit.sh https://krate.tech/sitemap.xml
#
# The key is the 32-hex-character file at docs/landing/<key>.txt, which the
# pages deploy copies to https://krate.tech/<key>.txt. Run this only after a
# deploy, once that URL returns the key. Expect HTTP 200 or 202.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DRY=0
SRC=https://krate.tech/sitemap.xml
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY=1 ;;
    *) SRC="$arg" ;;
  esac
done

KEYFILE=$(ls "$ROOT"/docs/landing/ | grep -E '^[0-9a-f]{32}\.txt$' | head -1)
if [ -z "$KEYFILE" ]; then
  echo "no IndexNow key file (docs/landing/<32 hex>.txt)" >&2
  exit 1
fi
KEY=${KEYFILE%.txt}
if [ "$(cat "$ROOT/docs/landing/$KEYFILE")" != "$KEY" ]; then
  echo "docs/landing/$KEYFILE must contain exactly its own name" >&2
  exit 1
fi

case "$SRC" in
  http*) SITEMAP=$(curl -fsSL "$SRC") ;;
  *) SITEMAP=$(cat "$SRC") ;;
esac

PAYLOAD=$(printf '%s' "$SITEMAP" | KEY="$KEY" python3 -c '
import json, os, re, sys
urls = re.findall(r"<loc>(https://krate\.tech/[^<]*)</loc>", sys.stdin.read())
if not urls:
    sys.exit("the sitemap lists no krate.tech URLs")
key = os.environ["KEY"]
print(json.dumps({"host": "krate.tech", "key": key,
                  "keyLocation": f"https://krate.tech/{key}.txt", "urlList": urls}, indent=1))
')

if [ "$DRY" = 1 ]; then
  printf '%s\n' "$PAYLOAD"
  echo "dry run: nothing sent ($(printf '%s' "$PAYLOAD" | grep -c '"https://krate.tech/') URLs)" >&2
  exit 0
fi

curl -sS -o /dev/null -w 'IndexNow answered HTTP %{http_code}\n' -X POST https://api.indexnow.org/indexnow \
  -H 'Content-Type: application/json; charset=utf-8' --data "$PAYLOAD"
