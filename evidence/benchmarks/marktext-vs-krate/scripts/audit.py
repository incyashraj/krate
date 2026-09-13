#!/usr/bin/env python3
"""Fail closed when a benchmark run lacks evidence needed for publication."""

from __future__ import annotations

import csv
import hashlib
import sys
from collections import Counter
from pathlib import Path


def read_tsv(path: Path) -> list[dict[str, str]]:
    if not path.exists():
        return []
    with path.open(newline="") as handle:
        return list(csv.DictReader(handle, delimiter="\t"))


def integrity(run_dir: Path):
    """(errors, warnings): raw output sealed and unchanged (1809), analysis.md
    what the raw files produce (1810). Runs from before the seal existed
    (2026-09-13) may be unsealed with a warning; a new run may not."""
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import seal as _seal  # noqa: E402
    import analyze as _analyze  # noqa: E402
    errors, warnings = [], []
    seal_problems = _seal.verify(run_dir)
    if seal_problems and any("no seal" in p for p in seal_problems):
        if run_dir.resolve().name >= "20260913T":
            errors.append("raw output is not sealed; run seal.py RUN_DIR before publishing (1809)")
        else:
            warnings.append("raw output is not sealed (run predates the seal); nothing proves it is untouched")
    else:
        errors.extend(f"seal: {p}" for p in seal_problems)
    problem = _analyze.check(run_dir)
    if (run_dir / "analysis.md").exists() and problem:
        errors.append("analysis.md is not what analyze.py produces from raw/ (1810)")
    return errors, warnings


