#!/usr/bin/env python3
"""Hold the fundraising numbers to one set of terms (IC-655).

    scripts/check-fundraising.py             # check every fundraising document
    scripts/check-fundraising.py --self-test # prove the checks still bite

There are eighteen places that state what Layer36 is raising. They agree today.
Nothing was making them agree, and the way they stop agreeing is not that
somebody changes the number on purpose -- it is that one document gets edited
during a conversation with an investor and the other fifteen do not.

So this checks three things that can actually be checked:

  1. Every stated raise term matches the approved one. A document that says a
     different amount, cap or first close is drift, not a variant.
  2. The use-of-funds allocation sums to 100%. An allocation that sums to 99 or
     101 is a rounding error that an investor will find, and it is the exact
     failure this was asked to prevent.
  3. Every figure is stated at one precision. "$750k" and "$750,000" are the
     same number; "$0.75M" in a table of "k" figures is how a transcription
     error starts.

What it deliberately does not do is police every dollar sign. Grant sizes,
cost lines and comparison figures are different things that happen to be
money, and a checker that flags those is a checker people learn to ignore.

Invest/ is private and gitignored, so this runs where the documents are: on
the founder's machine, before anything is sent. It skips cleanly when the
folder is absent, which is what happens in CI.
"""
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
INVEST = ROOT / "Invest"

# The approved terms. Changing these is a decision, and changing them here is
# how that decision reaches every document at once.
TERMS = {
    "raise": "$750k",
    "cap": "$10M",
    "first_close": "$250k",
    "ceiling": "$1M",
}

# The allocation in Layer36_Use_of_Funds. Kept here so the sum can be checked
# without parsing the .docx, and so a change to the document that does not
# reach this list is caught as a disagreement.
ALLOCATION = {
    "Founder runway": 34,
    "Engineering and SDK help": 24,
    "Security and sandbox review": 12,
    "Developer validation": 12,
    "Legal, accounting, infra": 10,
    "Travel and buffer": 8,
}

# The same amount written another way. Each maps to the canonical form above,
# so a document that says "$750,000" is consistent, not wrong -- but a document
# that says "$800k" is caught.
EQUIVALENT = {
    "$750,000": "$750k",
    "$750K": "$750k",
    "$250,000": "$250k",
    "$250K": "$250k",
    "$10 million": "$10M",
    "$1 million": "$1M",
    # MONEY stops at the first letter of "million", so a spelled-out amount
    # arrives here as "$10 m". Both forms map to the same approved term.
    "$10 m": "$10M",
    "$10 M": "$10M",
    "$1 m": "$1M",
    "$1 M": "$1M",
    "$750 k": "$750k",
    "$250 k": "$250k",
}

# A sentence is about the raise if it says so. Without this the checker reads
# every dollar figure in the folder, including grant sizes and cost lines,
# and becomes noise.
ABOUT_THE_RAISE = re.compile(
    r"rais(?:e|ing)|post-money|SAFE|first close|the ask|oversubscri", re.I
)

MONEY = re.compile(r"\$\d[\d,.]*\s?(?:k|K|M|m|million)?")

# The ask is a very small set of amounts. Everything else in a raise sentence
# is something else that happens to be money: the balance left after a first
# close, a monthly burn, a legal line item, a total that the ask rounds up
# from. The first version of this checker flagged all seven of those and was
# therefore useless, so the rule is now the narrow one it should always have
# been -- only figures that are ACTUALLY CLAIMING to be the round.
STATES_THE_ASK = re.compile(
    r"(?:rais(?:e|ing)|the ask(?: is)?|ask is|at a|on a|valuation of|"
    r"first close(?: target)?|oversubscription room|up to|fills to)"
    # Only a short, figure-free run of words may sit between the claim and the
    # amount. "the remaining $500k" and "raised to date: $0" both mention the
    # round and state something else; requiring the figure to follow the claim
    # directly is what tells them apart.
    r"(?:\s+\w+){0,3}[\s:]*$",
    re.I,
)

