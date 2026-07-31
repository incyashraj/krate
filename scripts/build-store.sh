#!/usr/bin/env sh
# Build the Krate store from the .krate files themselves.
#
# Every listing -- name, version, size, identity, and the exact list of what the
# app asks for -- is read out of the bundle by `krate run --dump-caps`. Nothing
# is typed by hand, so a listing cannot drift from the file it describes, and
# nobody has to be trusted to describe their own app honestly.
#
#   sh scripts/build-store.sh <output-dir> <app.krate> [more.krate...]
set -eu

out="${1:?usage: build-store.sh <output-dir> <app.krate>...}"
shift

KRATE="${KRATE_BIN:-target/release/krate}"
if [ ! -x "$KRATE" ]; then
  KRATE="$(command -v krate || true)"
fi
if [ -z "$KRATE" ] || [ ! -x "$KRATE" ]; then
  echo "no krate binary; build it or set KRATE_BIN" >&2
  exit 1
fi

here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

mkdir -p "$out/apps"
index="$out/apps/index.json"
printf '[\n' > "$index"

first=1
for bundle in "$@"; do
  if [ ! -f "$bundle" ]; then
    echo "skipping missing $bundle" >&2
    continue
  fi
  name="$(basename "$bundle")"
  cp "$bundle" "$out/apps/$name"

  # The bundle is the source of truth for its own listing. Failures are loud:
  # a store that quietly lists nothing looks like a store with no apps, and
  # that is exactly how an empty shelf shipped once already.
  if ! meta="$("$KRATE" run "$bundle" --dump-caps --dump-caps-format json 2>&1)"; then
    echo "could not read $bundle with $KRATE:" >&2
    printf '%s\n' "$meta" >&2
    exit 1
  fi

  size="$(wc -c < "$bundle" | tr -d ' ')"
  [ "$first" -eq 1 ] || printf ',\n' >> "$index"
  first=0

  printf '%s' "$meta" \
    | KRATE_FILE="$name" KRATE_SIZE="$size" python3 "$here/store-listing.py" >> "$index"
done

printf '\n]\n' >> "$index"

if [ "$first" -eq 1 ]; then
  echo "no apps could be listed; refusing to publish an empty store" >&2
  exit 1
fi

echo "wrote $index"
KRATE_INDEX="$index" python3 - <<'PY'
import json, os
apps = json.load(open(os.environ["KRATE_INDEX"]))
print(f"  {len(apps)} app(s) listed")
for a in apps:
    print(f"   - {a['name']} {a['version']}  {a['bytes']} bytes  {len(a['asks'])} ask(s)")
PY
