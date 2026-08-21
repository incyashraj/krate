#!/usr/bin/env python3
"""Generate the exact Markdown workloads used by both benchmark legs."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path


BODY = (
    "Line {line}: the quick brown fox jumps over the lazy dog, "
    "note line with some *emphasis* and `code`.\n"
)


def make_document(content_lines: int) -> bytes:
    chunks: list[str] = []
    section = 0
    for line in range(1, content_lines + 1):
        if (line - 1) % 50 == 0:
            section += 1
            if chunks:
                chunks.append("\n")
            chunks.append(f"## Section {section}\n\n")
        chunks.append(BODY.format(line=line))
    return "".join(chunks).encode("utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", type=Path, default=Path(__file__).parent)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)

    for count in (5_000, 50_000):
        path = args.output_dir / f"notes-{count}.md"
        data = make_document(count)
        path.write_bytes(data)
        physical_lines = data.count(b"\n")
        digest = hashlib.sha256(data).hexdigest()
        print(
            f"{path.name}\tcontent_lines={count}\tphysical_lines={physical_lines}"
            f"\tbytes={len(data)}\tsha256={digest}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
