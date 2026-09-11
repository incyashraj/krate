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

# Claims that were checked, found wrong, and retired (IC-628). These are not
# performance figures to be backed by a measurement -- they are sentences that
# must not come back, in any document, however old.
#
# The scan is repository-wide rather than limited to the public surfaces above,
# because the way a retired claim returns is by being copied out of an old deck
# or a stale memo that nobody thought of as public until it was sent to
# somebody. Invest/OUTREACH_TRUTH.md is the record of why each one went.
RETIRED = [
    (
        r"6,?400\s*x",
        '"6400x smaller" was stated against "regular applications", which is '
        "not a measurement of anything. Against a native Rust build of the "
        "same program it is ~19x; against a typical Electron app it is "
        "~5,000-10,000x. Say which comparison you mean "
        "(Invest/OUTREACH_TRUTH.md)",
    ),
    (
        r"20\s*x\s+faster",
        '"20x faster" was never measured against "regular applications". '
        "The honest speed claims are the ones in "
        "evidence/claims/performance.json, each against a named app on a "
        "named machine (Invest/OUTREACH_TRUTH.md)",
    ),
    (
        r"faster\s+than\s+regular\s+applications",
        '"regular applications" is not a thing that can be measured. Name the '
        "app being compared against (Invest/OUTREACH_TRUTH.md)",
    ),
]

# Where a retired claim would hide. The truth file itself is excluded: it
# quotes every retired claim in order to retire it, so scanning it would make
# the record of the correction into a violation.
RETIRED_SCAN_SUFFIXES = (".md", ".tex", ".html", ".txt")
RETIRED_SKIP_PARTS = (
    ".git",
    "docs/book",  # build output; stale copies produce false hits
    "target",
    "node_modules",
    "Invest/krate_bible",  # the source register, quoting claims to correct them
)
RETIRED_SKIP_FILES = (
    "Invest/OUTREACH_TRUTH.md",  # the file that retires them
    "scripts/check-claims.py",  # this file, which must name them to forbid them
)

# The surfaces a stranger reads. Evidence notes are deliberately absent: they
# are the source, and holding them to themselves would be circular.
# The fixed surfaces are load-bearing BY NAME: if one of these is missing,
# the gate must fail rather than shrug, because a moved README is exactly how
# the most-read page leaves the scan while the OK count stays plausible
# (K-277: renaming two of them away still printed "OK -- 6 public
# surface(s)"). The globbed answers pages below may legitimately be empty.
FIXED_SURFACES = [
    "README.md",
    "docs/landing/index.html",
    "docs/landing/studio/index.html",
    # docs/index.html used to sit here. It has NEVER existed in this
    # repository -- no git history at all -- so the gate spent its whole life
    # claiming a surface it never scanned, and the silent-skip loop hid that
    # until the skip became a failure (K-277). The site's front page is
    # docs/landing/index.html, above.
    "docs/krate-mode.md",
    "docs/open/index.html",
]
SURFACES = list(FIXED_SURFACES)
# The answers pages share one meta description, so a claim corrected in the
# landing page can survive in three copies (IC-401 -- it did). Globbed rather
# than listed so a new page joins the check by existing.
import glob as _glob

SURFACES += sorted(
    p for p in _glob.glob("docs/answers/*.html")
)

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
    return load_from(RECORD)


def load_from(record_path):
    """The claim record, or a refusal in words (K-277 / IC-828).

    This used to be one line, and a missing or corrupt record exited 1 via a
    raw traceback. The exit code was right and the words were absent -- a
    traceback in a CI log reads as "the script broke", not "the evidence is
    missing", and those call for opposite responses.
    """
    try:
        record = json.loads(record_path.read_text())
    except FileNotFoundError:
        sys.exit(
            f"the claim record is MISSING: {record_path}\n"
            "Every public performance figure is held to this file, so with it "
            "absent the gate has nothing to hold anything to. Restore it; do "
            "not ship without it."
        )
    except (json.JSONDecodeError, OSError) as err:
        sys.exit(
            f"the claim record cannot be read: {record_path}\n  {err}\n"
            "A gate that cannot read its evidence must not pass."
        )
    claims = record.get("claims")
    if not isinstance(claims, list) or not claims:
        sys.exit(
            f"the claim record holds no claims: {record_path}\n"
            "Zero evidence means zero-work, and a zero-work check passing is "
            "the defect this gate exists to prevent (IC-828)."
        )
    return record


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


def scan_retired():
    """Every live document, looking for a claim that was retired (IC-628).

    Repository-wide on purpose. A retired claim comes back by being copied out
    of an old deck, not by being re-typed into the landing page.
    """
    problems = []
    scanned = 0
    for path in ROOT.rglob("*"):
        if not path.is_file() or path.suffix not in RETIRED_SCAN_SUFFIXES:
            continue
        rel = path.relative_to(ROOT).as_posix()
        if any(part in rel for part in RETIRED_SKIP_PARTS):
            continue
        if rel in RETIRED_SKIP_FILES:
            continue
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        scanned += 1
        for line_no, line in enumerate(text.splitlines(), 1):
            for pattern, why in RETIRED:
                if re.search(pattern, line, re.I):
                    problems.append(f"{rel}:{line_no}: {line.strip()[:90]}\n      {why}")
    return scanned, problems


