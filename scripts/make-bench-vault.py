#!/usr/bin/env python3
"""Make the vaults the Obsidian-versus-Krate benchmark opens (IC-999).

Both apps must open byte-identical vaults, so every byte here comes from a
fixed seed through our own generator (splitmix64), never from Python's
`random`, whose algorithms are free to change between versions. The same
arguments give the same bytes on any machine and any Python 3.8+; the
self-test proves it by generating twice and comparing.

Loads:
  light    100 notes
  medium   1,000 notes
  heavy    10,000 notes
  big-N    one note of exactly N lines (1000, 50000, 200000), alone in a vault

Notes are realistic Markdown: headings, paragraphs with bold, italic and
code, bullet, numbered and task lists, quotes, fenced code, [[links]] to
other notes (some notes are hubs that many link to, as in a real vault,
so backlinks have real work) with aliases and heading links, and #tags,
some nested. Folders follow a common vault layout and no folder holds
more than 400 entries (Krate's runtime lists at most 4,096 entries in one
folder; see evidence/benchmarks/obsidian-vs-krate/parity.md).

Each vault gets a manifest beside it, <out>/<name>.manifest.json, with
every file's size and sha256 and one digest over the whole tree.

Usage:
  python3 scripts/make-bench-vault.py --out /tmp/bench --load medium
  python3 scripts/make-bench-vault.py --out /tmp/bench --big 200000
  python3 scripts/make-bench-vault.py --out /tmp/bench --all
  python3 scripts/make-bench-vault.py --self-test
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

GENERATOR = "make-bench-vault/1"
DEFAULT_SEED = 0x1C999
LOADS = {"light": 100, "medium": 1_000, "heavy": 10_000}
BIG = (1_000, 50_000, 200_000)
FOLDER_MAX = 400

MASK = (1 << 64) - 1


class Rng:
    """splitmix64: tiny, fast, and the same everywhere."""

    def __init__(self, seed: int) -> None:
        self.state = seed & MASK

    def next(self) -> int:
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK
        return z ^ (z >> 31)

    def below(self, n: int) -> int:
        return self.next() % n if n > 0 else 0

    def chance(self, per_thousand: int) -> bool:
        return self.below(1000) < per_thousand

    def pick(self, items):
        return items[self.below(len(items))]

    def skewed(self, n: int) -> int:
        """An index in 0..n, low ones far more often: hubs."""
        a = self.below(n)
        b = self.below(n)
        c = self.below(n)
        return min(a, b, c)


WORDS = (
    "the of and to in is that for it as with was on be at by this had not are but from or have an "
    "they which one you were her all she there would their we him been has when who will more no if "
    "out so said what up its about into than them can only other new some could time these two may "
    "then do first any my now such like our over man me even most made after also did many before "
    "must through back years where much your way well down should because each just those people how "
    "too little state good very make world still own see men work long get here between both life "
    "being under never day same another know while last might us great old year off come since "
    "against go came right used take three plan garden river paper budget meeting review draft "
    "project design notes idea question answer reading chapter quarter sprint release customer team "
    "server client schedule morning evening weekend recipe travel health habit focus goal metric "
    "experiment result summary decision risk follow-up timeline roadmap backlog feature bug test"
).split()

TITLE_A = (
    "Alder Birch Cedar Delta Ember Fjord Granite Harbor Indigo Juniper Kestrel Lumen Maple Nimbus "
    "Onyx Prairie Quartz Raven Sierra Tundra Umber Velvet Willow Yarrow Zephyr Atlas Beacon Comet "
    "Dune Echo Flint Grove Haven Iris Jade Kite Lark Mesa Nova Orbit"
).split()
TITLE_B = (
    "plan notes review meeting ideas research draft log checklist summary journal roadmap brief "
    "retro budget proposal reading recipe trip spec design question findings"
).split()

TAGS = (
    "project project/alpha project/beta project/gamma area/health area/finance area/home reading "
    "idea todo meeting review research draft archive person book recipe travel work personal "
    "writing learning weekly daily important someday reference howto decision risk"
).split()

FOLDERS = (
    "Projects",
    "Areas",
    "Resources",
    "Archive",
    "Journal/2025",
    "Journal/2026",
    "People",
    "Meetings",
)

CODE_LANGS = ("python", "rust", "js", "bash", "sql", "")
CODE_LINES = {
    "python": ["def total(items):", "    return sum(i.price for i in items)", "", "print(total(cart))"],
    "rust": ["fn total(items: &[Item]) -> u64 {", "    items.iter().map(|i| i.price).sum()", "}"],
    "js": ["const total = items.reduce((s, i) => s + i.price, 0);", "console.log(total);"],
    "bash": ["for f in *.md; do", '  wc -l "$f"', "done"],
    "sql": ["select title, count(*) as links", "from notes join links using (id)", "group by title;"],
    "": ["plain text block", "kept exactly as typed"],
}


def sentence(rng: Rng, names: list[str], link_per_thousand: int = 45) -> str:
    """A sentence with some inline Markdown, links and tags."""
    n = 6 + rng.below(14)
    out: list[str] = []
    for i in range(n):
        w = rng.pick(WORDS)
        roll = rng.below(1000)
        if names and roll < link_per_thousand:
            target = names[rng.skewed(len(names))]
            kind = rng.below(10)
            if kind == 0:
                w = f"[[{target}|{rng.pick(WORDS)}]]"
            elif kind == 1:
                w = f"[[{target}#Summary]]"
            else:
                w = f"[[{target}]]"
        elif roll < link_per_thousand + 25:
            w = f"**{w}**"
        elif roll < link_per_thousand + 50:
            w = f"*{w}*"
        elif roll < link_per_thousand + 62:
            w = f"`{w}`"
        elif roll < link_per_thousand + 72:
            w = f"#{rng.pick(TAGS)}"
        out.append(w)
    s = " ".join(out)
    return s[0].upper() + s[1:] + "."


def paragraph(rng: Rng, names: list[str]) -> str:
    return " ".join(sentence(rng, names) for _ in range(1 + rng.below(4)))


def note_body(rng: Rng, title: str, names: list[str]) -> str:
    lines: list[str] = [f"# {title}", ""]
    lines.append(paragraph(rng, names))
    lines.append("")
    tags = sorted({rng.pick(TAGS) for _ in range(1 + rng.below(3))})
    lines.append(" ".join(f"#{t}" for t in tags))
    lines.append("")
    sections = ("Summary", "Notes", "Tasks", "Links", "Details", "Questions", "Log")
    for s in range(2 + rng.below(4)):
        lines.append(f"## {sections[s % len(sections)]}")
        lines.append("")
        kind = rng.below(6)
        if kind == 0:
            for _ in range(2 + rng.below(5)):
                lines.append(f"- {sentence(rng, names)}")
                if rng.chance(250):
                    lines.append(f"\t- {sentence(rng, names)}")
        elif kind == 1:
            for _ in range(2 + rng.below(5)):
                mark = "x" if rng.chance(400) else " "
                lines.append(f"- [{mark}] {sentence(rng, names, 60)}")
        elif kind == 2:
            for i in range(2 + rng.below(4)):
                lines.append(f"{i + 1}. {sentence(rng, names)}")
        elif kind == 3:
            lang = rng.pick(CODE_LANGS)
            lines.append(f"```{lang}")
            lines.extend(CODE_LINES[lang])
            lines.append("```")
        elif kind == 4:
            lines.append(f"> {sentence(rng, names)}")
            lines.append(f"> {sentence(rng, names)}")
        else:
            lines.append(paragraph(rng, names))
            lines.append("")
            lines.append(paragraph(rng, names))
        lines.append("")
    if names:
        related = sorted({names[rng.skewed(len(names))] for _ in range(1 + rng.below(4))})
        lines.append("Related: " + ", ".join(f"[[{r}]]" for r in related))
        lines.append("")
    return "\n".join(lines)


def note_names(count: int, rng: Rng) -> list[str]:
    names: list[str] = []
    for i in range(count):
        a = rng.pick(TITLE_A)
        b = rng.pick(TITLE_B)
        names.append(f"{a} {b} {i + 1:05d}")
    return names


def folder_for(i: int, count: int) -> str:
    """Spread notes over the usual folders, adding numbered subfolders when
    one would pass FOLDER_MAX entries."""
    area = FOLDERS[i % len(FOLDERS)]
    per_area = (count + len(FOLDERS) - 1) // len(FOLDERS)
    if per_area <= FOLDER_MAX:
        return area
    buckets = (per_area + FOLDER_MAX - 1) // FOLDER_MAX
    b = (i // len(FOLDERS)) % buckets
    return f"{area}/{b + 1:02d}"


def make_vault(root: Path, count: int, seed: int) -> list[Path]:
    rng = Rng(seed ^ count)
    names = note_names(count, rng)
    written: list[Path] = []
    # A front door, as most vaults have: it links to the busiest notes.
    home = ["# Home", "", "Start here. The notes people open most:", ""]
    for n in names[: min(20, count)]:
        home.append(f"- [[{n}]]")
    home.append("")
    home.append("#index")
    home.append("")
    files: list[tuple[str, str]] = [("Home.md", "\n".join(home))]
    for i, name in enumerate(names):
        body = note_body(rng, name, names)
        files.append((f"{folder_for(i, count)}/{name}.md", body))
    for rel, text in files:
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(text.encode("utf-8"))
        written.append(p)
    return written


def make_big(root: Path, lines: int, seed: int) -> list[Path]:
    """One note of exactly `lines` lines (each ends with a newline)."""
    rng = Rng(seed ^ (lines << 8))
    companions = [f"Companion note {i:03d}" for i in range(1, 51)]
    out: list[str] = [f"# Big note {lines}"]
    section = 0
    while len(out) < lines:
        n = len(out)
        if n % 50 == 1:
            section += 1
            out.append(f"## Section {section}")
            continue
        if n % 400 == 10:
            lang = rng.pick(CODE_LANGS)
            block = [f"```{lang}", *CODE_LINES[lang], "```"]
            out.extend(block[: lines - len(out)])
            continue
        kind = rng.below(10)
        if kind < 5:
            out.append(sentence(rng, companions, 40))
        elif kind < 7:
            out.append(f"- {sentence(rng, companions, 40)}")
        elif kind == 7:
            out.append(f"- [{'x' if rng.chance(500) else ' '}] {sentence(rng, companions, 20)}")
        elif kind == 8:
            out.append("")
        else:
            out.append(f"> {sentence(rng, companions, 20)}")
    out = out[:lines]
    p = root / f"Big note {lines}.md"
    root.mkdir(parents=True, exist_ok=True)
    p.write_bytes(("\n".join(out) + "\n").encode("utf-8"))
    return [p]


def manifest(root: Path, name: str, args: dict) -> dict:
    files = []
    tree = hashlib.sha256()
    for p in sorted(root.rglob("*"), key=lambda q: q.relative_to(root).as_posix()):
        if not p.is_file():
            continue
        rel = p.relative_to(root).as_posix()
        data = p.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        tree.update(f"{rel}\0{digest}\n".encode("utf-8"))
        files.append({"path": rel, "bytes": len(data), "sha256": digest})
    return {
        "generator": GENERATOR,
        "vault": name,
        "args": args,
        "files": len(files),
        "bytes": sum(f["bytes"] for f in files),
        "tree_sha256": tree.hexdigest(),
        "entries": files,
    }


def build(out: Path, name: str, seed: int) -> dict:
    root = out / name
    if root.exists():
        shutil.rmtree(root)
    root.mkdir(parents=True)
    if name in LOADS:
        make_vault(root, LOADS[name], seed)
        args = {"load": name, "notes": LOADS[name], "seed": seed}
    else:
        lines = int(name.split("-", 1)[1])
        make_big(root, lines, seed)
        args = {"big_lines": lines, "seed": seed}
    m = manifest(root, name, args)
    (out / f"{name}.manifest.json").write_text(json.dumps(m, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    return m


def largest_folder(root: Path) -> int:
    most = 0
    for d, dirs, files in os.walk(root):
        most = max(most, len(dirs) + len(files))
    return most


def self_test() -> int:
    failures = 0

    def check(ok: bool, what: str) -> None:
        nonlocal failures
        print(("ok    " if ok else "FAIL  ") + what)
        if not ok:
            failures += 1

    r1, r2 = Rng(7), Rng(7)
    check([r1.next() for _ in range(5)] == [r2.next() for _ in range(5)], "the generator repeats for one seed")
    check(Rng(1).next() != Rng(2).next(), "different seeds differ")
    # splitmix64's published first output for seed 0, so a port can be checked.
    check(Rng(0).next() == 0xE220A8397B1DCDAF, "splitmix64 matches its reference value")
    with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
        for name in ("light", "big-1000"):
            ma = build(Path(a), name, DEFAULT_SEED)
            mb = build(Path(b), name, DEFAULT_SEED)
            check(ma == mb, f"{name}: two runs are byte-identical ({ma['tree_sha256'][:16]})")
        light = json.loads((Path(a) / "light.manifest.json").read_text())
        notes = [e for e in light["entries"] if e["path"].endswith(".md")]
        check(len(notes) == LOADS["light"] + 1, f"light: {LOADS['light']} notes and Home ({len(notes)})")
        other = build(Path(b), "light", DEFAULT_SEED + 1)
        check(other["tree_sha256"] != light["tree_sha256"], "another seed gives another vault")
        big = (Path(a) / "big-1000" / "Big note 1000.md").read_bytes()
        check(big.count(b"\n") == 1000 and big.endswith(b"\n"), "big-1000 is exactly 1,000 lines")
        text = "".join(p.read_text(encoding="utf-8") for p in (Path(a) / "light").rglob("*.md"))
        check(text.count("[[") > 300, f"light: notes link to each other ({text.count('[[')} links)")
        check(text.count("#project") > 5 and "```" in text and "- [ ]" in text, "light: tags, code and tasks")
        # Links resolve: every [[target]] (before | or #) names a note.
        names = {Path(e["path"]).stem for e in notes}
        targets = set()
        for chunk in text.split("[[")[1:]:
            t = chunk.split("]]", 1)[0].split("|", 1)[0].split("#", 1)[0]
            targets.add(t)
        check(targets <= names, f"light: all {len(targets)} link targets are notes")
        # The heavy layout keeps folders under the runtime's listing limit.
        counts: dict[str, int] = {}
        for i in range(LOADS["heavy"]):
            f = folder_for(i, LOADS["heavy"])
            counts[f] = counts.get(f, 0) + 1
        check(max(counts.values()) <= FOLDER_MAX, f"heavy: at most {max(counts.values())} notes a folder")
        check(largest_folder(Path(a) / "light") <= FOLDER_MAX, "light: folders are small")
    print("self-test:", "PASS" if failures == 0 else f"FAIL ({failures})")
    return 0 if failures == 0 else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, help="folder to write vaults and manifests into")
    ap.add_argument("--load", choices=sorted(LOADS), action="append", default=[])
    ap.add_argument("--big", type=int, choices=BIG, action="append", default=[])
    ap.add_argument("--all", action="store_true", help="every load and every big note")
    ap.add_argument("--seed", type=lambda s: int(s, 0), default=DEFAULT_SEED)
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    if a.out is None:
        ap.error("--out is required")
    names = list(a.load) + [f"big-{n}" for n in a.big]
    if a.all:
        names = list(LOADS) + [f"big-{n}" for n in BIG]
    if not names:
        ap.error("name a --load, a --big, or --all")
    a.out.mkdir(parents=True, exist_ok=True)
    for name in names:
        m = build(a.out, name, a.seed)
        print(f"{name}\tfiles={m['files']}\tbytes={m['bytes']}\ttree_sha256={m['tree_sha256']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
