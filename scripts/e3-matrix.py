#!/usr/bin/env python3
"""The ported-app platform matrix as retained evidence (IC-755).

The replay job runs every ported app on macOS, Ubuntu and Windows and kept
nothing: a job log, gone with the run. The matrix -- which app ran where,
which failed, which was never tried -- existed only as a green tick that
said "all of it" or a red one that said "some of it".

`scripts/replay-ported-apps.sh` now writes one TSV per host with a row per
corpus bundle: pass, fail, or skip with the reason. This turns that TSV
into the evidence registry's own shapes, so the registry judges the claim:

  * a RESULTS file per host, audited against a frozen per-host profile that
    requires a cell for every bundle in the corpus. A bundle the replay
    never tried is a required cell that did not run, and it blocks the
    claim rather than vanishing (1576, 1579);
  * an evidence RECORD per host, pass or fail, so the claim "every ported
    app runs on every desktop OS" is PROVED only when all three hosts have
    a passing record on the same commit (1580).

The export is the files themselves: JSON in evidence/, readable with
nothing signed in to (1579).

  python3 scripts/e3-matrix.py ingest replay.tsv     # results + record
  python3 scripts/e3-matrix.py state                 # the claim, judged
  python3 scripts/e3-matrix.py --self-test
"""

import datetime
import importlib.util
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RESULTS = ROOT / "evidence" / "registry" / "results"
RECORDS = ROOT / "evidence" / "registry" / "records"
PROFILES = ROOT / "evidence" / "registry" / "profiles"
CLAIM = "C-PORTED-APPS-EVERY-OS"

# How a host names itself in the TSV, and the profile and platform it maps to.
HOSTS = {
    "macos": {"profile": "P-E3-REPLAY-MACOS", "arch": "arm64"},
    "ubuntu": {"profile": "P-E3-REPLAY-UBUNTU", "arch": "x86_64"},
    "windows": {"profile": "P-E3-REPLAY-WINDOWS", "arch": "x86_64"},
}
OUTCOMES = {"pass", "fail", "skip"}