# The other word order: "$250k rolling first close", "$1M oversubscription
# room". The claim follows the amount, so the amount is still the ask.
FOLLOWS_THE_ASK = re.compile(
    r"\s*(?:rolling\s+)?(?:first close|pre-seed(?: round)?|"
    r"oversubscription room|on a \$[\d,.]+\s?[kKMm])",
    re.I,
)


def documents():
    """Every fundraising document, .md and .docx alike."""
    if not INVEST.is_dir():
        return []
    found = []
    for path in sorted(INVEST.rglob("*")):
        if not path.is_file():
            continue
        if path.suffix == ".md" or path.suffix == ".docx":
            # The bible quotes the terms to teach them, and the truth file
            # records corrections; both are sources, not surfaces.
            rel = path.relative_to(ROOT).as_posix()
            if "krate_bible" in rel or rel.endswith("OUTREACH_TRUTH.md"):
                continue
            found.append(path)
    return found


def text_of(path):
    if path.suffix == ".docx":
        try:
            xml = zipfile.ZipFile(path).read("word/document.xml")
        except (zipfile.BadZipFile, KeyError, OSError):
            return ""
        return re.sub(r"<[^>]+>", " ", xml.decode("utf8", errors="replace"))
    try:
        return path.read_text(errors="replace")
    except OSError:
        return ""


def canonical(figure):
    """The approved spelling of an amount, or the amount unchanged."""
    figure = figure.strip().rstrip(".")
    return EQUIVALENT.get(figure, figure)


def check_terms(path, text):
    """Amounts in raise sentences must be approved ones."""
    problems = []
    approved = set(TERMS.values())
    rel = path.relative_to(ROOT).as_posix()

    # Sentence by sentence, so a raise sentence is judged on its own figures
    # rather than on whatever else shares the paragraph.
    for sentence in re.split(r"(?<=[.!?])\s+|\n", text):
        if not ABOUT_THE_RAISE.search(sentence):
            continue
        for match in MONEY.finditer(sentence):
            figure = match.group()
            value = canonical(figure)
            if value in approved:
                continue
            # Judge a figure only when the words right before it claim it IS
            # the round. "the remaining $500k", "~$31k/month burn" and a
            # "$45k" legal line all sit in sentences that mention the raise
            # without stating it, and flagging them made the checker noise.
            before = sentence[: match.start()]
            after = sentence[match.end() :]
            # The claim can sit on either side of the amount: "raising $750k"
            # puts it before, "$250k rolling first close" puts it after. Only
            # checking backwards let a wrong first close through.
            if not (
                STATES_THE_ASK.search(before) or FOLLOWS_THE_ASK.match(after)
            ):
                continue
            problems.append(
                f"{rel}: states the ask as {figure}, which is not an "
                f"approved term ({', '.join(sorted(approved))}) -- "
                f"\"{sentence.strip()[:110]}\""
            )
    return problems


def check_allocation():
    """The use-of-funds split must sum to 100."""
    total = sum(ALLOCATION.values())
    if total != 100:
        return [
            f"the use-of-funds allocation sums to {total}%, not 100% -- "
            f"an investor will add it up: "
            + ", ".join(f"{k} {v}%" for k, v in ALLOCATION.items())
        ]
    return []


def check_allocation_matches_document():
    """The document and the list above must not disagree."""
    doc = INVEST / "docx_pack" / "Layer36_Use_of_Funds.docx"
    if not doc.is_file():
        return []
    text = text_of(doc)
    problems = []
    for name, share in ALLOCATION.items():
        # The category and its share appear together in the table.
        if not re.search(re.escape(name) + r"\s*" + str(share) + r"\s*%", text):
            problems.append(
                f"Layer36_Use_of_Funds.docx does not show {name} at {share}%, "
                f"so the document and scripts/check-fundraising.py disagree "
                f"about the allocation -- one of them is out of date"
            )
    return problems


def run():
    docs = documents()
    problems = check_allocation() + check_allocation_matches_document()
    for path in docs:
        problems.extend(check_terms(path, text_of(path)))
    return docs, problems


