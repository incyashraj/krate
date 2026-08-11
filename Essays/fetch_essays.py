#!/usr/bin/env python3
"""Download every Paul Graham essay listed on paulgraham.com/articles.html.

Keeps the raw HTML (so quotes can always be re-verified against the source)
and a cleaned plain-text version used for search.

Polite: one request at a time, with a delay between them.
"""

import html
import json
import os
import re
import sys
import time
import urllib.request
from pathlib import Path

BASE = "https://paulgraham.com/"
INDEX_URL = BASE + "articles.html"
ROOT = Path(__file__).resolve().parent
RAW = ROOT / "raw"
TEXT = ROOT / "text"
INDEX = ROOT / "index"

UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) personal-archive/1.0"
DELAY = 1.0  # seconds between requests


def get(url):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def decode(raw_bytes):
    for enc in ("utf-8", "cp1252", "latin-1"):
        try:
            return raw_bytes.decode(enc)
        except UnicodeDecodeError:
            continue
    return raw_bytes.decode("utf-8", errors="replace")


def discover():
    """Return [(filename, title)] for every essay linked from the index."""
    page = decode(get(INDEX_URL))
    # Links look like: <a href="greatwork.html">How to Do Great Work</a>
    pairs = re.findall(
        r'<a\s+href="([a-zA-Z0-9_.\-]+\.(?:html|txt))"[^>]*>(.*?)</a>',
        page,
        flags=re.I | re.S,
    )
    seen, out = set(), []
    skip = {
        "index.html", "articles.html", "rss.html", "bio.html", "books.html",
        "faq.html", "raq.html", "quo.html", "sitemap.html", "arc.html",
        "lisp.html", "antispam.html", "kedrosky.html", "rss.txt",
    }
    for href, label in pairs:
        href = href.strip()
        title = re.sub(r"<[^>]+>", "", label)
        title = html.unescape(title).strip()
        if not title or href.lower() in skip or href in seen:
            continue
        # Nav links and cross-references are short/boilerplate.
        if title.lower() in {"", "index", "essays", "home", "next", "prev"}:
            continue
        seen.add(href)
        out.append((href, title))
    return out


def strip_html(doc):
    """PG's pages are hand-written HTML; the essay body is plain text in tables."""
    doc = re.sub(r"(?is)<script.*?</script>", " ", doc)
    doc = re.sub(r"(?is)<style.*?</style>", " ", doc)
    # Comments first: otherwise tag-stripping eats the opening "<!--" and
    # leaves a stray "-->" in the text.
    doc = re.sub(r"(?s)<!--.*?-->", " ", doc)
    doc = re.sub(r"(?s)<!--.*", " ", doc)  # unclosed comment
    doc = doc.replace("-->", " ")          # orphaned closer
    # Preserve paragraph and line structure before dropping tags.
    doc = re.sub(r"(?i)<br\s*/?>", "\n", doc)
    doc = re.sub(r"(?i)</?p\b[^>]*>", "\n\n", doc)
    doc = re.sub(r"(?i)</(?:tr|div|table|h[1-6]|li)>", "\n", doc)
    doc = re.sub(r"<[^>]+>", "", doc)
    doc = html.unescape(doc)
    doc = doc.replace("\r\n", "\n").replace("\r", "\n")
    doc = re.sub(r"[ \t\xa0]+", " ", doc)
    doc = re.sub(r" *\n *", "\n", doc)
    doc = re.sub(r"\n{3,}", "\n\n", doc)
    return doc.strip()


def clean_txt(doc):
    doc = doc.replace("\r\n", "\n").replace("\r", "\n")
    doc = re.sub(r"[ \t\xa0]+", " ", doc)
    doc = re.sub(r"\n{3,}", "\n\n", doc)
    return doc.strip()


def main():
    for d in (RAW, TEXT, INDEX):
        d.mkdir(parents=True, exist_ok=True)

    essays = discover()
    print(f"index lists {len(essays)} essays", flush=True)

    manifest, failures = [], []
    for i, (fname, title) in enumerate(essays, 1):
        slug = fname.rsplit(".", 1)[0]
        raw_path = RAW / fname
        txt_path = TEXT / (slug + ".txt")

        if txt_path.exists() and txt_path.stat().st_size > 200:
            body = txt_path.read_text(encoding="utf-8")
            manifest.append({
                "slug": slug, "title": title, "url": BASE + fname,
                "chars": len(body), "words": len(body.split()),
            })
            print(f"[{i:3}/{len(essays)}] cached  {slug}", flush=True)
            continue

        try:
            raw = get(BASE + fname)
            raw_path.write_bytes(raw)
            doc = decode(raw)
            body = clean_txt(doc) if fname.endswith(".txt") else strip_html(doc)

            if len(body) < 200:
                failures.append((fname, f"too short ({len(body)} chars)"))
                print(f"[{i:3}/{len(essays)}] SHORT   {slug}", flush=True)
                continue

            txt_path.write_text(body, encoding="utf-8")
            manifest.append({
                "slug": slug, "title": title, "url": BASE + fname,
                "chars": len(body), "words": len(body.split()),
            })
            print(f"[{i:3}/{len(essays)}] ok      {slug} ({len(body.split())} words)", flush=True)
        except Exception as e:
            failures.append((fname, str(e)))
            print(f"[{i:3}/{len(essays)}] FAIL    {slug}: {e}", flush=True)

        time.sleep(DELAY)

    (INDEX / "manifest.json").write_text(
        json.dumps(manifest, indent=2), encoding="utf-8"
    )

    total_words = sum(m["words"] for m in manifest)
    print(f"\nsaved {len(manifest)} essays, {total_words:,} words")
    if failures:
        print(f"{len(failures)} failed:")
        for f, why in failures:
            print(f"  {f}: {why}")
        (INDEX / "failures.json").write_text(
            json.dumps(failures, indent=2), encoding="utf-8"
        )


if __name__ == "__main__":
    sys.exit(main())
