#!/usr/bin/env python3
"""Re-extract text/*.txt from the saved raw/*.html.

Safe to run any time: raw HTML is the source of truth, so this can be re-run
after improving the extraction rules without re-downloading anything.
"""

import re
from pathlib import Path

from fetch_essays import clean_txt, decode, strip_html

ROOT = Path(__file__).resolve().parent
RAW = ROOT / "raw"
TEXT = ROOT / "text"


def main():
    fixed = 0
    for raw_path in sorted(RAW.iterdir()):
        if raw_path.suffix not in (".html", ".txt"):
            continue
        slug = raw_path.stem
        doc = decode(raw_path.read_bytes())
        body = clean_txt(doc) if raw_path.suffix == ".txt" else strip_html(doc)
        if len(body) < 200:
            print(f"skip {slug}: too short")
            continue

        out = TEXT / (slug + ".txt")
        before = out.read_text(encoding="utf-8") if out.exists() else ""
        if before != body:
            out.write_text(body, encoding="utf-8")
            fixed += 1
    print(f"re-extracted {fixed} files")

    left = [p.name for p in TEXT.glob("*.txt")
            if "-->" in p.read_text(encoding="utf-8")]
    print(f"files still containing '-->': {len(left)}")
    if left:
        print("  " + ", ".join(left[:10]))


if __name__ == "__main__":
    main()
