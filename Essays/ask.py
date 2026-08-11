#!/usr/bin/env python3
"""Ask a question; get back the passages Paul Graham actually wrote about it.

This does NOT generate prose. It retrieves real paragraphs and verifies each
one against its source file before printing it. If a quote is not present
verbatim in the .txt on disk, it is not shown.

Usage:
    python3 ask.py "how do you get startup ideas?"
    python3 ask.py -n 8 "what makes someone do great work?"
"""

import argparse
import pickle
import re
import sys
from pathlib import Path

from sklearn.metrics.pairwise import cosine_similarity

ROOT = Path(__file__).resolve().parent
TEXT = ROOT / "text"
INDEX = ROOT / "index"

# Below this, retrieval is guessing rather than finding.
WEAK = 0.06


def load():
    with open(INDEX / "search.pkl", "rb") as f:
        return pickle.load(f)


def verify(chunk):
    """Re-read the source file and confirm the passage is really there."""
    path = TEXT / (chunk["slug"] + ".txt")
    if not path.exists():
        return False
    return chunk["text"] in path.read_text(encoding="utf-8")


def search(store, question, n=6, per_essay=2):
    q = store["vectorizer"].transform([question])
    scores = cosine_similarity(q, store["matrix"])[0]
    order = scores.argsort()[::-1]

    hits, seen = [], {}
    for i in order:
        if scores[i] <= 0:
            break
        c = store["chunks"][i]
        # Don't let one essay crowd out the rest of the corpus.
        if seen.get(c["slug"], 0) >= per_essay:
            continue
        if not verify(c):  # the honesty gate
            continue
        seen[c["slug"]] = seen.get(c["slug"], 0) + 1
        hits.append({**c, "score": float(scores[i])})
        if len(hits) >= n:
            break
    return hits


def wrap(text, width=78, indent="  "):
    words, lines, cur = text.split(), [], ""
    for w in words:
        if cur and len(cur) + len(w) + 1 > width:
            lines.append(indent + cur)
            cur = w
        else:
            cur = (cur + " " + w).strip()
    if cur:
        lines.append(indent + cur)
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("question", nargs="+")
    ap.add_argument("-n", type=int, default=6, help="passages to return")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()

    question = " ".join(args.question)
    store = load()
    hits = search(store, question, n=args.n)

    if args.json:
        import json
        print(json.dumps(hits, indent=2))
        return 0

    print(f"\nQ: {question}\n")
    if not hits:
        print("Nothing in the essays matches this. He may simply never have")
        print("written about it -- rather than guess, this returns nothing.\n")
        return 1

    strong = [h for h in hits if h["score"] >= WEAK]
    if not strong:
        print("!! Weak matches only. He may not address this directly;")
        print("   read these skeptically.\n")

    for i, h in enumerate(hits, 1):
        flag = "" if h["score"] >= WEAK else "  [weak match]"
        print(f"[{i}] {h['title']}  ({h['slug']}.txt){flag}")
        print(wrap(h["text"]))
        print()

    print(f"-- {len(hits)} passages, all verified against files in Essays/text/\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