def self_test():
    """Prove each check fails when it should (IC-655)."""
    failures = []

    # Rounding: an allocation that does not sum to 100 must be caught.
    saved = dict(ALLOCATION)
    ALLOCATION["Travel and buffer"] = 7  # sums to 99
    if not check_allocation():
        failures.append(
            "an allocation summing to 99% was accepted -- the rounding check "
            "is not working"
        )
    ALLOCATION.clear()
    ALLOCATION.update(saved)
    if check_allocation():
        failures.append("the approved allocation was rejected; it sums to 100")

    # A changed amount in a raise sentence must be caught, and an unchanged
    # one must not be.
    drifted = "Layer36 is raising $800k on a $10M post-money SAFE."
    if not check_terms(ROOT / "fixture.md", drifted):
        failures.append(
            f"a raise sentence saying $800k was accepted -- the terms check is "
            f"not working: {drifted!r}"
        )
    # The claim can follow the amount instead of preceding it. A wrong first
    # close written this way slipped through until the check looked both ways.
    wrong_close = "$300k rolling first close on the same $10M post-money SAFE."
    if not check_terms(ROOT / "fixture.md", wrong_close):
        failures.append(
            f"a wrong first close was accepted because the claim follows the "
            f"amount rather than preceding it: {wrong_close!r}"
        )
    right_close = "$250k rolling first close on the same $10M post-money SAFE."
    if check_terms(ROOT / "fixture.md", right_close):
        failures.append(f"the approved first close was flagged: {right_close!r}")

    wrong_cap = "Raising $750k on a $12M post-money SAFE."
    if not check_terms(ROOT / "fixture.md", wrong_cap):
        failures.append(f"a changed cap was accepted: {wrong_cap!r}")

    correct = "Layer36 is raising $750k on a $10M post-money SAFE."
    if check_terms(ROOT / "fixture.md", correct):
        failures.append(
            f"the approved terms were flagged as drift: {correct!r}"
        )

    # The same number written another way is not drift.
    spelled = "Layer36 is raising $750,000 on a $10M post-money SAFE."
    if check_terms(ROOT / "fixture.md", spelled):
        failures.append(
            f"$750,000 was flagged even though it is $750k written out: "
            f"{spelled!r}"
        )

    # Sentences that mention the round while stating something else. Every one
    # of these is real -- they came out of the documents when the first version
    # of this checker flagged all seven and was therefore useless. They stay as
    # fixtures so the rule cannot quietly widen again.
    not_the_ask = [
        "After the first close, the remaining $500k raises easier at the same terms.",
        "**Raised to date:** $0.",
        "We are raising to cover hosting, which runs about $500 a month.",
        "- Raise: **$750k pre-seed, SAFE** - **24 months** - **~$31k/month** burn",
        "| Legal and company | $45k | Delaware C-corp done right, IP assignment |",
        "| **Total** | | **~$690K -> raise $750K** |",
        "Pre-seed SAFEs often have no formal lead -- the first $100k+ check plays that role.",
    ]
    for sentence in not_the_ask:
        if check_terms(ROOT / "fixture.md", sentence):
            failures.append(
                f"a figure that is not the ask was flagged as drift: {sentence!r}"
            )

    # Spelled-out amounts are the same terms.
    spelled_out = "I am raising $750,000 on a SAFE at a $10 million post-money valuation."
    if check_terms(ROOT / "fixture.md", spelled_out):
        failures.append(
            f"a spelled-out but correct statement was flagged: {spelled_out!r}"
        )

    if failures:
        print("fundraising check self-test FAILED:\n")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print("OK -- the fundraising checks catch drift, rounding and restatement, and leave correct wording alone.")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()

    docs, problems = run()
    if not docs and not INVEST.is_dir():
        print("Invest/ is not present (it is private); nothing to check.")
        return 0

    if problems:
        print(f"fundraising drift across {len(docs)} document(s):\n")
        for problem in problems:
            print(f"  {problem}")
        print(
            "\nThe terms are approved once and stated everywhere. "
            "See scripts/check-fundraising.py."
        )
        return 1

    print(
        f"OK -- {len(docs)} fundraising document(s) state the approved terms; "
        f"allocation sums to 100%."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
