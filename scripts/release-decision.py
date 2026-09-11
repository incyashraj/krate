#!/usr/bin/env python3
"""Generate the release decision record for one commit (IC-159).

    scripts/release-decision.py [--sha <commit>] [--accept K-nnn --because "..."]
    scripts/release-decision.py --self-test

A release decision used to be "the CI run is green", which answers whether
tests passed and nothing else: not what evidence the release rests on, not
which known defects ship inside it, not whether the fuzz campaign that
vouches for the parsers has actually been running. This gathers every source
into one dated record bound to the commit, with a verdict -- RELEASE or HOLD
-- and the reasons, and exits 1 on HOLD so a pipeline can refuse.

Two rules keep it honest:

  Absence is a verdict. A source this cannot read (no gh, no BUGS.md on this
  machine) is recorded as NOT ASSESSED and forces HOLD, never skipped
  quietly. A record that only mentions what it managed to check is how false
  greens are made.

  Accepted limitations are named people saying so. An open blocker holds the
  release unless it is accepted with --accept K-nnn --because "...", and the
  record prints who accepted what and why. There is no flag that accepts
  everything.

The record is content-hashed and names its commit. Real signing arrives with
CP1's keys; until then the hash binds the words to the bytes, and the record
says exactly that rather than claiming a signature it does not have.
"""
import argparse
import datetime
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "evidence" / "releases"
# Every required lane's verdict is also written as an evidence record, so a
# release gate leaves a retained, public-safe record behind it (IC-681) --
# not only a decision file that quotes the private board and stays local.
REGISTRY_RECORDS = ROOT / "evidence" / "registry" / "records"
REPO = "incyashraj/krate"
FUZZ_FRESH_DAYS = 7
ADVISORY_FRESH_DAYS = 7

# The lanes a stable release stands on. A lane missing from the run is a
# platform gap, which is its own finding -- not a silent pass.
REQUIRED_LANES = [
    "Full test (macos-latest)",
    "Full test (ubuntu-latest)",
    "Full test (windows-2022)",
    "Dependency audit (cargo-deny)",
    "Fuzz evidence freshness",
]


def sh(args):
    out = subprocess.run(args, capture_output=True, text=True)
    return out.returncode, out.stdout.strip(), out.stderr.strip()


# ---- sources ---------------------------------------------------------------

# What each lane runs on, and what its oracle actually judges. A record that
# says "hosted, ubuntu, x86_64, execution" can be read by the gate; "CI was
# green" cannot.
LANE_FACTS = {
    "Full test (macos-latest)": ("macos", "arm64",
        "the whole workspace test suite plus the replay and open-every-bundle steps ran and reported no failure"),
    "Full test (ubuntu-latest)": ("ubuntu", "x86_64",
        "the whole workspace test suite plus the replay and open-every-bundle steps ran and reported no failure"),
    "Full test (windows-2022)": ("windows", "x86_64",
        "the whole workspace test suite plus the replay and open-every-bundle steps ran and reported no failure"),
    "Dependency audit (cargo-deny)": ("ubuntu", "x86_64",
        "cargo-deny found no advisory, licence or source violation in the locked dependency graph"),
    "Fuzz evidence freshness": ("ubuntu", "x86_64",
        "the last successful fuzz nightly is within the freshness window"),
}
LANE_OUTCOME = {"pass": "pass", "fail": "fail", "skipped": "skipped", "missing": "blocked"}


def lane_slug(lane):
    return re.sub(r"[^a-z0-9]+", "-", lane.lower()).strip("-")