def missing_fixed_surfaces(fixed=None):
    """Which load-bearing surfaces are absent. Parameterised for the
    rehearsal in self_test, so the check is exercised rather than trusted."""
    fixed = FIXED_SURFACES if fixed is None else fixed
    return [surface for surface in fixed if not (ROOT / surface).is_file()]


def self_test():
    """Prove the retired-claim scan actually bites (IC-628).

    A scan that returns "clean" is worthless unless something demonstrates it
    would have spoken up. This writes each retired claim into a temporary
    document inside the tree, checks it is caught, and removes it -- so the
    guard is tested on every run rather than trusted.
    """
    import tempfile

    failures = []
    for pattern, _why in RETIRED:
        # The sentence a stale deck would actually carry, not the regex.
        samples = {
            r"6,?400\s*x": "Krate apps are 6400x smaller.",
            r"20\s*x\s+faster": "Krate runs 20x faster.",
            r"faster\s+than\s+regular\s+applications": (
                "It is faster than regular applications."
            ),
        }
        if pattern not in samples:
            # A retired claim with no sample is an untested pattern, which is
            # the failure this whole self-test exists to prevent. Say so in
            # words rather than dying with a KeyError.
            failures.append(
                f"pattern {pattern!r} has no sample sentence in self_test(), "
                f"so nothing proves it works -- add one beside it"
            )
            continue
        sample = samples[pattern]

        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".md", dir=ROOT, delete=False
        ) as handle:
            handle.write(f"# Stale deck\n\n{sample}\n")
            temp = Path(handle.name)
        try:
            _, found = scan_retired()
            if not any(temp.name in problem for problem in found):
                failures.append(
                    f"the scan did not catch {sample!r} -- pattern {pattern!r} "
                    f"is not doing its job"
                )
        finally:
            temp.unlink()

    # And it must not fire on the corrected wording, or nobody will be able to
    # write the true sentence.
    honest = "About 19x smaller than the same program built as a native Rust binary."
    with tempfile.NamedTemporaryFile(
        mode="w", suffix=".md", dir=ROOT, delete=False
    ) as handle:
        handle.write(f"# Corrected\n\n{honest}\n")
        temp = Path(handle.name)
    try:
        _, found = scan_retired()
        if any(temp.name in problem for problem in found):
            failures.append(
                f"the scan fired on the corrected wording {honest!r}, which "
                f"would stop anyone writing the true claim"
            )
    finally:
        temp.unlink()

    # The missing-fixture rehearsals (IC-709's close_with, verbatim: "a
    # deliberate missing-fixture rehearsal"). Each proves the gate refuses
    # when its inputs are gone, rather than passing over nothing.
    if missing_fixed_surfaces(["no-such-surface.md"]) != ["no-such-surface.md"]:
        failures.append(
            "a missing fixed surface was not reported -- the gate would "
            "print OK with its most-read pages gone"
        )
    if missing_fixed_surfaces([]):
        failures.append("an empty fixed list reported phantom missing surfaces")
    try:
        load_from(ROOT / "evidence" / "claims" / "no-such-record.json")
        failures.append(
            "a missing claim record did not stop the gate -- zero evidence "
            "passed as zero drift"
        )
    except SystemExit as refusal:
        if "MISSING" not in str(refusal.code):
            failures.append(
                f"the missing-record refusal does not say what is missing: "
                f"{refusal.code!r}"
            )

    if failures:
        print("retired-claim scan self-test FAILED:\n")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(f"OK -- the retired-claim scan catches all {len(RETIRED)} withdrawn claims, and leaves the corrected wording alone.")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()

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

    missing = missing_fixed_surfaces()
    if missing:
        print("public surfaces this gate is FOR are missing from the tree:\n")
        for surface in missing:
            print(f"  {surface}")
        print(
            "\nA moved page silently leaves the scan and the OK keeps "
            "printing (K-277). If a surface was renamed, rename it here in "
            "FIXED_SURFACES in the same change."
        )
        return 1

    figures = known_figures(record)
    problems = []
    checked = 0
    for surface in SURFACES:
        if not (ROOT / surface).is_file():
            # Only the globbed extras can land here now, and a glob that
            # matched a file which then vanished mid-run is not worth dying
            # over.
            continue
        checked += 1
        problems.extend(check_surface(surface, record, figures))

    scanned, retired = scan_retired()

    if problems or retired:
        if problems:
            print(f"claim drift in {checked} surface(s):\n")
            for problem in problems:
                print(f"  {problem}")
            print(
                "\nEvery public performance number must name an evidence row. "
                "See evidence/claims/performance.json."
            )
        if retired:
            if problems:
                print()
            print(f"retired claims found in {scanned} document(s):\n")
            for problem in retired:
                print(f"  {problem}")
            print(
                "\nThese claims were checked, found wrong, and withdrawn. They "
                "do not come back. See Invest/OUTREACH_TRUTH.md."
            )
        return 1

    print(
        f"OK -- {checked} public surface(s), every performance figure backed by "
        f"the claim record; {scanned} document(s) free of retired claims."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
