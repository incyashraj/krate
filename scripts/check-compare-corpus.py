#!/usr/bin/env python3
"""The comparison corpus and the wording it permits (IC-822).

Krate publishes numbers against other software. Every such number rests on
a comparison, and a comparison is only as honest as the answer to "is the
thing on the other side the same kind of thing?" For the notes benchmark
the answer is no: MarkText is a mature editor and the Krate replica does
the measured workflow and nothing else. The lab note says so in prose. This
makes it a record a check can read.

What the corpus records, per compared app
-----------------------------------------
  * the comparator: name, version, licence, where it came from, when it
    was released and whether it is still maintained -- each verified from
    the source named, with the date it was verified (1748);
  * the common required features, each marked present or absent on each
    side, and the optional features and known differences (1742);
  * the correctness cells a run must carry before its performance numbers
    mean anything: content on screen, input not dropped (1744-1746);
  * the stratum, so a notes number is never quoted as a games number
    (1747);
  * the wording the corpus permits: `equivalent-product` only when every
    common required feature is present on both sides, else
    `workload-matched` (1743, 1813).

What the check enforces
-----------------------
  * the record is well formed and sealed; an edit without a re-seal fails;
  * the permitted wording is COMPUTED from the feature table and must match
    what the record claims for itself, so nobody can hand-set
    "equivalent-product" over an absent feature;
  * the phrases "equivalent product", "feature parity", "the same product"
    and "the same app as <comparator>" may not appear in any live document
    unless the corpus permits equivalent-product wording for that
    comparator. "The same app" alone is not gated: it is the portability
    sentence -- one file, three operating systems -- and has nothing to do
    with comparisons;
  * the profile each app names must carry the correctness cells the
    corpus requires. Today it does not, and that is reported as a standing
    finding rather than hidden or quietly fixed: the profile is frozen and
    sealed, and adding cells to it is a new profile version with a new run.

  python3 scripts/check-compare-corpus.py          # validate, gate, report
  python3 scripts/check-compare-corpus.py --seal   # re-seal after an edit
  python3 scripts/check-compare-corpus.py --self-test
"""

import hashlib
import importlib.util
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "evidence" / "compare" / "corpus.json"
PROFILES = ROOT / "evidence" / "registry" / "profiles"
SCHEMA = "krate.compare.corpus.v1"

STRATA = {"notes", "editor", "network", "media", "graphics-game", "service", "data-heavy"}
PRESENCE = {"present", "absent"}
WORDINGS = {"equivalent-product", "workload-matched"}
ISO = re.compile(r"^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}Z)?$")

# Phrases that claim more than a workload match. A phrase is a CLAIM only
# when the comparator is named on the same line and nothing on that line
# negates it -- "does not claim feature parity with MarkText" is the rule
# being stated, not broken. The first version gated the bare phrase and
# found 37 hits, every one a negation, a plan item about Krate's own
# releases, a rule-stating file, or a worktree copy of one. A gate that
# fires on the sentence forbidding the thing is not a gate.
GATED = [
    (r"equivalent\s+product", "equivalent product"),
    (r"feature[\s-]+parity", "feature parity"),
    (r"the\s+same\s+product", "the same product"),
    (r"(the\s+)?same\s+app(lication)?\s+as\b", "the same app as the comparator"),
]
NEGATED = re.compile(
    r"\b(not|no|never|without|nor|rather than|instead of|isn'?t|aren'?t|doesn'?t|don'?t|cannot|can'?t)\b",
    re.I,
)
SCAN_SUFFIXES = (".md", ".tex", ".html", ".txt")
# Beyond check-claims' skips: agent worktrees are copies, and benchmark run
# directories are machine output that quotes the rule to enforce it.
EXTRA_SKIP_PARTS = (".claude/", "/runs/")