def lane_record(sha, tree, lane, state, why, ci, now, supersedes=None, suffix=""):
    os_name, arch, oracle = LANE_FACTS.get(lane, ("unknown", "unknown", "the lane concluded"))
    run_id = ci.get("run_id", "")
    times = (ci.get("times") or {}).get(lane) or {}
    return {
        "id": f"E-CI-{sha[:9]}-{lane_slug(lane)}{suffix}",
        "claims": ["C-RELEASE-GATES"],
        "subject": {
            "kind": "source tree",
            "source": {"commit": sha, "tree": tree},
            "binary_digest": None,
            "package_digest": None,
            "relationship": "the CI workflow checked out this commit and built it inside the lane",
        },
        "environment": {"kind": "hosted", "os": os_name, "arch": arch, "runner": "GitHub-hosted"},
        "toolchain": {"rust": "as pinned by .github/workflows/ci.yml at this commit"},
        "command": f"GitHub Actions job {lane!r} of CI run {run_id}",
        "inputs": {"prerequisites": []},
        "oracle": {"class": "execution", "description": oracle},
        "raw_result": {
            "conclusion": (ci.get("lanes") or {}).get(lane, "missing"),
            "run_id": run_id,
            "location": f"https://github.com/{REPO}/actions/runs/{run_id}" if run_id else "no run",
            "note": why,
        },
        "outcome": LANE_OUTCOME.get(state, "informational"),
        "scope": {
            "supports": f"{lane} concluded {state} on commit {sha[:9]}",
            "exclusions": ["anything the lane does not exercise", "clean recipients", "published artifacts"],
        },
        "independence": "first-party API observation",
        "time": {"start": times.get("started_at") or now, "finish": times.get("completed_at") or now},
        "operator": "GitHub Actions, CI workflow",
        "privacy": {"audience": "public", "redact": []},
        "retention": {"raw": "the GitHub Actions run log", "until": "GitHub's log retention"},
        "expiry": {"triggers": ["source-change"], "on": None},
        "supersedes": supersedes,
        "invalidated_by": None,
    }


def emit_registry_records(sha, tree, ci, findings, records_dir=REGISTRY_RECORDS, now=None):
    """One record per required lane. Append-only: a lane whose verdict changed
    since the last record (CI was re-run) gets a new record that names the
    old one; a verdict already on file is left alone."""
    if not ci.get("assessed"):
        return []
    now = now or datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    records_dir.mkdir(parents=True, exist_ok=True)
    written = []
    for lane, state, why in findings:
        base = f"E-CI-{sha[:9]}-{lane_slug(lane)}"
        existing = sorted(
            records_dir.glob(f"{base}*.json"),
            key=lambda p: (len(p.stem), p.stem),
        )
        supersedes, suffix = None, ""
        if existing:
            latest = json.loads(existing[-1].read_text())
            if latest.get("outcome") == LANE_OUTCOME.get(state, "informational"):
                continue
            supersedes = latest["id"]
            suffix = f"-r{len(existing) + 1}"
        record = lane_record(sha, tree, lane, state, why, ci, now, supersedes, suffix)
        path = records_dir / f"{record['id']}.json"
        path.write_text(json.dumps(record, indent=1) + "\n")
        written.append(record["id"])
    return written


def gather_ci(sha):
    """Every required lane's conclusion on this exact commit."""
    code, out, _err = sh([
        "gh", "api", f"repos/{REPO}/actions/runs?head_sha={sha}&per_page=10",
        "--jq", '[.workflow_runs[] | select(.name == "CI")][0].id // empty',
    ])
    if code != 0:
        return {"assessed": False, "why": "gh is unavailable or unauthenticated"}
    if not out:
        return {"assessed": False, "why": f"no CI run exists for {sha}"}
    run_id = out
    code, out, _err = sh([
        "gh", "api", f"repos/{REPO}/actions/runs/{run_id}/jobs?per_page=100", "--paginate",
        "--jq", '.jobs[] | "\\(.name)\\t\\(.conclusion // "pending")\\t\\(.started_at // "")\\t\\(.completed_at // "")"',
    ])
    if code != 0:
        return {"assessed": False, "why": "could not list the run's jobs"}
    lanes, times = {}, {}
    for line in out.splitlines():
        name, conclusion, started, completed = (line.split("\t") + ["", "", ""])[:4]
        lanes[name] = conclusion
        times[name] = {"started_at": started or None, "completed_at": completed or None}
    return {"assessed": True, "run_id": run_id, "lanes": lanes, "times": times}


