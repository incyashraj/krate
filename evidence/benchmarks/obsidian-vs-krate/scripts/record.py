#!/usr/bin/env python3
"""Turn one instrument's raw output into one row of a leg's TSV (IC-999).

Each leg script calls this once per sample. The row names its raw files,
so analyze.py and audit.py can always go back to them. The accept or
reject decision is made here, by the rules of PROTOCOL.md section 7, and
the reason is written into the row; nothing is ever dropped.

  record.py startup   --run R --json J --isolation I|- --app A --load L --mode M --index N
  record.py resources --run R --launch J --samples S --top T --fp30 F --fp-end F --isolation I|- --app A --load L --index N --launched-at E
  record.py network   --run R --launch J --polls P --app A --load L --mode M --index N
  record.py growth    --run R --launch J --samples S --app A --load L
  record.py interact  --run R --json J --app A --load L --index N --scenario S [--before B --after A]
  record.py battery   --run R --launch J --before B --after A --app A --load L --minutes M --mode X
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import sys
from pathlib import Path

HEADERS = {
    "startup": ["timestamp_utc", "app", "load", "mode", "run_index", "window_ms", "content_ms", "content_lower_ms",
                "lines_read", "window_w", "window_h", "ocr_level", "captures", "pid", "status", "reason", "launch_json"],
    "resources": ["timestamp_utc", "app", "load", "run_index", "fp30_bytes", "fp120_bytes", "rss30_bytes", "rss120_bytes",
                  "procs30", "procs120", "threads30", "threads120", "fds30", "fds120", "idle_seconds",
                  "idle_cpu_pct", "idle_wakeups_per_s", "interrupt_wakeups_per_s", "billed_energy_delta",
                  "top_power_avg", "top_idlew_avg", "footprint_cli30_bytes", "translated_pids",
                  "status", "reason", "launch_json", "samples"],
    "network": ["timestamp_utc", "app", "load", "mode", "run_index", "seconds", "polls", "remote_count", "remotes",
                "status", "reason", "launch_json", "polls_file"],
    "growth": ["timestamp_utc", "app", "load", "minutes", "fp_first_bytes", "fp_last_bytes", "fp_max_bytes",
               "growth_pct", "samples_count", "status", "reason", "launch_json", "samples"],
    "interact": ["timestamp_utc", "app", "load", "scenario", "run_index", "value_ms", "second_ms", "third_ms",
                 "extra", "status", "reason", "json"],
    "battery": ["timestamp_utc", "app", "load", "mode", "minutes", "mah_before", "mah_after", "mah_used",
                "pct_before", "pct_after", "status", "reason", "launch_json"],
}


def now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def load(path: str | None) -> dict:
    if not path or path == "-":
        return {}
    try:
        return json.loads(Path(path).read_text())
    except Exception:
        return {}


def jsonl(path: str) -> list[dict]:
    out = []
    try:
        for line in Path(path).read_text().splitlines():
            line = line.strip()
            if line.startswith("{"):
                try:
                    out.append(json.loads(line))
                except ValueError:
                    pass
    except OSError:
        pass
    return out


def rel(run: Path, path: str | None) -> str:
    if not path or path == "-":
        return "-"
    try:
        return Path(path).resolve().relative_to(run.resolve()).as_posix()
    except ValueError:
        return path


def append(run: Path, leg: str, row: dict) -> None:
    out = run / "raw" / f"{leg}.tsv"
    out.parent.mkdir(parents=True, exist_ok=True)
    header = HEADERS[leg]
    new = not out.exists() or out.stat().st_size == 0
    with out.open("a") as fh:
        if new:
            fh.write("\t".join(header) + "\n")
        fh.write("\t".join(str(row.get(k, "")).replace("\t", " ").replace("\n", " ") for k in header) + "\n")


def launch_verdict(launch: dict) -> tuple[str, str]:
    """PROTOCOL.md 7: the note on screen, else why not."""
    if launch.get("content_on_screen") is True:
        return "accepted", "-"
    if not launch:
        return "rejected", "probe_wrote_nothing"
    if "window_ms" not in launch:
        if "exit_status" in launch:
            return "rejected", f"app_exited_status_{launch['exit_status']}"
        return "rejected", "no_window"
    if launch.get("problem"):
        return "rejected", "capture_refused_screen_recording"
    return "rejected", "note_not_on_screen"


def isolation(verdict: tuple[str, str], iso: dict) -> tuple[str, str]:
    if verdict[0] == "accepted" and iso.get("problems"):
        return "rejected", ";".join(iso["problems"])
    return verdict


def startup(a) -> None:
    run = Path(a.run)
    launch = load(a.json)
    status, reason = isolation(launch_verdict(launch), load(a.isolation))
    b = launch.get("content_window_bounds") or launch.get("window_bounds") or {}
    append(run, "startup", {
        "timestamp_utc": now(), "app": a.app, "load": a.load, "mode": a.mode, "run_index": a.index,
        "window_ms": launch.get("window_ms", ""), "content_ms": launch.get("content_ms", ""),
        "content_lower_ms": launch.get("content_lower_ms", ""), "lines_read": launch.get("lines_read", 0),
        "window_w": b.get("width", ""), "window_h": b.get("height", ""), "ocr_level": launch.get("ocr_level", ""),
        "captures": launch.get("captures", ""), "pid": launch.get("pid", ""), "status": status, "reason": reason,
        "launch_json": rel(run, a.json),
    })


def at(samples: list[dict], t: float) -> dict:
    """The sample nearest to t seconds after launch."""
    return min(samples, key=lambda s: abs(s["t_launch"] - t)) if samples else {}


def top_tree(path: str, pids: set[int]) -> tuple[float | str, float | str]:
    """Mean per-sample sums of top's POWER and IDLEW over the tree. top's
    first sample has no interval to measure over and is skipped."""
    blocks: list[list[list[str]]] = []
    try:
        lines = Path(path).read_text().splitlines()
    except OSError:
        return "", ""
    for line in lines:
        if line.startswith("PID") and "POWER" in line:
            blocks.append([])
        elif blocks and re.match(r"^\s*\d+", line):
            blocks[-1].append(line.split())
    power, idlew = [], []
    for block in blocks[1:]:
        p = w = 0.0
        for parts in block:
            try:
                # PID COMMAND... %CPU POWER IDLEW; a command may hold spaces.
                if int(parts[0]) in pids:
                    p += float(parts[-2].rstrip("+-*"))
                    w += float(parts[-1].rstrip("+-*"))
            except (ValueError, IndexError):
                continue
        power.append(p)
        idlew.append(w)
    if not power:
        return "", ""
    return sum(power) / len(power), sum(idlew) / len(idlew)


def resources(a) -> None:
    run = Path(a.run)
    launch = load(a.launch)
    status, reason = isolation(launch_verdict(launch), load(a.isolation))
    samples = jsonl(a.samples)
    launched = float(a.launched_at)
    for s in samples:
        s["t_launch"] = s.get("epoch", 0.0) - launched
    s30 = samples[0] if samples else {}
    s120 = at(samples, 120.0)
    last = samples[-1] if samples else {}
    row = {"timestamp_utc": now(), "app": a.app, "load": a.load, "run_index": a.index,
           "launch_json": rel(run, a.launch), "samples": rel(run, a.samples)}
    if status == "accepted" and len(samples) >= 2:
        wall = last["t_s"] - s30["t_s"]
        row.update({
            "fp30_bytes": s30.get("phys_footprint", ""), "fp120_bytes": s120.get("phys_footprint", ""),
            "rss30_bytes": s30.get("resident", ""), "rss120_bytes": s120.get("resident", ""),
            "procs30": s30.get("process_count", ""), "procs120": s120.get("process_count", ""),
            "threads30": s30.get("threads", ""), "threads120": s120.get("threads", ""),
            "fds30": s30.get("open_fds", ""), "fds120": s120.get("open_fds", ""),
            "idle_seconds": f"{wall:.3f}",
            "idle_cpu_pct": f"{(last['cpu_ns'] - s30['cpu_ns']) / 1e9 / wall * 100:.4f}",
            "idle_wakeups_per_s": f"{(last['idle_wakeups'] - s30['idle_wakeups']) / wall:.3f}",
            "interrupt_wakeups_per_s": f"{(last['interrupt_wakeups'] - s30['interrupt_wakeups']) / wall:.3f}",
            "billed_energy_delta": last.get("billed_energy", 0) - s30.get("billed_energy", 0),
        })
        pids = {p["pid"] for s in samples for p in s.get("processes", [])}
        power, idlew = top_tree(a.top, pids)
        row["top_power_avg"] = f"{power:.3f}" if power != "" else ""
        row["top_idlew_avg"] = f"{idlew:.3f}" if idlew != "" else ""
        fp = load(a.fp30)
        row["footprint_cli30_bytes"] = fp.get("total footprint", "")
        translated = [str(p.get("pid")) for p in fp.get("processes", []) if p.get("translated")]
        row["translated_pids"] = ",".join(translated) or "-"
        if translated:
            status, reason = "rejected", "rosetta_translated"
        if any(not s.get("root_alive", True) for s in samples):
            status, reason = "rejected", "app_exited_during_idle"
    elif status == "accepted":
        status, reason = "rejected", "too_few_samples"
    row["status"], row["reason"] = status, reason
    append(run, "resources", row)


def network(a) -> None:
    run = Path(a.run)
    launch = load(a.launch)
    status, reason = launch_verdict(launch)
    polls = jsonl(a.polls)
    remotes = sorted({c for p in polls for c in p.get("remotes", [])})
    append(run, "network", {
        "timestamp_utc": now(), "app": a.app, "load": a.load, "mode": a.mode, "run_index": a.index,
        "seconds": polls[-1]["t_s"] if polls else "", "polls": len(polls), "remote_count": len(remotes),
        "remotes": ",".join(remotes) or "-", "status": status, "reason": reason,
        "launch_json": rel(run, a.launch), "polls_file": rel(run, a.polls),
    })


def growth(a) -> None:
    run = Path(a.run)
    launch = load(a.launch)
    status, reason = launch_verdict(launch)
    samples = jsonl(a.samples)
    fps = [s["phys_footprint"] for s in samples if "phys_footprint" in s]
    row = {"timestamp_utc": now(), "app": a.app, "load": a.load, "samples_count": len(fps),
           "launch_json": rel(run, a.launch), "samples": rel(run, a.samples)}
    if fps:
        row.update({"minutes": f"{(samples[-1]['t_s'] - samples[0]['t_s']) / 60:.2f}", "fp_first_bytes": fps[0],
                    "fp_last_bytes": fps[-1], "fp_max_bytes": max(fps),
                    "growth_pct": f"{(fps[-1] - fps[0]) / fps[0] * 100:.3f}"})
    if status == "accepted" and any(not s.get("root_alive", True) for s in samples):
        status, reason = "rejected", "app_exited_during_growth"
    if status == "accepted" and len(fps) < 2:
        status, reason = "rejected", "too_few_samples"
    row["status"], row["reason"] = status, reason
    append(run, "growth", row)


def interact(a) -> None:
    run = Path(a.run)
    d = load(a.json)
    status, reason = d.get("status", "rejected"), d.get("reason", "interact_wrote_nothing")
    if str(d.get("schema", "")).startswith("krate.benchmark.start"):
        status, reason = launch_verdict(d)
        if status == "accepted":
            status, reason = "rejected", "interact_not_run"
    if status == "refused":
        status = "rejected"
    row = {"timestamp_utc": now(), "app": a.app, "load": a.load, "scenario": a.scenario, "run_index": a.index,
           "json": rel(run, a.json)}
    if a.scenario == "switcher":
        row.update({"value_ms": d.get("result_after_last_key_ms", ""), "second_ms": d.get("switcher_open_ms", ""),
                    "third_ms": d.get("open_note_ms", "")})
    elif a.scenario == "search":
        row.update({"value_ms": d.get("result_after_last_key_ms", ""), "second_ms": d.get("result_after_first_key_ms", "")})
    elif a.scenario == "typing":
        st = d.get("stats_ms", {})
        row.update({"value_ms": st.get("median", ""), "second_ms": st.get("p90", ""), "third_ms": st.get("max", ""),
                    "extra": f"n={len(d.get('latencies_ms', []))};missed={d.get('missed', '')}"})
    elif a.scenario == "scroll":
        g = d.get("frame_gap_ms", {})
        before, after = load(a.before), load(a.after)
        moved = bool(before.get("content_on_screen") and after.get("content_on_screen")
                     and after.get("last_line", 0) > before.get("last_line", 0))
        row.update({"value_ms": g.get("median", ""), "second_ms": g.get("p95", ""), "third_ms": g.get("max", ""),
                    "extra": f"p99={g.get('p99', '')};fps={d.get('frames_per_second', '')};hz={d.get('actual_hz', '')};"
                             f"lines={before.get('last_line', 0)}->{after.get('last_line', 0)}"})
        if status == "accepted" and not moved:
            status, reason = "rejected", "scroll_did_not_move_the_note"
    row["status"], row["reason"] = status, reason
    append(run, "interact", row)


def battery(a) -> None:
    run = Path(a.run)
    launch = load(a.launch)
    status, reason = launch_verdict(launch)
    b, e = load(a.before), load(a.after)
    row = {"timestamp_utc": now(), "app": a.app, "load": a.load, "mode": a.mode, "minutes": a.minutes,
           "mah_before": b.get("mah", ""), "mah_after": e.get("mah", ""), "pct_before": b.get("pct", ""),
           "pct_after": e.get("pct", ""), "launch_json": rel(run, a.launch)}
    if b.get("mah") is not None and e.get("mah") is not None:
        row["mah_used"] = b["mah"] - e["mah"]
    if status == "accepted" and (not b.get("on_battery") or not e.get("on_battery")):
        status, reason = "rejected", "not_on_battery_throughout"
    row["status"], row["reason"] = status, reason
    append(run, "battery", row)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("leg", choices=sorted(HEADERS))
    for flag in ("run", "json", "isolation", "app", "load", "mode", "index", "launch", "samples", "top", "fp30",
                 "fp-end", "launched-at", "polls", "scenario", "before", "after", "minutes"):
        ap.add_argument(f"--{flag}")
    a = ap.parse_args()
    {"startup": startup, "resources": resources, "network": network, "growth": growth,
     "interact": interact, "battery": battery}[a.leg](a)
    return 0


if __name__ == "__main__":
    sys.exit(main())
