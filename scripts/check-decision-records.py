#!/usr/bin/env python3
"""Decision records, maturity labels and the approval rule (IC-663).

The ADR directory publishes a template, a status vocabulary, an index and a
process. Nothing checked that a record follows them, that the two indexes
agree with the files, that a superseded record points back at its
successor, or that the process could be followed at all -- it asked for two
approvals from a project with one maintainer, which is a rule nobody can
obey and everybody therefore ignores.

What is enforced, and on what
-----------------------------
Merged ADRs are immutable by their own rule, so a check cannot demand that
old records grow new sections. So there are two bars:

  * every record, old or new: the front matter the template names, a
    status from the published vocabulary, an ISO date, an author, and
    supersession links that name a record which exists and points back;
  * records dated on or after the day this check landed (2026-09-13): every
    template section, including the compatibility effect test 1290 asks
    for. Older records missing a section are reported as findings so the
    gap is visible, and never fail the build for a file that may not be
    edited.

The two indexes (docs/adr/README.md and the book's ADR page) must list every
record with the status the record itself carries.

RFCs carry a maturity label. Draft and Experimental need nothing. Candidate
must record review requests and implementation gaps (1279). Candidate and
Stable must name evidence records that exist in the registry (1278). A
sentence calling Krate "a standard" fails unless a named external standards
process completed, and none has (1280).

The approval rule is read from docs/governance/maintainers.json and checked
against the registry it sits beside: a rule the registry cannot satisfy is
refused unless the founder-stage rule is active and published on the
process pages (1281-1282). Appointment and removal are dated history
entries, never edits to the list (1289).

  python3 scripts/check-decision-records.py
  python3 scripts/check-decision-records.py --self-test
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ADR_DIR = ROOT / "docs" / "adr"
RFC_DIR = ROOT / "docs" / "rfc"
BOOK_INDEX = ROOT / "docs" / "book" / "src" / "contributing" / "adrs.md"
MAINTAINERS = ROOT / "docs" / "governance" / "maintainers.json"
REGISTRY_RECORDS = ROOT / "evidence" / "registry" / "records"
REGISTRY_CLAIMS = ROOT / "evidence" / "registry" / "claims"
PROCESS_PAGES = [ROOT / "docs" / "adr" / "README.md", BOOK_INDEX, ROOT / "CONTRIBUTING.md"]
STANDARD_SURFACES = ["docs/adr", "docs/rfc", "README.md", "docs/landing", "docs/book/src"]

# The day this check landed. Records from before it are immutable and are
# held to the front-matter bar only.
NEW_RECORDS_FROM = "2026-09-13"

ADR_STATUSES = {"Proposed", "Accepted", "Superseded", "Rejected", "Withdrawn"}
RFC_STATUSES = {"Draft", "Experimental", "Candidate", "Stable", "Withdrawn"}
ISO_DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
ADR_REF = re.compile(r"ADR-(\d{4})")
PLACEHOLDER = re.compile("^(-{1,2}|\\*\\(if applicable\\)\\*|\u2014|)$")

# What the template requires of an ADR, as section headings (matched
# case-insensitively on the heading text).
ADR_SECTIONS = ["Context", "Decision", "Alternatives considered", "Consequences", "Revisiting", "References"]
# Asked for by 1290 and added to the template with this check.
ADR_SECTIONS_NEW = ADR_SECTIONS + ["Compatibility"]

STANDARD_CLAIM = re.compile(
    r"\bKrate\b[^.\n]{0,80}\b(is|are|became|becomes|as)\s+(an?\s+)?(open|industry|de[\s-]facto|official)?\s*standard\b",
    re.I,
)
NEGATED = re.compile(r"\b(not|no|never|isn'?t|aren'?t|rather than|instead of)\b", re.I)


def front_matter(text):
    """`**Key:** value` lines above the first rule."""
    out = {}
    head = text.split("\n---", 1)[0]
    for m in re.finditer(r"^\*\*([A-Za-z ]+):\*\*\s*(.*?)\s*$", head, re.M):
        out[m.group(1).strip()] = m.group(2).strip()
    return out


def sections(text):
    return {m.group(1).strip().lower() for m in re.finditer(r"^##\s+(.+?)\s*$", text, re.M)}


def section_body(text, name):
    m = re.search(rf"^##\s+{re.escape(name)}\s*$(.*?)(?=^##\s|\Z)", text, re.M | re.S | re.I)
    return m.group(1).strip() if m else ""


def adr_number(path):
    m = re.match(r"(\d{4})-", path.name)
    return m.group(1) if m else None


def load_adrs(adr_dir=ADR_DIR):
    return {adr_number(p): p for p in sorted(adr_dir.glob("*.md")) if adr_number(p)}


def check_adr(number, path, adrs, today_new=NEW_RECORDS_FROM):
    """(failures, findings) for one ADR."""
    fails, finds = [], []
    text = path.read_text(errors="replace")
    fm = front_matter(text)
    name = path.name

    status_raw = fm.get("Status", "")
    status = status_raw.split(" ")[0] if status_raw else ""
    if status not in ADR_STATUSES:
        fails.append(f"{name}: Status {status_raw!r} is not one of {sorted(ADR_STATUSES)}")
    date = fm.get("Date", "")
    if not ISO_DATE.match(date):
        fails.append(f"{name}: Date {date!r} is not YYYY-MM-DD")
    if not fm.get("Authors") and not fm.get("Author"):
        fails.append(f"{name}: no Authors line")

    # Supersession must name a record that exists and points back (1290).
    for key, back_key, back_word in (("Supersedes", "Superseded by", "superseded by"), ("Superseded by", "Supersedes", "supersedes")):
        value = fm.get(key, "")
        if PLACEHOLDER.match(value):
            continue
        refs = ADR_REF.findall(value)
        if not refs:
            # Prose supersession of something that is not an ADR (a plan
            # section) is allowed but noted: it cannot be checked both ways.
            finds.append(f"{name}: {key} names something that is not an ADR ({value[:60]!r}); the link cannot be checked both ways")
            continue
        for ref in refs:
            other = adrs.get(ref)
            if other is None:
                fails.append(f"{name}: {key} ADR-{ref}, which does not exist")
                continue
            other_fm = front_matter(other.read_text(errors="replace"))
            if number not in ADR_REF.findall(other_fm.get(back_key, "")):
                fails.append(f"{name}: {key} ADR-{ref}, but ADR-{ref} does not say it is {back_word} ADR-{number}")
        if key == "Superseded by" and status != "Superseded":
            fails.append(f"{name}: is superseded by {value} but its Status is {status!r}, not Superseded")

    have = sections(text)
    required = ADR_SECTIONS_NEW if date >= today_new else ADR_SECTIONS
    missing = [s for s in required if s.lower() not in have]
    if missing:
        if date >= today_new:
            fails.append(f"{name}: missing section(s) the template requires: {', '.join(missing)}")
        else:
            finds.append(f"{name}: pre-{today_new} record without {', '.join(missing)}; immutable, so recorded rather than required")
    decision = section_body(text, "Decision")
    if decision and not re.search(r"\bWe will\b", decision):
        finds.append(f"{name}: the Decision does not start with the template's \"We will\"")
    if date >= today_new and not section_body(text, "Compatibility"):
        fails.append(f"{name}: Compatibility section is empty; say what breaks, what migrates, or that nothing does (1290)")
    return fails, finds


def check_indexes(adrs, readme=None, book=None):
    """Both indexes list every record with the status the record carries."""
    fails = []
    readme = readme if readme is not None else (ADR_DIR / "README.md").read_text(errors="replace")
    book = book if book is not None else (BOOK_INDEX.read_text(errors="replace") if BOOK_INDEX.exists() else "")
    for number, path in adrs.items():
        status = front_matter(path.read_text(errors="replace")).get("Status", "").split(" ")[0]
        for label, text in (("docs/adr/README.md", readme), ("the book's ADR page", book)):
            row = next((l for l in text.splitlines() if re.search(rf"\b{number}\b", l) and l.startswith("|")), None)
            if row is None:
                fails.append(f"{label}: does not list ADR-{number}")
            elif status and status not in row:
                fails.append(f"{label}: lists ADR-{number} as something other than {status}: {row.strip()[:80]}")
    return fails


def registry_ids():
    ids = set()
    for d in (REGISTRY_RECORDS, REGISTRY_CLAIMS):
        if d.is_dir():
            ids |= {p.stem for p in d.glob("*.json")}
    return ids


def check_rfc(path, ids):
    fails = []
    text = path.read_text(errors="replace")
    fm = front_matter(text)
    status = fm.get("Status", "").split(" ")[0]
    name = path.name
    if status not in RFC_STATUSES:
        fails.append(f"{name}: Status {status!r} is not one of {sorted(RFC_STATUSES)} (1278)")
        return fails
    have = sections(text)
    if status in ("Candidate", "Stable"):
        evidence = section_body(text, "Evidence")
        cited = set(re.findall(r"\b([EC]-[A-Za-z0-9.-]{4,})\b", evidence))
        if not cited:
            fails.append(f"{name}: {status} needs an Evidence section naming registry records; a label without evidence is a claim (1278)")
        for ref in sorted(cited - ids):
            fails.append(f"{name}: cites {ref}, which is not in the evidence registry (1278)")
    if status == "Candidate":
        for needed in ("review requests", "implementation gaps"):
            if needed not in have:
                fails.append(f"{name}: Candidate must record {needed} (1279)")
    return fails


def check_standard_claims(root=ROOT, surfaces=STANDARD_SURFACES, completed_processes=()):
    """Calling Krate a standard is refused until an external process completed (1280)."""
    fails = []
    for rel in surfaces:
        base = root / rel
        files = [base] if base.is_file() else (list(base.rglob("*.md")) + list(base.rglob("*.html")) if base.is_dir() else [])
        for path in files:
            try:
                text = path.read_text(errors="replace")
            except OSError:
                continue
            for line_no, line in enumerate(text.splitlines(), 1):
                m = STANDARD_CLAIM.search(line)
                if m and not NEGATED.search(line[: m.start()]) and not completed_processes:
                    fails.append(f"{path.relative_to(root).as_posix()}:{line_no}: calls Krate a standard; no external standards process has completed (1280): {line.strip()[:80]}")
    return fails


def check_approval_rule(registry, pages):
    """A rule the registry cannot satisfy is refused unless the founder-stage
    rule is active and published (1281-1282); history is dated (1289)."""
    fails = []
    active = [m for m in registry.get("maintainers", []) if m.get("active")]
    rule = registry.get("approval_rule", {})
    need = int(rule.get("required_approvals", 0))
    founder = rule.get("founder_stage", {})
    for m in registry.get("maintainers", []):
        for field in ("handle", "since", "appointed_by"):
            if not m.get(field):
                fails.append(f"maintainer {m.get('handle', '?')}: {field} is required (1289)")
    for h in registry.get("history", []):
        for field in ("handle", "date", "change", "reason"):
            if not h.get(field):
                fails.append(f"history entry {h}: {field} is required (1289)")
    if not active:
        fails.append("no active maintainer is registered; nothing can be approved")
    if need > len(active):
        if not founder.get("active"):
            fails.append(f"the rule needs {need} approvals and {len(active)} maintainer(s) are active: nobody can follow it, and the founder-stage rule is not active (1282)")
        else:
            for label, text in pages:
                if "founder stage" not in text.lower() and "founder-stage" not in text.lower():
                    fails.append(f"{label}: demands {need} approvals but does not publish the founder-stage rule the registry says applies (1281)")
    return fails


def cmd_check():
    fails, finds = [], []
    adrs = load_adrs()
    for number, path in adrs.items():
        f, n = check_adr(number, path, adrs)
        fails += f
        finds += n
    fails += check_indexes(adrs)
    ids = registry_ids()
    for path in sorted(RFC_DIR.glob("*.md")) if RFC_DIR.is_dir() else []:
        fails += check_rfc(path, ids)
    fails += check_standard_claims()
    if MAINTAINERS.is_file():
        registry = json.loads(MAINTAINERS.read_text())
        pages = [(p.relative_to(ROOT).as_posix(), p.read_text(errors="replace")) for p in PROCESS_PAGES if p.exists()]
        fails += check_approval_rule(registry, pages)
    else:
        fails.append(f"{MAINTAINERS.relative_to(ROOT)} is missing: the approval rule has nothing to be checked against")

    for line in finds:
        print(f"  finding: {line}")
    if fails:
        print(f"\n{len(fails)} problem(s) with the decision records:", file=sys.stderr)
        for line in fails:
            print(f"  {line}", file=sys.stderr)
        return 1
    print(f"OK -- {len(adrs)} ADRs and {len(list(RFC_DIR.glob('*.md')))} RFC(s) follow the published template, both indexes agree, "
          "supersession links resolve, no standard claim stands, and the approval rule can be followed")
    return 0


# ------------------------------------------------------------------ self-test


def _adr(number, status="Accepted", date="2026-05-04", supersedes="-", superseded_by="-", sections_=None, decision="We will do the thing."):
    secs = sections_ if sections_ is not None else ADR_SECTIONS
    body = "\n".join(f"## {s}\n\n{decision if s == 'Decision' else 'text'}\n" for s in secs)
    return (f"# ADR-{number}: T\n\n**Status:** {status}  \n**Date:** {date}  \n**Authors:** @x  \n"
            f"**Supersedes:** {supersedes}  \n**Superseded by:** {superseded_by}\n\n---\n\n{body}")


def self_test():
    import tempfile
    failures = []

    def check(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp)
        (d / "0001-a.md").write_text(_adr("0001", status="Superseded", superseded_by="ADR-0002"))
        (d / "0002-b.md").write_text(_adr("0002", supersedes="ADR-0001"))
        (d / "0003-c.md").write_text(_adr("0003", sections_=["Context", "Decision"]))  # old, incomplete
        (d / "0004-d.md").write_text(_adr("0004", date="2026-09-20", sections_=ADR_SECTIONS))  # new, old bar only
        (d / "0010-j.md").write_text(_adr("0010", date="2026-09-20", sections_=ADR_SECTIONS_NEW).replace("## Compatibility\n\ntext", "## Compatibility\n\n"))
        (d / "0005-e.md").write_text(_adr("0005", date="2026-09-20", sections_=ADR_SECTIONS_NEW))
        (d / "0006-f.md").write_text(_adr("0006", supersedes="ADR-0099"))
        (d / "0007-g.md").write_text(_adr("0007", superseded_by="ADR-0002"))  # 0002 does not point back; status still Accepted
        (d / "0008-h.md").write_text(_adr("0008", status="Maybe", date="May 4"))
        (d / "0009-i.md").write_text(_adr("0009", decision="The thing is done."))
        adrs = load_adrs(d)

        f, n = check_adr("0001", adrs["0001"], adrs); check("a superseded record pointing at its successor passes", f == [], f"{f}")
        f, n = check_adr("0002", adrs["0002"], adrs); check("and the successor pointing back passes", f == [], f"{f}")
        f, n = check_adr("0003", adrs["0003"], adrs)
        check("an OLD incomplete record is a finding, not a failure (immutable)", f == [] and any("immutable" in x for x in n), f"{f} {n}")
        f, n = check_adr("0004", adrs["0004"], adrs)
        check("a NEW record with only the old sections fails on the section list", any("missing section" in x and "Compatibility" in x for x in f), f"{f}")
        f, n = check_adr("0010", adrs["0010"], adrs)
        check("a NEW record whose Compatibility section is empty fails", any("Compatibility section is empty" in x for x in f), f"{f}")
        f, n = check_adr("0005", adrs["0005"], adrs); check("a new complete record passes", f == [], f"{f}")
        f, n = check_adr("0006", adrs["0006"], adrs); check("superseding a record that does not exist fails", any("does not exist" in x for x in f), f"{f}")
        f, n = check_adr("0007", adrs["0007"], adrs)
        check("a one-way supersession link fails", any("does not say it is" in x for x in f), f"{f}")
        check("superseded-by with Status Accepted fails", any("not Superseded" in x for x in f), f"{f}")
        f, n = check_adr("0008", adrs["0008"], adrs)
        check("a status outside the vocabulary fails", any("Status" in x for x in f), f"{f}")
        check("a non-ISO date fails", any("Date" in x for x in f), f"{f}")
        f, n = check_adr("0009", adrs["0009"], adrs)
        check("a Decision without 'We will' is a finding", any("We will" in x for x in n), f"{n}")

        readme = "| 0001 | a | Superseded |\n| 0002 | b | Accepted |\n"
        book = "| [ADR-0001](x) | a | Superseded |\n| [ADR-0002](x) | b | Proposed |\n"
        two = {k: adrs[k] for k in ("0001", "0002")}
        f = check_indexes(two, readme=readme, book=book)
        check("an index with the wrong status fails", any("something other than Accepted" in x for x in f), f"{f}")
        f = check_indexes({"0001": adrs["0001"], "0002": adrs["0002"], "0003": adrs["0003"]}, readme=readme, book=book)
        check("an index missing a record fails", any("does not list ADR-0003" in x for x in f), f"{f}")

        # RFC maturity (1278-1279)
        r = d / "rfc"; r.mkdir()
        (r / "0001-x.md").write_text("# RFC\n\n**Status:** Draft  \n**Date:** 2026-01-01  \n\n---\n\n## Summary\n\nx\n")
        check("a Draft RFC needs nothing", check_rfc(r / "0001-x.md", set()) == [])
        (r / "0002-y.md").write_text("# RFC\n\n**Status:** Stable  \n\n---\n\n## Summary\n\nx\n")
        check("a Stable RFC without evidence fails", any("1278" in x for x in check_rfc(r / "0002-y.md", set())))
        (r / "0003-z.md").write_text("# RFC\n\n**Status:** Stable  \n\n---\n\n## Evidence\n\nSee E-REAL and E-FAKE.\n")
        f = check_rfc(r / "0003-z.md", {"E-REAL"})
        check("a Stable RFC citing an unknown record fails and names it", any("E-FAKE" in x for x in f) and not any("E-REAL" in x for x in f), f"{f}")
        (r / "0004-w.md").write_text("# RFC\n\n**Status:** Candidate  \n\n---\n\n## Evidence\n\nE-REAL\n")
        f = check_rfc(r / "0004-w.md", {"E-REAL"})
        check("a Candidate without review requests and gaps fails", sum("1279" in x for x in f) == 2, f"{f}")
        (r / "0005-v.md").write_text("# RFC\n\n**Status:** Mature  \n\n---\n")
        check("a maturity label outside the vocabulary fails", any("1278" in x for x in check_rfc(r / "0005-v.md", set())))

        # Standard claims (1280)
        s = d / "surf"; s.mkdir()
        (s / "a.md").write_text("Krate is an open standard for apps.\nIt is not true that Krate is an open standard.\nThe WIT standard is used.\n")
        f = check_standard_claims(root=d, surfaces=["surf"])
        check("calling Krate a standard fails", any("a.md:1" in x for x in f), f"{f}")
        check("the negation does not", not any("a.md:2" in x for x in f), f"{f}")
        check("mentioning a standard Krate uses does not", not any("a.md:3" in x for x in f), f"{f}")
        check("and it passes once an external process is recorded as completed",
              check_standard_claims(root=d, surfaces=["surf"], completed_processes=("W3C",)) == [])

        # Approval rule (1281-1282, 1289)
        one = {"maintainers": [{"handle": "@a", "since": "2026-01-01", "appointed_by": "founding", "active": True}],
               "history": [], "approval_rule": {"required_approvals": 2, "founder_stage": {"active": False}}}
        f = check_approval_rule(one, [("page", "Minimum 2 maintainers approve.")])
        check("two approvals with one maintainer and no founder rule fails", any("1282" in x for x in f), f"{f}")
        one["approval_rule"]["founder_stage"] = {"active": True}
        f = check_approval_rule(one, [("page", "Minimum 2 maintainers approve.")])
        check("founder rule active but unpublished on the page fails", any("1281" in x for x in f), f"{f}")
        f = check_approval_rule(one, [("page", "Minimum 2 approve. Founder stage: the accountable maintainer records their own review.")])
        check("published founder rule passes", f == [], f"{f}")
        two_m = dict(one); two_m["maintainers"] = one["maintainers"] + [{"handle": "@b", "since": "2026-02-01", "appointed_by": "@a", "active": True}]
        two_m["approval_rule"] = {"required_approvals": 2, "founder_stage": {"active": False}}
        check("two maintainers satisfy two approvals with no founder rule", check_approval_rule(two_m, [("page", "x")]) == [])
        bad = dict(one); bad["history"] = [{"handle": "@z", "date": "2026-03-01", "change": "removed"}]
        check("a history entry without a reason fails (1289)", any("reason" in x for x in check_approval_rule(bad, [("page", "founder stage")])))

    if failures:
        print("check-decision-records self-test FAILED:\n")
        for x in failures:
            print(f"  - {x}")
        return 1
    print("check-decision-records self-test OK -- template sections required of new records and reported on immutable ones, "
          "supersession checked both ways, indexes must agree, maturity labels need evidence, standard claims are refused, "
          "and an approval rule the registry cannot satisfy is refused unless the founder-stage rule is published")
    return 0


def main(argv):
    if "--self-test" in argv:
        return self_test()
    return cmd_check()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
