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
[ -x "$KRATE" ] || KRATE="$(command -v krate || true)"
[ -n "$KRATE" ] && [ -x "$KRATE" ] || {
  echo "no krate binary; build it or set KRATE_BIN" >&2
  exit 1
}

mkdir -p "$out/apps"
index="$out/apps/index.json"
printf '[\n' > "$index"

first=1
for bundle in "$@"; do
  [ -f "$bundle" ] || { echo "skipping missing $bundle" >&2; continue; }
  name="$(basename "$bundle")"
  cp "$bundle" "$out/apps/$name"

  # The bundle is the source of truth for its own listing.
  meta="$("$KRATE" run "$bundle" --dump-caps --dump-caps-format json 2>/dev/null || true)"
  [ -n "$meta" ] || { echo "could not read $bundle" >&2; continue; }

  size="$(wc -c < "$bundle" | tr -d ' ')"
  [ "$first" -eq 1 ] || printf ',\n' >> "$index"
  first=0

  printf '%s' "$meta" | KRATE_FILE="$name" KRATE_SIZE="$size" python3 -c '
import json, os, sys

meta = json.load(sys.stdin)
app = meta.get("app") or {}

# Only the capabilities the app must be granted. The defaults every app gets are
# noise on a listing: someone deciding whether to trust an app cares about what
# is unusual about it, not that it can print to its own output.
default_grants = set(meta.get("capabilities") or [])
asks = [c for c in (meta.get("requested") or []) if c not in default_grants]

print(json.dumps({
    "name": app.get("name") or os.environ["KRATE_FILE"],
    "id": app.get("id"),
    "version": app.get("version"),
    "file": os.environ["KRATE_FILE"],
    "bytes": int(os.environ["KRATE_SIZE"]),
    "digest": meta.get("digest"),
    "asks": asks,
}, indent=2))
' >> "$index"
done

printf '\n]\n' >> "$index"
echo "wrote $index"
python3 -c "
import json
apps = json.load(open('$index'))
print(f'  {len(apps)} app(s) listed')
for a in apps:
    print(f\"   - {a['name']} {a['version']}  {a['bytes']} bytes  {len(a['asks'])} ask(s)\")
"