def classify_lanes(ci):
    """Each required lane: pass / fail / missing. Missing is a platform gap."""
    findings = []
    if not ci.get("assessed"):
        return [("ci", "not-assessed", ci.get("why", ""))]
    lanes = ci["lanes"]
    for wanted in REQUIRED_LANES:
        if wanted not in lanes:
            findings.append((wanted, "missing", "this lane never ran on the commit -- a platform gap, not a pass"))
        elif lanes[wanted] == "success":
            findings.append((wanted, "pass", ""))
        elif lanes[wanted] == "skipped":
            findings.append((wanted, "skipped", "skipped is not evidence"))
        else:
            findings.append((wanted, "fail", f"concluded {lanes[wanted]}"))
    return findings


def gather_fuzz():
    code, out, _err = sh([
        "gh", "api",
        f"repos/{REPO}/actions/workflows/self-hosted-fuzz-nightly.yml/runs?status=success&per_page=1",
        "--jq", ".workflow_runs[0].created_at // empty",
    ])
    if code != 0:
        return {"assessed": False, "why": "gh is unavailable"}
    if not out:
        return {"assessed": True, "fresh": False, "age_days": None,
                "why": "no successful fuzz nightly has ever run"}
    last = datetime.datetime.fromisoformat(out.replace("Z", "+00:00"))
    age = (datetime.datetime.now(datetime.timezone.utc) - last).days
    return {"assessed": True, "fresh": age <= FUZZ_FRESH_DAYS, "age_days": age, "last": out}


def gather_advisories():
    record = ROOT / "evidence" / "advisories" / "latest.md"
    if not record.is_file():
        return {"assessed": False, "why": "evidence/advisories/latest.md is missing -- run scripts/record-advisory-evidence.sh"}
    text = record.read_text(errors="replace")
    verdict = re.search(r"^Verdict: (\w+)", text, re.M)
    scanned = re.search(r"^Scanned: ([0-9T:Z-]+)", text, re.M)
    age = None
    if scanned:
        at = datetime.datetime.fromisoformat(scanned.group(1).replace("Z", "+00:00"))
        age = (datetime.datetime.now(datetime.timezone.utc) - at).days
    return {
        "assessed": True,
        "verdict": verdict.group(1) if verdict else "unreadable",
        "age_days": age,
        "fresh": age is not None and age <= ADVISORY_FRESH_DAYS,
    }


def gather_claims():
    code, out, _err = sh([sys.executable, str(ROOT / "scripts" / "check-claims.py")])
    return {"assessed": True, "ok": code == 0, "detail": out.splitlines()[-1] if out else ""}


def parse_bugs(text):
    """Open entries from the board, with a tolerant read of a drifting format.

    The board's vocabulary has drifted (Status values like 'half' and
    'partly', severities like 'major'); an entry this cannot classify is
    reported as unparseable rather than guessed at, because a blocker
    misread as minor ships inside a release.
    """
    open_section = text.split("## Fixed")[0]
    entries = []
    matches = list(re.finditer(r"^### (K-\d+) -- (.+)$", open_section, re.M))
    for i, match in enumerate(matches):
        # The chunk ends where the next entry begins. A fixed window read the
        # NEXT bug's Severity line as this bug's -- the self-test caught it.
        end = matches[i + 1].start() if i + 1 < len(matches) else len(open_section)
        chunk = open_section[match.start():end]
        # Letters only: the board writes "FIXED, shipped in v0.2.2" and
        # "open, unclaimed", and \S+ dragged the comma along -- which made a
        # fixed bug read as unresolved and hold a release it had no claim on.
        status = re.search(r"^Status:\s+([A-Za-z]+)", chunk, re.M)
        severity = re.search(r"^Severity:\s+([A-Za-z]+)", chunk, re.M)
        entries.append({
            "id": match.group(1),
            "title": match.group(2).strip(),
            "status": (status.group(1).lower() if status else None),
            "severity": (severity.group(1).lower() if severity else None),
        })
    resolved = {"fixed", "superseded", "closed"}
    findings = {"blockers": [], "serious": [], "unparseable": []}
    for entry in entries:
        if entry["status"] in resolved:
            continue
        if entry["severity"] is None or entry["status"] is None:
            findings["unparseable"].append(entry)
        elif "blocker" in entry["severity"]:
            findings["blockers"].append(entry)
        elif entry["severity"] in ("serious", "high", "major"):
            findings["serious"].append(entry)
    return findings


