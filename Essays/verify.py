#!/usr/bin/env python3
"""Prove the honesty guarantee holds.

Checks, in order:
  1. Every essay in the manifest has a text file with real content.
  2. Every indexed chunk exists verbatim in its source file.
  3. Live retrieval returns only verifiable passages.
  4. A deliberately fake quote is correctly rejected.

Exit code 0 means the guarantee holds.
"""

import json
import pickle
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
TEXT = ROOT / "text"
INDEX = ROOT / "index"

PROBES = [
    "how do you get startup ideas?",
    "what makes someone do great work?",
    "why are nerds unpopular?",
    "how should you write clearly?",
    "what can't you say?",
    "should I raise money from investors?",
    "how do you decide what to work on?",
    "what is the danger of caring what other people think?",
]


def main():
    failures = []

    # 1. corpus present
    manifest = json.loads((INDEX / "manifest.json").read_text(encoding="utf-8"))
    missing = [e["slug"] for e in manifest
               if not (TEXT / (e["slug"] + ".txt")).exists()]
    if missing:
        failures.append(f"{len(missing)} essays missing text: {missing[:5]}")
    print(f"1. corpus: {len(manifest) - len(missing)}/{len(manifest)} essays present")

    # 2. every chunk verifiable
    with open(INDEX / "search.pkl", "rb") as f:
        store = pickle.load(f)
    cache, bad = {}, 0
    for c in store["chunks"]:
        src = cache.setdefault(
            c["slug"], (TEXT / (c["slug"] + ".txt")).read_text(encoding="utf-8")
        )
        if c["text"] not in src:
            bad += 1
    if bad:
        failures.append(f"{bad} indexed chunks are not verbatim in source")
    print(f"2. chunks: {len(store['chunks']) - bad}/{len(store['chunks'])} verbatim in source")

    # 3. live retrieval
    from ask import search, verify
    total, unverified, empty = 0, 0, 0
    for q in PROBES:
        hits = search(store, q, n=5)
        if not hits:
            empty += 1
            print(f"   no results: {q!r}")
            continue
        for h in hits:
            total += 1
            if not verify(h):
                unverified += 1
    if unverified:
        failures.append(f"{unverified} retrieved passages failed verification")
    print(f"3. retrieval: {total} passages over {len(PROBES)} questions, "
          f"{unverified} unverified, {empty} questions with no match")

    # 4. a fabricated quote must be rejected
    fake = {"slug": store["chunks"][0]["slug"],
            "text": "Paul Graham never wrote this sentence about zebras."}
    if verify(fake):
        failures.append("verifier accepted a fabricated quote")
        print("4. fabrication check: FAILED -- fake quote accepted")
    else:
        print("4. fabrication check: fake quote correctly rejected")

    print()
    if failures:
        print("FAILED:")
        for f in failures:
            print("  - " + f)
        return 1
    print("All checks passed. Every quote the tool can return is real.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
