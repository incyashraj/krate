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


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: audit.py RUN_DIR", file=sys.stderr)
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