def gather_registry():
    """The evidence registry's own gate: a material claim the evidence does
    not carry holds the release (IC-682). Read through the registry script
    itself, so this file never grows a second opinion about claim states."""
    import importlib.util
    path = ROOT / "scripts" / "evidence-registry.py"
    if not path.exists():
        return {"assessed": False, "why": "scripts/evidence-registry.py is missing"}
    spec = importlib.util.spec_from_file_location("registry", path)
    registry = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(registry)
    records, claims, load_problems = registry.load()
    problems = registry.validate(records, claims, load_problems)
    if problems:
        return {"assessed": True, "valid": False, "problems": problems, "refusals": []}
    return {"assessed": True, "valid": True, "problems": [],
            "refusals": registry.publication_gate(claims, records)}


def gather_bugs():
    board = ROOT / "BUGS.md"
    if not board.is_file():
        return {"assessed": False,
                "why": "BUGS.md is not on this machine (it is private); run this where the board lives"}
    result = parse_bugs(board.read_text(errors="replace"))
    result["assessed"] = True
    return result


# ---- the verdict -----------------------------------------------------------

def decide(sha, ci_findings, fuzz, advisories, claims, bugs, accepted, registry=None):
    holds = []

    # The evidence registry: a release surface may not carry a claim the
    # records do not support, and a registry that cannot be read is not a
    # registry that passed.
    if registry is not None:
        if not registry.get("assessed"):
            holds.append(f"evidence registry: not assessed -- {registry.get('why', '')}")
        elif not registry.get("valid"):
            holds.append(f"evidence registry: {len(registry['problems'])} malformed record(s)/claim(s) -- run scripts/evidence-registry.py validate")
        else:
            for refusal in registry.get("refusals", []):
                holds.append(f"evidence registry: {refusal}")

    for name, state, why in ci_findings:
        if state == "pass":
            continue
        holds.append(f"{name}: {state}" + (f" -- {why}" if why else ""))

    if not fuzz.get("assessed"):
        holds.append(f"fuzz evidence: not assessed -- {fuzz.get('why', '')}")
    elif not fuzz.get("fresh"):
        age = fuzz.get("age_days")
        holds.append(
            f"fuzz evidence: stale -- last successful nightly is "
            f"{age if age is not None else 'never'} day(s) old (limit {FUZZ_FRESH_DAYS})"
        )

    if not advisories.get("assessed"):
        holds.append(f"advisory evidence: not assessed -- {advisories.get('why', '')}")
    elif advisories.get("verdict") != "ok":
        holds.append(f"advisory evidence: verdict {advisories.get('verdict')}")
    elif not advisories.get("fresh"):
        holds.append(f"advisory evidence: stale ({advisories.get('age_days')} day(s) old)")

    if not claims.get("ok"):
        holds.append("public claims: drift detected -- see scripts/check-claims.py")

    if not bugs.get("assessed"):
        holds.append(f"defect tracker: not assessed -- {bugs.get('why', '')}")
    else:
        for bug in bugs.get("blockers", []):
            if bug["id"] in accepted:
                continue
            holds.append(f"open blocker {bug['id']}: {bug['title'][:80]}")
        for bug in bugs.get("unparseable", []):
            holds.append(
                f"{bug['id']} could not be classified (status={bug['status']}, "
                f"severity={bug['severity']}) -- an unreadable entry might be a blocker"
            )

    return ("RELEASE" if not holds else "HOLD"), holds


