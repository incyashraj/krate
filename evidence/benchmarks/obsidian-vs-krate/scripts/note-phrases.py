#!/usr/bin/env python3
"""The targets of each vault: which note opens at start, the phrases that
prove it is on screen, and the switcher and search queries (IC-999).

A phrase is 5 consecutive plain words from one line of the note (a word is
plain when it carries no Markdown: no [[, **, `, # and so on), normalised
the way start-probe normalises what it reads (lower case, anything but
a-z, 0-9 and '-' a space). Plain words are drawn as written by both apps
in live preview, so a phrase on screen is a phrase in the note. A phrase
that occurs twice in the note is dropped, so each names one line.

  note-phrases.py --vaults DIR --out DIR     one <load>.json and
                                              <load>.open.tsv per vault
  note-phrases.py --self-test

Targets, fixed by rule, not by hand:
  open      light/medium/heavy: the note numbered 00001 (the busiest hub);
            big-N: its one note
  switcher  the last-numbered note (its name, typed in lower case)
  search    a phrase found in exactly one note of the vault, from the note
            numbered at the middle; the answer is that note's name
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

N = 5
PLAIN = re.compile(r"^[A-Za-z][A-Za-z-]*[.,:;]?$")


def norm_words(text: str) -> list[str]:
    return re.sub(r"[^a-z0-9-]+", " ", text.lower()).split()


def line_phrases(line: str) -> list[str]:
    """Every run of N plain words in one source line."""
    runs: list[list[str]] = [[]]
    for tok in line.split():
        if PLAIN.match(tok):
            runs[-1].extend(norm_words(tok))
        else:
            runs.append([])
    out = []
    for run in runs:
        for i in range(len(run) - N + 1):
            out.append(" ".join(run[i : i + N]))
    return out


def phrases(text: str, limit: int | None = None) -> list[tuple[int, str]]:
    """(line number, phrase), the first unique phrase of each line."""
    counts: dict[str, int] = {}
    per_line: list[tuple[int, list[str]]] = []
    for n, line in enumerate(text.split("\n"), start=1):
        if line.startswith("```") or line.startswith("    "):
            continue
        ps = line_phrases(line)
        per_line.append((n, ps))
        for p in ps:
            counts[p] = counts.get(p, 0) + 1
    out = []
    for n, ps in per_line:
        for p in ps:
            if counts[p] == 1:
                out.append((n, p))
                break
        if limit and len(out) >= limit:
            break
    return out


def numbered(vault: Path) -> dict[int, Path]:
    out = {}
    for p in vault.rglob("*.md"):
        m = re.search(r" (\d{5})\.md$", p.name)
        if m and not any(part.startswith(".") for part in p.relative_to(vault).parts):
            out[int(m.group(1))] = p
    return out


def targets(vault: Path, load: str) -> dict:
    rel = lambda p: p.relative_to(vault).as_posix()  # noqa: E731
    if load.startswith("big-"):
        (note,) = [p for p in vault.glob("*.md")]
        return {"load": load, "open_note": rel(note), "open_name": note.stem}
    notes = numbered(vault)
    first, last = notes[min(notes)], notes[max(notes)]
    middle = notes[sorted(notes)[len(notes) // 2]]
    # A phrase of the middle note that no other note contains, written in
    # the note exactly so (no punctuation inside it), so an exact-phrase
    # search in either app finds it as typed.
    corpus = {p: p.read_text(encoding="utf-8").lower() for p in vault.rglob("*.md")}
    query = None
    for _, ph in phrases(corpus[middle]):
        if ph in corpus[middle] and sum(1 for t in corpus.values() if ph in t) == 1:
            query = ph
            break
    if query is None:
        raise SystemExit(f"{load}: no phrase of {middle.name} is unique in the vault")
    return {
        "load": load,
        "open_note": rel(first),
        "open_name": first.stem,
        "switcher": {"query": last.stem.lower(), "expect": last.stem, "note": rel(last)},
        "search": {"query": f'"{query}"', "expect": middle.stem, "note": rel(middle)},
    }


def write(vaults: Path, out: Path) -> list[str]:
    out.mkdir(parents=True, exist_ok=True)
    done = []
    for vault in sorted(p for p in vaults.iterdir() if p.is_dir()):
        load = vault.name
        t = targets(vault, load)
        text = (vault / t["open_note"]).read_text(encoding="utf-8")
        ph = phrases(text)
        t["open_phrases"] = len(ph)
        tsv = "# line\tphrase (note-phrases.py; start-probe reads this)\n" + "".join(f"{n}\t{p}\n" for n, p in ph)
        (out / f"{load}.open.tsv").write_text(tsv, encoding="utf-8")
        if "switcher" in t:
            sw = phrases((vault / t["switcher"]["note"]).read_text(encoding="utf-8"))
            (out / f"{load}.switcher.tsv").write_text("".join(f"{n}\t{p}\n" for n, p in sw), encoding="utf-8")
        (out / f"{load}.json").write_text(json.dumps(t, indent=1, sort_keys=True) + "\n", encoding="utf-8")
        done.append(f"{load}\topen={t['open_note']}\tphrases={len(ph)}")
    return done


def self_test() -> int:
    fails = []
    line = "After did back since just make sprint been very *through* could [[Juniper brief 00020]] very then did."
    got = line_phrases(line)
    if got[0] != "after did back since just" or any("through" in p or "juniper" in p for p in got):
        fails.append(f"markup words break phrases: {got[:3]}")
    if line_phrases("- [ ] Out take or schedule too health") != ["out take or schedule too", "take or schedule too health"]:
        fails.append(f"list marks are not words: {line_phrases('- [ ] Out take or schedule too health')}")
    doc = "# T\n\nthe quick brown fox jumps over\nthe quick brown fox jumps again\nunique words only here today\n"
    ph = phrases(doc)
    if (5, "unique words only here today") not in ph or any(p == "the quick brown fox jumps" for _, p in ph):
        fails.append(f"a phrase on two lines is dropped, unique ones kept: {ph}")
    sys.path.insert(0, str(Path(__file__).resolve().parents[4] / "scripts"))
    import importlib.util

    gen_path = Path(__file__).resolve().parents[4] / "scripts" / "make-bench-vault.py"
    spec = importlib.util.spec_from_file_location("mbv", gen_path)
    mbv = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mbv)
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        mbv.build(root / "v", "light", mbv.DEFAULT_SEED)
        mbv.build(root / "v", "big-1000", mbv.DEFAULT_SEED)
        write(root / "v", root / "t")
        t = json.loads((root / "t" / "light.json").read_text())
        if not t["open_note"].endswith("00001.md") or not t["switcher"]["expect"].endswith("00100"):
            fails.append(f"light targets: {t}")
        if t["open_phrases"] < 6:
            fails.append(f"the light open note has {t['open_phrases']} phrases; at least 6 expected")
        q = t["search"]["query"].strip('"')
        hits = [p for p in (root / "v" / "light").rglob("*.md") if q in p.read_text().lower()]
        if [h.stem for h in hits] != [t["search"]["expect"]]:
            fails.append(f"search phrase is in exactly the expected note: {hits}")
        big = (root / "t" / "big-1000.open.tsv").read_text().splitlines()
        if len(big) < 300:
            fails.append(f"big-1000 gives a phrase for most lines: {len(big)}")
    if fails:
        for f in fails:
            print("FAIL", f)
        return 1
    print("note-phrases self-test OK -- markup breaks phrases, repeated phrases dropped, targets fixed by rule, the search phrase is in one note only")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--vaults", type=Path)
    ap.add_argument("--out", type=Path)
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    if not a.vaults or not a.out:
        ap.error("--vaults and --out are required")
    for line in write(a.vaults, a.out):
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
