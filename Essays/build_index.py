#!/usr/bin/env python3
"""Build the searchable index over the downloaded essays.

Chunks each essay into paragraphs (the unit PG actually writes in), then
builds a TF-IDF matrix over them. Every chunk keeps a byte offset back into
its source .txt file, so any quote can be re-verified character-for-character
against the file on disk.
"""

import json
import pickle
import re
from pathlib import Path

from sklearn.feature_extraction.text import TfidfVectorizer

ROOT = Path(__file__).resolve().parent
TEXT = ROOT / "text"
INDEX = ROOT / "index"

MIN_CHARS = 120   # skip fragments too small to be a real point
MAX_CHARS = 1800  # split anything longer so quotes stay findable


def strip_boilerplate(body, title):
    """Drop the header/footer scaffolding that isn't essay prose."""
    lines = body.split("\n")
    # The title usually appears in the first few lines; start after it.
    start = 0
    for i, ln in enumerate(lines[:12]):
        if title and title.lower()[:28] in ln.lower():
            start = i + 1
            break
    body = "\n".join(lines[start:])

    # PG's footers: notes, thanks, translations, and nav junk.
    cut_markers = [
        r"\n\s*Notes?\s*\n", r"\n\s*Thanks to\b", r"\n\s*Thanks\s*\n",
        r"\n\s*Related:\s*\n", r"\n\s*Japanese Translation",
        r"\n\s*\[?Translations?\b",
    ]
    for pat in cut_markers:
        m = re.search(pat, body)
        if m and m.start() > len(body) * 0.4:  # only if it's genuinely the tail
            body = body[: m.start()]
    return body.strip()


def split_long(para, limit=MAX_CHARS):
    """Split an over-long paragraph on sentence boundaries."""
    if len(para) <= limit:
        return [para]
    sents = re.split(r"(?<=[.!?])\s+", para)
    out, cur = [], ""
    for s in sents:
        if cur and len(cur) + len(s) + 1 > limit:
            out.append(cur.strip())
            cur = s
        else:
            cur = (cur + " " + s).strip()
    if cur.strip():
        out.append(cur.strip())
    return out


def chunk_essay(slug, title, body):
    """Paragraph chunks, each with a verified offset into the source text."""
    cleaned = strip_boilerplate(body, title)
    chunks = []
    for para in re.split(r"\n\s*\n", cleaned):
        para = para.strip()
        if len(para) < MIN_CHARS:
            continue
        if re.match(r"^\s*\[\d+\]", para):  # footnote block
            continue
        for piece in split_long(para):
            if len(piece) < MIN_CHARS:
                continue
            offset = body.find(piece)  # verified below; -1 means reformatted
            chunks.append({
                "slug": slug,
                "title": title,
                "text": piece,
                "offset": offset,
            })
    return chunks


def main():
    manifest = json.loads((INDEX / "manifest.json").read_text(encoding="utf-8"))
    all_chunks = []

    for entry in manifest:
        slug, title = entry["slug"], entry["title"]
        path = TEXT / (slug + ".txt")
        if not path.exists():
            continue
        body = path.read_text(encoding="utf-8")
        all_chunks.extend(chunk_essay(slug, title, body))

    # Verify every chunk really exists in its source file, byte for byte.
    verified, broken = [], 0
    for c in all_chunks:
        src = (TEXT / (c["slug"] + ".txt")).read_text(encoding="utf-8")
        if c["text"] in src:
            verified.append(c)
        else:
            broken += 1

    print(f"{len(verified)} chunks from {len(manifest)} essays "
          f"({broken} dropped as unverifiable)")

    vectorizer = TfidfVectorizer(
        lowercase=True,
        stop_words="english",
        ngram_range=(1, 2),
        min_df=1,
        max_df=0.55,
        sublinear_tf=True,
        strip_accents="unicode",
    )
    matrix = vectorizer.fit_transform([c["text"] for c in verified])
    print(f"tf-idf matrix: {matrix.shape[0]} chunks x {matrix.shape[1]} terms")

    with open(INDEX / "search.pkl", "wb") as f:
        pickle.dump(
            {"chunks": verified, "vectorizer": vectorizer, "matrix": matrix},
            f,
        )
    print(f"wrote {INDEX / 'search.pkl'}")


if __name__ == "__main__":
    main()