def self_test() -> int:
    import tempfile
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import seal as _seal  # noqa: E402
    import analyze as _analyze  # noqa: E402
    failures = []

    def make(name):
        run = Path(tempfile.mkdtemp()) / name
        (run / "raw").mkdir(parents=True)
        for f, header in (("size.tsv", "app\tbytes"), ("startup.tsv", "app\tworkload_content_lines\tmode\tstatus\twindow_ms"),
                          ("resources.tsv", "app\tworkload_content_lines\tstatus"), ("scroll.tsv", "app\tworkload_content_lines\tstatus")):
            (run / "raw" / f).write_text(header + "\n")
        (run / "machine.tsv").write_text("host_arch\tarm64\n")
        (run / "analysis.md").write_text(_analyze.render(run))
        return run

    old = make("20260825T000000Z")
    e, w = integrity(old)
    if e or not any("predates" in x for x in w):
        failures.append(f"an unsealed run from before the seal warns and does not fail: {e} {w}")
    new = make("20260913T000000Z")
    e, w = integrity(new)
    if not any("1809" in x for x in e):
        failures.append(f"an unsealed NEW run fails (1809): {e}")
    _seal.write(new)
    e, w = integrity(new)
    if e or w:
        failures.append(f"a sealed, consistent run is clean: {e} {w}")
    with (new / "raw" / "startup.tsv").open("a") as fh:
        fh.write("krate\t5000\twarm\taccepted\t200\n")
    e, _ = integrity(new)
    if not any("seal: raw/startup.tsv: changed" in x for x in e):
        failures.append(f"a raw sample changed after sealing fails on the seal: {e}")
    if not any("1810" in x for x in e):
        failures.append(f"and the analysis no longer matches its inputs (1810): {e}")
    _seal.write(new)
    (new / "analysis.md").write_text(_analyze.render(new))
    if integrity(new) != ([], []):
        failures.append(f"re-sealed and re-rendered is clean again: {integrity(new)}")
    (new / "analysis.md").write_text("typed by hand\n")
    e, _ = integrity(new)
    if not any("1810" in x for x in e) or any("seal:" in x for x in e):
        failures.append(f"an edited analysis fails 1810 and only 1810: {e}")
    if failures:
        print("audit self-test FAILED:")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("audit self-test OK -- the seal and the regeneration are part of the verdict, old runs warn, new runs fail")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    if len(sys.argv) != 2:
        print("usage: audit.py RUN_DIR | --self-test", file=sys.stderr)
        return 2
    run_dir = Path(sys.argv[1])
    raw = run_dir / "raw"
    errors: list[str] = []
    warnings: list[str] = []

    for required in ("machine.tsv", "inputs.tsv", "config.snapshot.env", "analysis.md"):
        if not (run_dir / required).exists():
            errors.append(f"missing {required}")
    staged_bundle = run_dir / "artifacts" / "mark-replica.krate"
    if not staged_bundle.exists():
        errors.append("missing self-contained artifacts/mark-replica.krate")
    for required in ("size.tsv", "startup.tsv", "resources.tsv", "scroll.tsv"):
        if not (raw / required).exists():
            errors.append(f"missing raw/{required}")

    inputs = read_tsv(run_dir / "inputs.tsv")
    input_kinds = Counter(row.get("kind") for row in inputs)
    for kind in ("marktext_binary", "krate_bundle", "krate_binary", "fixture"):
        if not input_kinds[kind]:
            errors.append(f"input manifest lacks {kind}")
    marktext = next((r for r in inputs if r.get("kind") == "marktext_binary"), None)
    bundle_input = next((r for r in inputs if r.get("kind") == "krate_bundle"), None)
    if staged_bundle.exists() and bundle_input:
        staged_sha = hashlib.sha256(staged_bundle.read_bytes()).hexdigest()
        if staged_sha != bundle_input.get("sha256"):
            errors.append("staged Krate bundle hash differs from input manifest")
    machine = dict((r[0], r[1]) for r in csv.reader((run_dir / "machine.tsv").open(), delimiter="\t") if len(r) >= 2) if (run_dir / "machine.tsv").exists() else {}
    if machine.get("host_arch") == "arm64" and marktext and "arm64" not in marktext.get("version_or_arch", ""):
        errors.append("MarkText was not ARM64 on an ARM64 host")

    size_rows = read_tsv(raw / "size.tsv")
    artifacts = {r.get("artifact") for r in size_rows}
    if "Krate_Studio_shared_runtime" not in artifacts:
        errors.append("shared Krate Studio/runtime installed size was not captured")

    startup = read_tsv(raw / "startup.tsv")
    accepted = Counter(
        (r.get("app"), r.get("workload_content_lines"), r.get("mode"))
        for r in startup
        if r.get("status") == "accepted" and r.get("window_ms")
    )
    modes = {r.get("mode") for r in startup}
    for mode in modes:
        for workload in ("5000", "50000"):
            for app in ("krate", "marktext"):
                count = accepted[(app, workload, mode)]
                if count < 10:
                    errors.append(f"only {count} accepted startup samples for {app}/{workload}/{mode}; need 10")

    resources = read_tsv(raw / "resources.tsv")
    for workload in ("5000", "50000"):
        for app in ("krate", "marktext"):
            matching = [r for r in resources if r.get("app") == app and r.get("workload_content_lines") == workload and r.get("status") == "accepted"]
            if not matching:
                errors.append(f"no accepted resource sample for {app}/{workload}")
    rejected_rosetta = [r for r in resources if "Rosetta" in r.get("reason", "")]
    if rejected_rosetta:
        errors.append("resource run detected Rosetta translation")

    scroll = read_tsv(raw / "scroll.tsv")
    for workload in ("5000", "50000"):
        for app in ("krate", "marktext"):
            matching = [r for r in scroll if r.get("app") == app and r.get("workload_content_lines") == workload and r.get("status") == "accepted"]
            if not matching:
                errors.append(f"no accepted controlled-scroll sample for {app}/{workload}")

    fixture_hashes: dict[str, set[str]] = {"5000": set(), "50000": set()}
    for filename in ("startup.tsv", "resources.tsv", "scroll.tsv"):
        for row in read_tsv(raw / filename):
            workload = row.get("workload_content_lines")
            digest = row.get("fixture_sha256")
            if workload in fixture_hashes and digest:
                fixture_hashes[workload].add(digest)
    for workload, hashes in fixture_hashes.items():
        if len(hashes) != 1:
            errors.append(f"workload {workload} used {len(hashes)} different fixture hashes")

    if not (raw / "energy-index.tsv").exists():
        warnings.append("energy was not measured; make no power or battery-life claim")
    warnings.append("feature parity is not established; describe this as an equivalent-workload comparison")

    integrity_errors, integrity_warnings = integrity(run_dir)
    errors.extend(integrity_errors)
    warnings.extend(integrity_warnings)

    for warning in warnings:
        print(f"WARN: {warning}")
    if errors:
        for error in errors:
            print(f"FAIL: {error}")
        print(f"FAIL ({len(errors)} blocking issue(s))")
        return 1
    print("PASS: evidence is complete enough for publication under the stated scope")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
