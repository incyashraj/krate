#!/usr/bin/env python3
"""Turn untouched benchmark TSV files into a human-readable report."""

from __future__ import annotations

import csv
import math
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def rows(path: Path) -> list[dict[str, str]]:
    if not path.exists():
        return []
    with path.open(newline="") as handle:
        return list(csv.DictReader(handle, delimiter="\t"))


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def fmt_number(value: float, suffix: str = "") -> str:
    if value >= 100:
        return f"{value:,.0f}{suffix}"
    if value >= 10:
        return f"{value:,.1f}{suffix}"
    return f"{value:,.2f}{suffix}"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: analyze.py RUN_DIR", file=sys.stderr)
        return 2
    run_dir = Path(sys.argv[1])
    raw = run_dir / "raw"
    out: list[str] = [
        "# MarkText versus Krate benchmark analysis",
        "",
        "Generated only from the raw files in this run directory. This is an",
        "equivalent-workload comparison, not a claim of feature parity.",
        "",
    ]

    size_rows = rows(raw / "size.tsv")
    size = {(r["artifact"], r["measurement"]): r for r in size_rows}
    mark = size.get(("MarkText_0.17.1_ARM64", "installed_size"))
    bundle = size.get(("Krate_notes_bundle", "file_size"))
    studio = size.get(("Krate_Studio_shared_runtime", "installed_size"))
    out.extend(["## Artifact size", ""])
    if mark and bundle:
        mark_bytes = float(mark["value"]) * 1024
        bundle_bytes = float(bundle["value"])
        out.extend(
            [
                f"- MarkText 0.17.1 ARM64 installed: **{mark_bytes / 1024 / 1024:.1f} MiB**",
                f"- Krate notes app payload: **{int(bundle_bytes):,} bytes ({bundle_bytes / 1024:.1f} KiB)**",
                f"- Incremental per-app payload ratio: **{mark_bytes / bundle_bytes:,.0f}x smaller**",
            ]
        )
        if studio:
            studio_bytes = float(studio["value"]) * 1024
            out.append(f"- Shared Krate Studio/runtime installed once: **{studio_bytes / 1024 / 1024:.1f} MiB**")
            out.append(f"- First-app disk ratio including shared runtime: **{mark_bytes / (studio_bytes + bundle_bytes):.2f}x smaller**")
        else:
            out.append("- Shared Krate Studio/runtime size: **not captured; do not publish a first-use disk ratio**")
    else:
        out.append("Size measurements are incomplete.")
    out.append("")

    startup_rows = [r for r in rows(raw / "startup.tsv") if r["status"] == "accepted" and r["window_ms"]]
    groups: dict[tuple[str, str, str], list[float]] = defaultdict(list)
    for row in startup_rows:
        groups[(row["workload_content_lines"], row["mode"], row["app"])].append(float(row["window_ms"]))
    out.extend(["## Time to visible window", "", "| Workload | Mode | App | n | Median | p95 | Range |", "|---:|---|---|---:|---:|---:|---:|"])
    for key in sorted(groups):
        workload, mode, app = key
        values = groups[key]
        out.append(
            f"| {int(workload):,} | {mode} | {app} | {len(values)} | "
            f"{statistics.median(values):.1f} ms | {percentile(values, .95):.1f} ms | "
            f"{min(values):.1f}-{max(values):.1f} ms |"
        )
    out.append("")
    for workload, mode in sorted({(k[0], k[1]) for k in groups}):
        mark_values = groups.get((workload, mode, "marktext"), [])
        krate_values = groups.get((workload, mode, "krate"), [])
        if mark_values and krate_values:
            ratio = statistics.median(mark_values) / statistics.median(krate_values)
            out.append(f"- {int(workload):,} lines, {mode}: median visible-window time is **{ratio:.2f}x lower** for Krate.")
    out.append("")

    resource_rows = [r for r in rows(raw / "resources.tsv") if r["status"] == "accepted"]
    out.extend(["## Settled resources", "", "| Workload | App | Processes | Footprint | Average CPU |", "|---:|---|---:|---:|---:|"])
    for row in sorted(resource_rows, key=lambda r: (int(r["workload_content_lines"]), r["app"])):
        mib = float(row["footprint_bytes"]) / 1024 / 1024
        out.append(
            f"| {int(row['workload_content_lines']):,} | {row['app']} | {row['process_count']} | "
            f"{mib:.1f} MiB | {float(row['avg_cpu_percent']):.2f}% of one core |"
        )
    out.append("")

    scroll_rows = [r for r in rows(raw / "scroll.tsv") if r["status"] == "accepted"]
    out.extend(["## Controlled scrolling", ""])
    if scroll_rows:
        out.extend(["| Workload | App | Event rate | Average whole-tree CPU |", "|---:|---|---:|---:|"])
        for row in sorted(scroll_rows, key=lambda r: (int(r["workload_content_lines"]), r["app"])):
            out.append(
                f"| {int(row['workload_content_lines']):,} | {row['app']} | "
                f"{float(row['actual_hz']):.1f} Hz | {float(row['avg_cpu_percent']):.2f}% of one core |"
            )
    else:
        out.append("Not measured or rejected. Accessibility permission may be missing.")
    out.extend(["", "## Energy", ""])
    if (raw / "energy-index.tsv").exists():
        out.append("Raw `powermetrics` plists are present. Energy-impact values must be compared within this machine and run only.")
    else:
        out.append("Not measured. Do not make a power or battery-life claim from this run.")

    rejected = [r for name in ("startup.tsv", "resources.tsv", "scroll.tsv") for r in rows(raw / name) if r.get("status") == "rejected"]
    out.extend(["", "## Rejected samples", ""])
    if rejected:
        for row in rejected:
            out.append(f"- {row.get('app', '?')} / {row.get('workload_content_lines', '?')}: {row.get('reason', 'unspecified')}")
    else:
        out.append("None recorded.")

    (run_dir / "analysis.md").write_text("\n".join(out) + "\n")
    print(run_dir / "analysis.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