def _claims_module():
    """check-claims.py's skip lists, so the two scans agree on what is live."""
    spec = importlib.util.spec_from_file_location("check_claims", ROOT / "scripts" / "check-claims.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def seal_of(record):
    body = {k: v for k, v in record.items() if k != "seal"}
    return hashlib.sha256(json.dumps(body, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def permitted_wording(app):
    """Computed, never hand-set: every common required feature present on
    both sides, or the claim is workload-matched."""
    features = app.get("common_required_features") or []
    if not features:
        return "workload-matched"
    both = all(f.get("krate") == "present" and f.get("comparator") == "present" for f in features)
    return "equivalent-product" if both else "workload-matched"


def validate(record, profiles_dir=PROFILES):
    """Problems with the record itself. Empty means well formed."""
    p = []
    if record.get("schema") != SCHEMA:
        p.append(f"schema must be {SCHEMA}")
    apps = record.get("apps")
    if not isinstance(apps, list) or not apps:
        return p + ["apps must be a non-empty list"]
    if record.get("seal") != seal_of(record):
        p.append("the seal does not match the content: re-seal after an edit with --seal")
    seen = set()
    for app in apps:
        ident = app.get("id", "?")
        pre = f"app {ident}"
        if ident in seen:
            p.append(f"{pre}: listed twice")
        seen.add(ident)
        if app.get("stratum") not in STRATA:
            p.append(f"{pre}: stratum must be one of {sorted(STRATA)} (1747)")
        comp = app.get("comparator") or {}
        for field in ("name", "version", "licence", "release_origin", "released_at", "support_status", "verified_at", "verified_from"):
            if not comp.get(field):
                p.append(f"{pre}: comparator.{field} is required (1748)")
        for field in ("released_at", "verified_at"):
            if comp.get(field) and not ISO.match(str(comp[field])):
                p.append(f"{pre}: comparator.{field} must be an ISO date")
        feats = app.get("common_required_features")
        if not isinstance(feats, list) or not feats:
            p.append(f"{pre}: common_required_features must list what both sides must do (1742)")
        else:
            for f in feats:
                if not f.get("feature"):
                    p.append(f"{pre}: a feature row has no name")
                for side in ("krate", "comparator"):
                    if f.get(side) not in PRESENCE:
                        p.append(f"{pre}: feature {f.get('feature')!r} must mark {side} as present or absent")
        for field in ("known_differences", "human_review_tasks", "journeys", "required_correctness_cells"):
            if not isinstance(app.get(field), list) or not app.get(field):
                p.append(f"{pre}: {field} must be a non-empty list")
        claimed = app.get("permitted_wording")
        if claimed not in WORDINGS:
            p.append(f"{pre}: permitted_wording must be one of {sorted(WORDINGS)}")
        elif isinstance(feats, list) and feats and claimed != permitted_wording(app):
            p.append(
                f"{pre}: permitted_wording says {claimed!r} but the feature table computes "
                f"{permitted_wording(app)!r} -- an absent common feature cannot be wished away (1743)"
            )
        if not app.get("profile"):
            p.append(f"{pre}: profile must name the measurement profile the numbers come from")
    return p


def correctness_findings(record, profiles_dir=PROFILES):
    """Profiles missing the correctness cells the corpus requires (1744-1746).

    A finding, not a failure: the profile is frozen and sealed, so the fix is
    a new profile version and a new run, which this cannot do. What it can do
    is refuse to let the gap be forgotten."""
    out = []
    for app in record.get("apps", []):
        path = profiles_dir / f"{app.get('profile')}.json"
        if not path.is_file():
            out.append(f"{app.get('id')}: profile {app.get('profile')} does not exist")
            continue
        try:
            cells = {c.get("name") for c in json.loads(path.read_text()).get("cells", [])}
        except (json.JSONDecodeError, AttributeError):
            out.append(f"{app.get('id')}: profile {app.get('profile')} is unreadable")
            continue
        missing = [c for c in app.get("required_correctness_cells", []) if c not in cells]
        if missing:
            out.append(
                f"{app.get('id')}: profile {app.get('profile')} has no {', '.join(missing)} cell, "
                "so its performance numbers are not gated on correctness (1744-1746); "
                "the next profile version must carry them"
            )
    return out


def gated_phrases(record, root=ROOT, skip_parts=None, skip_files=None):
    """Live documents using wording the corpus does not permit (1813)."""
    if skip_parts is None or skip_files is None:
        claims = _claims_module()
        skip_parts = skip_parts or claims.RETIRED_SKIP_PARTS
        skip_files = skip_files or claims.RETIRED_SKIP_FILES
    allowed = {
        a["comparator"]["name"].lower()
        for a in record.get("apps", [])
        if permitted_wording(a) == "equivalent-product"
    }
    names = [a["comparator"]["name"] for a in record.get("apps", []) if a.get("comparator", {}).get("name")]
    compiled = [(re.compile(pattern, re.I), label) for pattern, label in GATED]

    problems, scanned = [], 0
    here = Path(__file__).resolve()
    self_rel = here.relative_to(root).as_posix() if here.is_relative_to(root) else None
    corpus_rel = CORPUS.relative_to(root).as_posix() if CORPUS.is_relative_to(root) else None
    for path in root.rglob("*"):
        if not path.is_file() or path.suffix not in SCAN_SUFFIXES:
            continue
        rel = path.relative_to(root).as_posix()
        if any(part in rel for part in skip_parts) or rel in skip_files:
            continue
        if any(part in rel for part in EXTRA_SKIP_PARTS):
            continue
        if rel in (self_rel, corpus_rel, "evidence/benchmarks/marktext-vs-krate/CLAIMS.md"):
            # The files that name the phrases in order to forbid them.
            continue
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        scanned += 1
        for line_no, line in enumerate(text.splitlines(), 1):
            low = line.lower()
            named = [n for n in names if n.lower() in low]
            if not named:
                continue  # not about any comparator: the portability or plan sense
            for regex, label in compiled:
                m = regex.search(line)
                if not m:
                    continue
                if NEGATED.search(line[: m.start()]):
                    continue  # the rule being stated, not broken
                if all(n.lower() in allowed for n in named):
                    continue  # the corpus permits it for every comparator named
                problems.append(
                    f"{rel}:{line_no}: {line.strip()[:90]}\n      says \"{label}\", which the "
                    "comparison corpus does not permit: a common required feature is absent on "
                    "one side, so the honest wording is workload-matched (IC-822, 1813)"
                )
    return scanned, problems


def cmd_check():
    if not CORPUS.is_file():
        print(f"{CORPUS.relative_to(ROOT)} is missing; nothing gates comparison wording. This is not a pass.", file=sys.stderr)
        return 2
    record = json.loads(CORPUS.read_text())
    problems = validate(record)
    if problems:
        print("the comparison corpus is not valid:", file=sys.stderr)
        for line in problems:
            print(f"  {line}", file=sys.stderr)
        return 1
    scanned, wording = gated_phrases(record)
    for app in record["apps"]:
        print(f"{app['id']}: {permitted_wording(app)} wording permitted "
              f"({sum(1 for f in app['common_required_features'] if f['krate'] == 'absent')} common features absent on the Krate side)")
    for line in correctness_findings(record):
        print(f"  standing finding: {line}")
    if wording:
        print(f"\n{len(wording)} document(s) claim more than the corpus permits:", file=sys.stderr)
        for line in wording:
            print(f"  {line}", file=sys.stderr)
        return 1
    print(f"OK -- {scanned} live documents scanned, none claims equivalence the corpus does not permit")
    return 0


def cmd_seal():
    record = json.loads(CORPUS.read_text())
    record["seal"] = seal_of(record)
    CORPUS.write_text(json.dumps(record, indent=1) + "\n")
    print(f"sealed {CORPUS.relative_to(ROOT)}: {record['seal'][:16]}...")
    return 0


# ------------------------------------------------------------------ self-test


def _app(**over):
    base = {
        "id": "t-vs-x", "stratum": "notes",
        "comparator": {"name": "Xed", "version": "1.0", "licence": "MIT", "release_origin": "https://x/r",
                       "released_at": "2020-01-01", "support_status": "maintained", "verified_at": "2026-09-13",
                       "verified_from": "test"},
        "common_required_features": [
            {"feature": "open", "krate": "present", "comparator": "present"},
            {"feature": "scroll", "krate": "present", "comparator": "present"},
        ],
        "known_differences": ["none"], "human_review_tasks": ["look"], "journeys": ["open"],
        "profile": "P-T", "required_correctness_cells": ["content-readiness"],
        "permitted_wording": "equivalent-product",
    }
    base.update(over)
    return base


def _record(*apps):
    rec = {"schema": SCHEMA, "apps": list(apps), "seal": ""}
    rec["seal"] = seal_of(rec)
    return rec


def self_test():
    import tempfile
    failures = []

    def check(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        profiles = tmp / "profiles"; profiles.mkdir()
        (profiles / "P-T.json").write_text(json.dumps({"cells": [{"name": "open"}]}))

        good = _record(_app())
        check("a sound record validates", validate(good, profiles) == [], f"{validate(good, profiles)}")
        check("all-present computes equivalent-product", permitted_wording(_app()) == "equivalent-product")

        # 1743: one absent common feature and the wording drops to workload-matched.
        partial = _app(common_required_features=[
            {"feature": "open", "krate": "present", "comparator": "present"},
            {"feature": "tabs", "krate": "absent", "comparator": "present"},
        ], permitted_wording="workload-matched")
        check("one absent feature computes workload-matched", permitted_wording(partial) == "workload-matched")
        lying = dict(partial, permitted_wording="equivalent-product")
        probs = validate(_record(lying), profiles)
        check("a hand-set equivalent-product over an absent feature is refused",
              any("cannot be wished away" in x for x in probs), f"{probs}")

        # The seal binds the content.
        tampered = _record(_app()); tampered["apps"][0]["comparator"]["version"] = "9.9"
        check("an edit without a re-seal is refused", any("seal" in x for x in validate(tampered, profiles)))

        # 1748: every comparator fact is required.
        for field in ("licence", "release_origin", "released_at", "support_status", "verified_at"):
            comp = dict(_app()["comparator"]); comp.pop(field)
            probs = validate(_record(_app(comparator=comp)), profiles)
            check(f"a comparator without {field} is refused", any(field in x for x in probs), f"{probs}")
        check("a stratum outside the seven is refused",
              any("stratum" in x for x in validate(_record(_app(stratum="misc")), profiles)))
        check("a feature side that is neither present nor absent is refused",
              any("present or absent" in x for x in validate(_record(_app(common_required_features=[
                  {"feature": "open", "krate": "mostly", "comparator": "present"}])), profiles)))

        # 1744-1746: a profile without the correctness cells is a standing finding.
        finds = correctness_findings(good, profiles)
        check("a profile missing content-readiness is a finding", any("content-readiness" in x for x in finds), f"{finds}")
        (profiles / "P-T.json").write_text(json.dumps({"cells": [{"name": "open"}, {"name": "content-readiness"}]}))
        check("and none once the cell exists", correctness_findings(good, profiles) == [])

        # 1813: the wording gate over live documents.
        docs = tmp / "docs"; docs.mkdir()
        (docs / "a.md").write_text(
            "Krate reaches feature parity with Xed.\n"
            "The same app runs on three OSes.\n"
            "It does not claim feature parity with Xed.\n"
            "Feature parity with current production is a release goal.\n"
        )
        (docs / "b.md").write_text("On the same workload, Krate used less memory than Xed.\n")
        (docs / "c.md").write_text("It is the same app as Xed in every respect.\n")
        scanned, probs = gated_phrases(_record(partial), root=tmp, skip_parts=(), skip_files=())
        check("three documents scanned", scanned == 3, f"{scanned}")
        check("feature parity WITH the comparator is gated when a feature is absent", any("a.md:1" in x for x in probs), f"{probs}")
        check("the portability sentence 'the same app runs on' is NOT gated", not any("a.md:2" in x for x in probs), f"{probs}")
        check("a negated sentence -- the rule being stated -- is NOT gated", not any("a.md:3" in x for x in probs), f"{probs}")
        check("'feature parity' about Krate's own releases, no comparator named, is NOT gated", not any("a.md:4" in x for x in probs), f"{probs}")
        check("workload-matched wording passes", not any("b.md" in x for x in probs), f"{probs}")
        check("'the same app as <comparator>' is gated", any("c.md:1" in x for x in probs), f"{probs}")
        _, allowed = gated_phrases(_record(_app()), root=tmp, skip_parts=(), skip_files=())
        check("with every feature present the same sentences are permitted", allowed == [], f"{allowed}")

    if failures:
        print("check-compare-corpus self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("check-compare-corpus self-test OK -- wording is computed from the feature table, comparator facts "
          "are required, the seal binds the record, missing correctness cells are findings, and the gate "
          "catches parity claims while leaving the portability sentence alone")
    return 0


def main(argv):
    if "--self-test" in argv:
        return self_test()
    if "--seal" in argv:
        return cmd_seal()
    return cmd_check()


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
