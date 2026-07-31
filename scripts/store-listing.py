#!/usr/bin/env python3
"""Turn one `krate run --dump-caps --dump-caps-format json` result into a store listing.

Kept separate from build-store.sh so the shell does not have to quote a Python
program, which is how a working script became a syntax error once already.

Reads the capability dump on stdin. Needs KRATE_FILE and KRATE_SIZE in the
environment, because the file name and its size are facts about the file rather
than about its contents.
"""

import json
import os
import sys


def main() -> int:
    raw = sys.stdin.read()
    name = os.environ.get("KRATE_FILE", "<unknown>")

    try:
        meta = json.loads(raw)
    except json.JSONDecodeError:
        # The binary answered, but not with the JSON this expects -- an older
        # krate, or an error printed on stdout. Say which, rather than raising a
        # stack trace at whoever is building the store.
        sys.stderr.write(f"{name}: krate did not return listing JSON.\n")
        sys.stderr.write(f"got: {raw[:200]}\n")
        return 1

    app = meta.get("app") or {}

    # Only the capabilities the app must be granted. The defaults every app
    # receives are noise on a listing: someone deciding whether to trust an app
    # cares about what is unusual about it, not that it can print its own output.
    default_grants = set(meta.get("capabilities") or [])
    asks = [c for c in (meta.get("requested") or []) if c not in default_grants]

    listing = {
        "name": app.get("name") or name,
        "id": app.get("id"),
        "version": app.get("version"),
        "file": name,
        "bytes": int(os.environ.get("KRATE_SIZE", "0")),
        "digest": meta.get("digest"),
        "asks": asks,
    }

    if not listing["digest"]:
        # A listing without an identity cannot be checked against the file it
        # describes, which is the one thing this store is for.
        sys.stderr.write(
            f"{name}: no content identity in the capability dump. "
            "The krate binary building this store is older than the digest.\n"
        )
        return 1

    json.dump(listing, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