NOT_ASSESSED_EVER = [
    "whether any two tests duplicate one requirement, or any required journey has none",
    "the installer, update and rollback paths, beyond what the lanes above exercise",
    "that published download artifacts byte-match what CI built (arrives with CP1 signing)",
    "example apps beyond the ones the full suite replays",
]


def render(sha, verdict, holds, ci, fuzz, advisories, claims, bugs, accepted, because):
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    lines = [
        "# Release decision",
        "",
        f"Commit:  {sha}",
        f"Decided: {now}",
        f"Verdict: {verdict}",
        "",
        "Generated by scripts/release-decision.py. The hash at the end binds",
        "these words to these bytes; a real signature arrives with CP1's keys,",
        "and this record does not claim one before then.",
        "",
        "## What holds the release" if holds else "## Nothing holds the release",
        "",
    ]
    for hold in holds:
        lines.append(f"- {hold}")
    if not holds:
        lines.append("Every assessed gate passed and no unaccepted blocker is open.")
    if accepted:
        lines += ["", "## Accepted limitations", ""]
        for bug_id in sorted(accepted):
            lines.append(f"- {bug_id}: accepted for this release -- {because.get(bug_id, 'no reason recorded')}")
    if bugs.get("assessed") and bugs.get("serious"):
        lines += ["", "## Riding along (serious, non-blocking, ships inside this release)", ""]
        for bug in bugs["serious"]:
            lines.append(f"- {bug['id']}: {bug['title'][:90]}")
    lines += ["", "## What this record does not assess", ""]
    for item in NOT_ASSESSED_EVER:
        lines.append(f"- {item}")
    lines += [
        "",
        "A gate absent from this record was not checked. That sentence is the",
        "false-green prevention: nothing here implies more than it says.",
        "",
    ]
    body = "\n".join(lines) + "\n"
    digest = hashlib.sha256(body.encode()).hexdigest()
    return body + f"Content-SHA256: {digest}\n"


# ---- self-test -------------------------------------------------------------

