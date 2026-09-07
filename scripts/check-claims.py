#!/usr/bin/env python3
"""Hold public performance wording to the evidence it came from (IC-830).

    scripts/check-claims.py            # check every public surface
    scripts/check-claims.py --list     # show the claim record

Prose does not get to hold its own numbers. Every figure a public surface
states about performance has to appear in evidence/claims/performance.json,
which names the exact evidence row, machine, workload and confidence behind
it. A number in the prose that the record does not carry is a drift, and this
exits 1 saying which one.

The failure this exists for was in the README: a table headed "Memory, 50,000
lines" whose open-time row read 1.8 s versus 0.2 s -- the 5,000-line warm
start and the first-ever open. Each number true somewhere; the pair true
nowhere. Nobody was lying. The prose had drifted from the note it cites and
nothing compared them.

It also refuses the sentences the benchmark's own CLAIMS.md forbids --
universal "faster than Electron" wording, "independently reproducible" while
the raw samples are unretained, any battery claim at all.
"""
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RECORD = ROOT / "evidence" / "claims" / "performance.json"

# The surfaces a stranger reads. Evidence notes are deliberately absent: they
# are the source, and holding them to themselves would be circular.
SURFACES = [
    "README.md",
    "docs/landing/index.html",
    "docs/index.html",
]

# Figures that are not performance claims: version numbers, ports, years,
# and the app-size facts that have their own dedicated line in the record.
IGNORE = re.compile(
    r"""(?x)
    ^(0|1|2|3|4|5|6|7|8|9|10|100)$   # bare small integers: list markers, counts
    """
)

NUMBER = re.compile(
    r"(?<![\w.])(\d+(?:[.,]\d+)?)\s*(GB|MB|KB|GiB|MiB|KiB|ms|s|fps|%)(?![\w])"
)


def load():
    return json.loads(RECORD.read_text())


def known_figures(record):
    """Every number the record vouches for, as a normalised string set."""
    figures = set()
    for claim in record["claims"]:
        for field in ("them", "us"):
            for value, unit in NUMBER.findall(claim.get(field, "")):
                figures.add(normalise(value, unit))
    return figures


def normalise(value, unit):
    value = value.replace(",", "")
    # Trailing zeros do not change a measurement: 1.80 s and 1.8 s are one
    # number written two ways, and a checker that disagrees is noise.
    try:
        value = ("%g" % float(value))
    except ValueError:
        pass
    return f"{value}{unit}"


def check_surface(path, record, figures):
    problems = []
    text = (ROOT / path).read_text(errors="replace")

    for phrase in record["forbidden"]:
        if re.search(phrase["pattern"], text, re.I):
            problems.append(
                f"{path}: says {phrase['pattern']!r} -- {phrase['why']}"
            )

    for line_no, line in enumerate(text.splitlines(), 1):
        # Only lines that are making a comparison. A number in ordinary prose
        # ("takes 2 minutes to install") is not a performance claim about the
        # product against another product.
        if not any(word in line.lower() for word in ("marktext", "electron")) and "|" not in line:
            continue
        for value, unit in NUMBER.findall(line):
            figure = normalise(value, unit)
            if IGNORE.match(figure):
                continue
            if figure not in figures:
                problems.append(
                    f"{path}:{line_no}: states {figure}, which no claim record "
                    f"vouches for -- add the measurement to "
                    f"evidence/claims/performance.json with its evidence row, "
                    f"or correct the prose"
                )
    return problems


def main():
    record = load()
    if "--list" in sys.argv:
        for claim in record["claims"]:
            print(f"  {claim['id']:18} {claim['metric']}")
            print(f"  {'':18} them: {claim['them']}   us: {claim['us']}")
            print(f"  {'':18} {claim['source']}")
            if claim.get("caveat"):
                print(f"  {'':18} caveat: {claim['caveat']}")
            print()
        return 0

    figures = known_figures(record)
    problems = []
    checked = 0
    for surface in SURFACES:
        if not (ROOT / surface).is_file():
            continue
        checked += 1
        problems.extend(check_surface(surface, record, figures))

    if problems:
        print(f"claim drift in {checked} surface(s):\n")
        for problem in problems:
            print(f"  {problem}")
        print(
            "\nEvery public performance number must name an evidence row. "
            "See evidence/claims/performance.json."
        )
        return 1

    print(f"OK -- {checked} public surface(s), every performance figure backed by the claim record.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