def registry():
    spec = importlib.util.spec_from_file_location("evidence_registry", ROOT / "scripts" / "evidence-registry.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_tsv(text):
    """Header lines `# key\tvalue`, then `app\toutcome\treason\tms\texit`."""
    meta, rows = {}, []
    for line in text.splitlines():
        if not line.strip():
            continue
        if line.startswith("#"):
            key, _, value = line[1:].strip().partition("\t")
            meta[key.strip()] = value.strip()
            continue
        parts = line.split("\t")
        if len(parts) < 5:
            raise ValueError(f"unreadable row: {line!r}")
        app, outcome, reason, ms, code = parts[:5]
        if outcome not in OUTCOMES:
            raise ValueError(f"{app}: outcome {outcome!r} is not pass, fail or skip")
        rows.append({"app": app, "outcome": outcome, "reason": reason, "ms": int(ms or 0), "exit": int(code or 0)})
    for key in ("os", "arch", "commit", "ran_at", "krate"):
        if key not in meta:
            raise ValueError(f"the TSV header lacks {key}")
    if meta["os"] not in HOSTS:
        raise ValueError(f"host {meta['os']!r} is not one of {sorted(HOSTS)}")
    return meta, rows


def results_from(meta, rows, profile):
    """The registry results file: one cell per row, in the profile's vocabulary."""
    cells = []
    for row in rows:
        name = row["app"]
        if row["outcome"] == "pass":
            cells.append({"name": name, "outcome": "pass", "value": 1, "samples": 1, "note": row["reason"] or "printed its expected marker"})
        elif row["outcome"] == "fail":
            cells.append({"name": name, "outcome": "fail", "value": 0, "samples": 1, "note": row["reason"]})
        else:
            # A skip is a cell that did not run, with its reason kept. The
            # audit decides what that means: required and not-run blocks.
            cells.append({"name": name, "outcome": "not-run", "note": row["reason"]})
    return {
        "profile": profile["id"],
        "profile_frozen_at": profile["frozen_at"],
        "ran_at": meta["ran_at"],
        "source": f"scripts/replay-ported-apps.sh on {meta['os']} at {meta['commit'][:9]} ({meta['krate']})",
        "environment": {"kind": "hosted", "os": meta["os"], "arch": meta["arch"], "version": meta.get("os_version", "GitHub-hosted")},
        "cells": cells,
    }


def record_from(meta, rows, ok, findings, tree):
    """The evidence record the claim rests on. Fail when the audit fails."""
    sha = meta["commit"]
    host = HOSTS[meta["os"]]
    failed = [r["app"] for r in rows if r["outcome"] == "fail"]
    skipped = [r["app"] for r in rows if r["outcome"] == "skip"]
    return {
        "id": f"E-E3-REPLAY-{sha[:9]}-{meta['os']}",
        "claims": [CLAIM],
        "subject": {
            "kind": "source tree",
            "source": {"commit": sha, "tree": tree},
            "binary_digest": None,
            "package_digest": None,
            "relationship": "the replay job checked out this commit, built the runtime, and ran every corpus bundle on this host",
        },
        "environment": {"kind": "hosted", "os": meta["os"], "arch": meta["arch"], "runner": "GitHub-hosted"},
        "toolchain": {"rust": "as pinned by rust-toolchain.toml at this commit"},
        "command": "sh scripts/replay-ported-apps.sh",
        "inputs": {"prerequisites": ["evidence/ported/*.krate"]},
        "oracle": {
            "class": "execution",
            "description": "each bundle ran headless with its recorded argument and its stdout carried the expected marker; "
                           "a bundle with no recorded check is a skip, kept as one, and blocks the claim",
        },
        "raw_result": {
            "passed": [r["app"] for r in rows if r["outcome"] == "pass"],
            "failed": failed,
            "skipped": skipped,
            "audit": findings,
            "tsv": meta.get("tsv_path", ""),
        },
        "outcome": "pass" if ok else "fail",
        "scope": {
            "supports": f"every corpus bundle with a recorded check ran and printed its marker on {meta['os']} at {sha[:9]}",
            "exclusions": ["bundles with no recorded check, which are listed under skipped rather than counted",
                           "anything about how the apps behave beyond printing their marker"],
        },
        "independence": "first-party API observation",
        "time": {"start": meta["ran_at"], "finish": meta["ran_at"]},
        "operator": "GitHub Actions, CI workflow, replay-ported-apps job",
        "privacy": {"audience": "public", "redact": []},
        "retention": {"raw": meta.get("tsv_path", "the job's artifact"), "until": "the results file is deleted"},
        "expiry": {"triggers": ["source-change"], "on": None},
        "supersedes": None,
        "invalidated_by": None,
    }


def ingest(tsv_path, results_dir=RESULTS, records_dir=RECORDS, profiles_dir=PROFILES, tree=None, reg=None, claims_dir=None):
    reg = reg or registry()
    meta, rows = parse_tsv(Path(tsv_path).read_text())
    meta["tsv_path"] = str(tsv_path)
    profile_path = profiles_dir / f"{HOSTS[meta['os']]['profile']}.json"
    profile = json.loads(profile_path.read_text())
    results = results_from(meta, rows, profile)
    ok, findings = reg.audit_profile(profile, results)
    results["seal"] = reg.seal(results)
    day = meta["ran_at"][:10]
    results_path = results_dir / f"{day}-e3-replay-{meta['os']}-{meta['commit'][:9]}.json"
    results_dir.mkdir(parents=True, exist_ok=True)
    results_path.write_text(json.dumps(results, indent=1) + "\n")
    record = record_from(meta, rows, ok, findings, tree or meta.get("tree") or meta["commit"])
    records_dir.mkdir(parents=True, exist_ok=True)
    record_path = records_dir / f"{record['id']}.json"
    record_path.write_text(json.dumps(record, indent=1) + "\n")
    bind_claim(record, claims_dir=claims_dir)
    return results_path, record_path, ok, findings


def bind_claim(record, claims_dir=None):
    """Point the claim at the commit this record measured, and list the
    record as its evidence. A record for a NEWER commit starts the evidence
    list over: the claim is about one commit, and the registry refuses
    evidence from another one anyway. Without this the card said
    "not tested" beside a record that tested it."""
    claims_dir = claims_dir or (ROOT / "evidence" / "registry" / "claims")
    path = claims_dir / f"{CLAIM}.json"
    if not path.is_file():
        return
    claim = json.loads(path.read_text())
    sha = record["subject"]["source"]["commit"]
    if not str(claim.get("subject", {}).get("commit", "")).startswith(sha[:9]) and \
       not sha.startswith(str(claim.get("subject", {}).get("commit", "")) or "-"):
        claim["subject"] = {"commit": sha[:9]}
        claim["evidence"] = []
    if record["id"] not in claim["evidence"]:
        claim["evidence"].append(record["id"])
    path.write_text(json.dumps(claim, indent=1) + "\n")


def cmd_ingest(args):
    if len(args) != 1:
        print("ingest needs one replay TSV", file=sys.stderr)
        return 2
    results_path, record_path, ok, findings = ingest(args[0])
    print(f"wrote {results_path.relative_to(ROOT)}")
    print(f"wrote {record_path.relative_to(ROOT)} ({'pass' if ok else 'FAIL'})")
    for line in findings:
        print(f"  {line}")
    return 0 if ok else 1


def cmd_fetch(args):
    """Download a CI run's three matrix files and ingest each (needs gh)."""
    import subprocess
    import tempfile
    if len(args) != 1:
        print("fetch needs one CI run id", file=sys.stderr)
        return 2
    run_id = args[0]
    worst = 0
    with tempfile.TemporaryDirectory() as tmp:
        for os_name in ("macos-latest", "ubuntu-latest", "windows-2022"):
            dest = Path(tmp) / os_name
            out = subprocess.run(["gh", "run", "download", run_id, "-n", f"e3-replay-{os_name}", "-D", str(dest)],
                                 capture_output=True, text=True)
            if out.returncode != 0:
                print(f"{os_name}: no matrix artifact on run {run_id} ({out.stderr.strip()[:80]}) -- NOT assessed")
                worst = max(worst, 2)
                continue
            tsvs = list(dest.rglob("replay-*.tsv"))
            if not tsvs:
                print(f"{os_name}: artifact carried no replay-*.tsv -- NOT assessed")
                worst = max(worst, 2)
                continue
            keep = ROOT / "evidence" / "e3" / f"run-{run_id}-{tsvs[0].name}"
            keep.parent.mkdir(parents=True, exist_ok=True)
            keep.write_text(tsvs[0].read_text())
            results_path, record_path, ok, findings = ingest(keep)
            print(f"{os_name}: {record_path.name} ({'pass' if ok else 'FAIL'})")
            for line in findings:
                print(f"  {line}")
            if not ok:
                worst = max(worst, 1)
    return worst


def cmd_state():
    reg = registry()
    records, claims, problems = reg.load()
    if problems:
        for p in problems:
            print(f"  {p}")
        return 1
    if CLAIM not in claims:
        print(f"{CLAIM} is not in the registry")
        return 1
    print(reg.card(claims[CLAIM], reg.assess(claims[CLAIM], records)))
    return 0


# ------------------------------------------------------------------ self-test


def _tsv(os_name="macos", rows=None, commit="a" * 40):
    head = f"# os\t{os_name}\n# arch\t{HOSTS[os_name]['arch']}\n# commit\t{commit}\n# tree\t{'b' * 40}\n# ran_at\t2026-09-13T00:00:00Z\n# krate\tkrate 0.4.0\n"
    rows = rows if rows is not None else [
        ("hexyl", "pass", "", 120, 0), ("savings", "pass", "", 90, 0), ("ddh", "pass", "", 80, 0),
        ("envelope", "pass", "", 70, 0), ("grex", "pass", "", 60, 0), ("eo2", "pass", "", 50, 0),
        ("mdview", "pass", "", 40, 0), ("chart", "pass", "", 30, 0), ("bounce", "pass", "", 20, 0),
        ("cubes", "pass", "", 10, 0), ("rssfwd", "pass", "", 5, 0),
    ]
    return head + "".join(f"{a}\t{o}\t{r}\t{ms}\t{c}\n" for a, o, r, ms, c in rows)


def self_test():
    import tempfile
    reg = registry()
    failures = []

    def check(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    profiles = {p.stem: json.loads(p.read_text()) for p in PROFILES.glob("P-E3-REPLAY-*.json")}
    check("three per-host profiles exist", set(profiles) == {h["profile"] for h in HOSTS.values()}, f"{sorted(profiles)}")
    for pid, prof in profiles.items():
        probs = reg.validate_profile(prof)
        check(f"{pid} is a valid profile", probs == [], f"{probs}")
        corpus = {p.stem for p in (ROOT / "evidence" / "ported").glob("*.krate")}
        cells = {c["name"] for c in prof.get("cells", [])}
        check(f"{pid} requires a cell for every corpus bundle", cells == corpus, f"missing {corpus - cells}, extra {cells - corpus}")
        optional = {c["name"] for c in prof["cells"] if c["status"] != "required"}
        check(f"{pid} requires every cell except the one deliberately not replayed", optional == {"rssfwd"}, f"{optional}")
        check(f"{pid} says WHY that one is optional", all("reaches the internet" in c["definition"] for c in prof["cells"] if c["status"] != "required"))

    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        res, rec = tmp / "results", tmp / "records"
        t = tmp / "macos.tsv"
        t.write_text(_tsv())
        rp, cp, ok, findings = ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
        check("an all-pass replay audits clean", ok, f"{findings}")
        record = json.loads(cp.read_text())
        check("and yields a passing record for the claim", record["outcome"] == "pass" and record["claims"] == [CLAIM])
        check("the record is a valid registry record", reg.validate_record(record, {}) == [], f"{reg.validate_record(record, {})}")
        check("the results file is sealed and verifies", reg.check_seal(json.loads(rp.read_text()))[0] is True)

        rows = [r for r in [
            ("hexyl", "pass", "", 1, 0), ("savings", "pass", "", 1, 0), ("ddh", "pass", "", 1, 0), ("envelope", "pass", "", 1, 0),
            ("grex", "pass", "", 1, 0), ("eo2", "pass", "", 1, 0), ("mdview", "pass", "", 1, 0), ("chart", "pass", "", 1, 0),
            ("bounce", "pass", "", 1, 0), ("cubes", "fail", "ran but did not produce 'rendered3d:yes'", 1, 0),
            ("rssfwd", "skip", "no replay check defined", 0, 0)]]
        t.write_text(_tsv(rows=rows))
        rp, cp, ok, findings = ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
        check("a failed bundle fails the audit", not ok and any("cubes" in f for f in findings), f"{findings}")
        check("a skipped OPTIONAL bundle is neutral: it is neither a pass nor a finding", not any("rssfwd" in f for f in findings), f"{findings}")
        record = json.loads(cp.read_text())
        check("the record is a FAIL, never a pass with a footnote", record["outcome"] == "fail")
        skipped_required = [r if r[0] != "chart" else ("chart", "skip", "not tried today", 0, 0) for r in rows]
        t.write_text(_tsv(rows=skipped_required))
        _, cp2, ok2, findings2 = ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
        check("a REQUIRED bundle that was skipped blocks, and is named", not ok2 and any("chart" in f and "did not run" in f for f in findings2), f"{findings2}")
        check("failures and skips are kept in the record, not erased (1576)", record["raw_result"]["failed"] == ["cubes"] and record["raw_result"]["skipped"] == ["rssfwd"])
        results = json.loads(rp.read_text())
        check("the skip's reason travels into the results file", any(c["name"] == "rssfwd" and c["outcome"] == "not-run" and "no replay check" in c["note"] for c in results["cells"]))

        for bad, why in ((_tsv().replace("# os\tmacos", "# os\tplan9"), "host"), (_tsv().replace("hexyl\tpass", "hexyl\tmaybe"), "outcome"), (_tsv().replace("# commit\t" + "a" * 40 + "\n", ""), "commit")):
            t.write_text(bad)
            try:
                ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
                failures.append(f"a TSV with a bad {why} must be refused")
            except ValueError:
                pass
            except Exception as err:  # noqa: BLE001 -- a crash is not a refusal
                failures.append(f"a TSV with a bad {why} was refused by a crash ({type(err).__name__}), not by the guard")

        # The claim itself, judged by the registry: three passing hosts on one
        # commit prove it; two do not; a failing host contradicts it.
        recs = {}
        for os_name in HOSTS:
            t.write_text(_tsv(os_name=os_name))
            _, cp, _, _ = ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
            r = json.loads(cp.read_text()); recs[r["id"]] = r
        claim_src = json.loads((ROOT / "evidence" / "registry" / "claims" / f"{CLAIM}.json").read_text())
        (tmp / f"{CLAIM}.json").write_text(json.dumps(dict(claim_src, subject={"commit": "0000000000"}, evidence=[])))
        for os_name in HOSTS:
            t.write_text(_tsv(os_name=os_name))
            ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
        bound = json.loads((tmp / f"{CLAIM}.json").read_text())
        check("ingest binds the claim to the measured commit", bound["subject"]["commit"] == "a" * 9, f"{bound['subject']}")
        check("and lists all three records as its evidence", sorted(bound["evidence"]) == sorted(recs), f"{bound['evidence']}")
        t.write_text(_tsv(os_name="macos", commit="c" * 40))
        ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
        rebound = json.loads((tmp / f"{CLAIM}.json").read_text())
        check("a record for a newer commit starts the evidence over", rebound["subject"]["commit"] == "c" * 9 and len(rebound["evidence"]) == 1, f"{rebound}")
        claim = dict(claim_src, subject={"commit": "a" * 9}, evidence=list(recs))
        v = reg.assess(claim, recs)
        check("three passing hosts on one commit prove the claim", v["state"] == "PROVED", f"{v}")
        two = {k: r for k, r in recs.items() if not k.endswith("-windows")}
        v = reg.assess(dict(claim, evidence=list(two)), two)
        check("two hosts do not", v["state"] != "PROVED", f"{v}")
        t.write_text(_tsv(os_name="windows", rows=rows))
        _, cp, _, _ = ingest(t, res, rec, PROFILES, reg=reg, claims_dir=tmp)
        r = json.loads(cp.read_text()); recs[r["id"]] = r
        v = reg.assess(dict(claim, evidence=list(recs)), recs)
        check("a failing host contradicts the claim", v["state"] == "UNSUPPORTED", f"{v}")

    if failures:
        print("e3-matrix self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("e3-matrix self-test OK -- every corpus bundle is a required cell per host, a skip is kept and blocks, a failure "
          "is a failing record, and the claim is proved only by three passing hosts on one commit")
    return 0


def main(argv):
    if "--self-test" in argv:
        return self_test()
    if not argv:
        print(__doc__)
        return 2
    if argv[0] == "ingest":
        return cmd_ingest(argv[1:])
    if argv[0] == "state":
        return cmd_state()
    if argv[0] == "fetch":
        return cmd_fetch(argv[1:])
    print(f"unknown command {argv[0]}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