def self_test():
    failures = []

    board = """### K-900 -- the sky is falling
Status:   open
Severity: blocker
### K-901 -- a drifting entry
Status:   half
### K-902 -- annoying but survivable
Status:   open
Severity: annoyance
## Fixed
### K-903 -- was a blocker, is fixed
Status:   fixed
Severity: blocker
### K-904 -- fixed with a trailing clause
Status:   FIXED, shipped in v0.2.2 (2026-08-30)
Severity: blocker
"""
    bugs = parse_bugs(board)
    if [b["id"] for b in bugs["blockers"]] != ["K-900"]:
        failures.append(f"blocker parse: {bugs['blockers']}")
    if [b["id"] for b in bugs["unparseable"]] != ["K-901"]:
        failures.append(f"an entry with no severity must be reported, got {bugs['unparseable']}")
    if any(b["id"] == "K-903" for b in bugs["blockers"]):
        failures.append("a fixed blocker must not hold a release")
    if any(b["id"] == "K-904" for b in bugs["blockers"]):
        failures.append(
            "'FIXED, shipped in v0.2.2' must read as fixed -- the comma was "
            "being captured into the status word"
        )

    green_ci = [(lane, "pass", "") for lane in REQUIRED_LANES]
    fresh_fuzz = {"assessed": True, "fresh": True, "age_days": 0}
    good_adv = {"assessed": True, "verdict": "ok", "fresh": True, "age_days": 0}
    good_claims = {"assessed": True, "ok": True}
    clean_bugs = {"assessed": True, "blockers": [], "serious": [], "unparseable": []}

    verdict, holds = decide("x", green_ci, fresh_fuzz, good_adv, good_claims, clean_bugs, set())
    if verdict != "RELEASE":
        failures.append(f"all-green must release, held on: {holds}")

    # Each gate, alone, must hold the release.
    cases = [
        ("a failed lane", [("Full test (windows-2022)", "fail", "concluded failure")] + green_ci[1:],
         fresh_fuzz, good_adv, good_claims, clean_bugs),
        ("a missing lane",
         classify_lanes({"assessed": True, "lanes": {lane: "success" for lane in REQUIRED_LANES[1:]}}),
         fresh_fuzz, good_adv, good_claims, clean_bugs),
        ("stale fuzz", green_ci, {"assessed": True, "fresh": False, "age_days": 10},
         good_adv, good_claims, clean_bugs),
        ("an open blocker", green_ci, fresh_fuzz, good_adv, good_claims,
         {"assessed": True, "blockers": [{"id": "K-900", "title": "x"}], "serious": [], "unparseable": []}),
        ("an unreadable entry", green_ci, fresh_fuzz, good_adv, good_claims,
         {"assessed": True, "blockers": [], "serious": [],
          "unparseable": [{"id": "K-901", "status": "half", "severity": None}]}),
        ("an unassessed board", green_ci, fresh_fuzz, good_adv, good_claims,
         {"assessed": False, "why": "not here"}),
        ("claim drift", green_ci, fresh_fuzz, good_adv, {"assessed": True, "ok": False}, clean_bugs),
    ]
    for name, ci, fz, adv, cl, bg in cases:
        verdict, _holds = decide("x", ci, fz, adv, cl, bg, set())
        if verdict != "HOLD":
            failures.append(f"{name} must hold the release")

    # The registry's own gate holds the release, and so does a registry that
    # cannot be read or is malformed (IC-682).
    for name, registry in [
        ("a refused material claim", {"assessed": True, "valid": True, "problems": [],
                                      "refusals": ["C-X (UNSUPPORTED): nothing"]}),
        ("a malformed registry", {"assessed": True, "valid": False, "problems": ["bad"], "refusals": []}),
        ("an unreadable registry", {"assessed": False, "why": "gone"}),
    ]:
        verdict, _holds = decide("x", green_ci, fresh_fuzz, good_adv, good_claims, clean_bugs, set(), registry)
        if verdict != "HOLD":
            failures.append(f"{name} must hold the release")
    verdict, _holds = decide("x", green_ci, fresh_fuzz, good_adv, good_claims, clean_bugs, set(),
                             {"assessed": True, "valid": True, "problems": [], "refusals": []})
    if verdict != "RELEASE":
        failures.append("a clean registry must not hold the release")

    # An accepted blocker releases -- and only the named one.
    with_blocker = {"assessed": True, "blockers": [{"id": "K-900", "title": "x"}],
                    "serious": [], "unparseable": []}
    verdict, _ = decide("x", green_ci, fresh_fuzz, good_adv, good_claims, with_blocker, {"K-900"})
    if verdict != "RELEASE":
        failures.append("an explicitly accepted blocker must not hold the release")
    verdict, _ = decide("x", green_ci, fresh_fuzz, good_adv, good_claims, with_blocker, {"K-999"})
    if verdict != "HOLD":
        failures.append("accepting a different bug must not release this one")

    # Every lane verdict becomes a registry record the gate can read, and a
    # changed verdict supersedes rather than overwrites (IC-681).
    import importlib.util
    import tempfile
    spec = importlib.util.spec_from_file_location("registry", ROOT / "scripts" / "evidence-registry.py")
    registry = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(registry)
    with tempfile.TemporaryDirectory() as tmp:
        records_dir = Path(tmp)
        ci = {"assessed": True, "run_id": "1", "lanes": {lane: "success" for lane in REQUIRED_LANES[1:]}, "times": {}}
        findings = classify_lanes(ci)
        first = emit_registry_records("abc123def0", "cafe1234", ci, findings, records_dir, now="2026-09-11T00:00:00Z")
        if len(first) != len(REQUIRED_LANES):
            failures.append(f"one record per required lane, got {len(first)}")
        recs, problems = registry.load_dir(records_dir, registry.RECORD_ID, "record")
        problems += [p for r in recs.values() for p in registry.validate_record(r, recs)]
        if problems:
            failures.append(f"emitted lane records must be valid registry records: {problems[:3]}")
        missing = [r for r in recs.values() if r["outcome"] == "blocked"]
        if len(missing) != 1:
            failures.append(f"a lane that never ran is recorded as blocked, not omitted: {[r['id'] for r in missing]}")
        again = emit_registry_records("abc123def0", "cafe1234", ci, findings, records_dir, now="2026-09-11T00:00:00Z")
        if again:
            failures.append(f"an unchanged verdict must not write a new record: {again}")
        ci["lanes"][REQUIRED_LANES[0]] = "failure"
        changed = emit_registry_records("abc123def0", "cafe1234", ci, classify_lanes(ci), records_dir, now="2026-09-11T01:00:00Z")
        if len(changed) != 1 or not changed[0].endswith("-r2"):
            failures.append(f"a changed verdict must supersede with a new record: {changed}")
        else:
            newer = json.loads((records_dir / f"{changed[0]}.json").read_text())
            if newer.get("supersedes") != changed[0][: -len("-r2")]:
                failures.append(f"the new record must name the one it supersedes: {newer.get('supersedes')}")
            if not (records_dir / f"{changed[0][: -len('-r2')]}.json").exists():
                failures.append("the superseded record must still exist -- nothing is deleted")

    # The record binds itself.
    record = render("abc123", "HOLD", ["x"], {}, fresh_fuzz, good_adv, good_claims, clean_bugs, set(), {})
    body, _, tail = record.rpartition("Content-SHA256: ")
    if hashlib.sha256(body.encode()).hexdigest() != tail.strip():
        failures.append("the content hash must match the content")

    if failures:
        print("release-decision self-test FAILED:\n")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print("OK -- every gate alone holds a release, acceptance is per-bug, and the record binds itself.")
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sha", default="HEAD")
    parser.add_argument("--accept", action="append", default=[])
    parser.add_argument("--because", action="append", default=[])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return self_test()

    if args.accept and len(args.because) != len(args.accept):
        print("every --accept needs its own --because; a limitation without a reason is not accepted")
        return 1
    accepted = set(args.accept)
    because = dict(zip(args.accept, args.because))

    code, sha, _err = sh(["git", "-C", str(ROOT), "rev-parse", args.sha])
    if code != 0:
        print(f"cannot resolve {args.sha}")
        return 1

    ci = gather_ci(sha)
    ci_findings = classify_lanes(ci)
    _code, tree, _err = sh(["git", "-C", str(ROOT), "rev-parse", f"{sha}^{{tree}}"])
    written = emit_registry_records(sha, tree or "0000000", ci, ci_findings)
    if written:
        print(f"evidence registry: wrote {len(written)} lane record(s) under {REGISTRY_RECORDS.relative_to(ROOT)}")
    fuzz = gather_fuzz()
    advisories = gather_advisories()
    claims = gather_claims()
    bugs = gather_bugs()
    registry = gather_registry()

    verdict, holds = decide(sha, ci_findings, fuzz, advisories, claims, bugs, accepted, registry)
    record = render(sha, verdict, holds, ci, fuzz, advisories, claims, bugs, accepted, because)

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = OUT_DIR / f"decision-{sha[:9]}.md"
    out.write_text(record)
    print(record)
    print(f"written to {out}")
    return 0 if verdict == "RELEASE" else 1


if __name__ == "__main__":
    sys.exit(main())
