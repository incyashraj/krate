#!/usr/bin/env python3
"""A complete SYNTHETIC run directory, for the kit's self-test only.

Every number in it is made up by a fixed-seed generator and the run is
marked `synthetic yes` in its machine files, so analyze.py heads its
analysis with SYNTHETIC and audit.py refuses it unless BENCH_SELF_TEST=1.
Rows are written by the real record.py from made-up instrument output, so
the self-test exercises the same path a measured run takes.

  synth-run.py OUT_DIR
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import random
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
KIT = HERE.parent
spec = importlib.util.spec_from_file_location("record", HERE / "record.py")
record = importlib.util.module_from_spec(spec)
spec.loader.exec_module(record)
sys.path.insert(0, str(HERE))
from audit import TREES  # noqa: E402

LOADS = ["light", "medium", "heavy", "big-50000", "big-200000"]
APPS = ["vault", "obsidian"]


def ns(**kw):
    return argparse.Namespace(**kw)


def git(*a):
    r = subprocess.run(["git", "-C", str(KIT), *a], capture_output=True, text=True)
    return r.stdout.strip()


def make(run: Path) -> None:
    rng = random.Random(0x1C999)
    if run.exists():
        shutil.rmtree(run)
    raw = run / "raw"
    for d in ("raw/launch", "raw/logs", "raw/resources", "raw/network", "raw/growth", "raw/interact",
              "raw/isolation", "artifacts", "kit", "targets", "vaults"):
        (run / d).mkdir(parents=True, exist_ok=True)

    proto = (KIT / "PROTOCOL.md").read_bytes()
    commit = git("log", "-1", "--format=%H", "--", "PROTOCOL.md")
    (run / "PROTOCOL.md").write_bytes(proto)
    (run / "protocol.json").write_text(json.dumps({
        "protocol_sha256": hashlib.sha256(proto).hexdigest(), "protocol_commit": commit,
        "protocol_committed_utc": git("log", "-1", "--format=%cI", "--", "PROTOCOL.md"),
        "protocol_dirty": False, "kit_commit": "synthetic", "kit_dirty": False}, indent=1, sort_keys=True) + "\n")
    shutil.copytree(HERE, run / "kit" / "scripts", ignore=shutil.ignore_patterns("__pycache__"))
    cfg = (KIT / "config.example.env").read_text()
    if 'BENCH_LEGS="size startup resources network interaction growth"' not in cfg:
        raise SystemExit("config.example.env no longer schedules every core leg; update synth-run.py")
    (run / "config.snapshot.env").write_text(cfg)

    # Stamped now: a fixed date failed the "protocol committed before the run"
    # check on every protocol commit after that date.
    started = __import__("datetime").datetime.now(__import__("datetime").timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    machine = (f"captured_utc\t{started}\n" "hw_model\tSynthetic1,1\nchip\tSynthetic chip\nmemory_bytes\t17179869184\n"
               "host_arch\tarm64\nmacos_version\t27.0\nmacos_build\t26A428\nlow_power_mode\t0\npower_source\tAC Power\n"
               "synthetic\tyes\n")
    for name in ("machine.prepared.tsv", "machine.start.tsv", "machine.end.tsv"):
        (run / name).write_text(machine)
    bundle = b"PK\x03\x04 synthetic vault bundle"
    (run / "artifacts" / "vault.krate").write_bytes(bundle)
    inputs = ["kind\tpath\tbytes\tsha256\tversion_or_arch",
              "obsidian_binary\t/synthetic/Obsidian\t1\t" + "0" * 64 + "\t1.15.1 x86_64 arm64",
              "krate_binary\t/synthetic/krate\t1\t" + "1" * 64 + "\tkrate 0.6.1 wasmtime 43.0.2; arm64",
              f"vault_krate\t/synthetic/vault.krate\t{len(bundle)}\t{hashlib.sha256(bundle).hexdigest()}\tdev.krate.vault 0.1.0"]
    inputs += [f"vault_tree\t{k}\t1\t{v}\tmake-bench-vault/1" for k, v in TREES.items()]
    for name in ("inputs.start.tsv", "inputs.end.tsv"):
        (run / name).write_text("\n".join(inputs) + "\n")

    (raw / "size.tsv").write_text(
        "timestamp_utc\tapp\titem\tmeasurement\tvalue\tunit\tsha256\tpath\n"
        "t\tobsidian\tdownload\tfile_size\t233346990\tbytes\t-\t/synthetic/Obsidian.dmg\n"
        "t\tobsidian\tinstalled_app\tallocated\t538392\tKiB\t-\t/synthetic/Obsidian.app\n"
        "t\tvault\tapp_bundle\tfile_size\t200000\tbytes\t-\t/synthetic/vault.krate\n"
        "t\tvault\tkrate_download\tfile_size\t30000000\tbytes\t-\t/synthetic/Krate.dmg\n"
        "t\tvault\tkrate_installed:Krate.app\tallocated\t60000\tKiB\t-\t/synthetic/Krate.app\n")

    def launch(tag, app, window, content):
        path = raw / "launch" / f"{tag}.json"
        path.write_text(json.dumps({"schema": "krate.benchmark.start.v2", "pid": 1000, "window_ms": window,
                                    "content_ms": content, "content_lower_ms": content - 40, "content_on_screen": True,
                                    "lines_read": 5, "min_phrases": 3, "captures": 3, "ocr_level": "accurate",
                                    "window_bounds": {"x": 0, "y": 0, "width": 1280, "height": 800}}))
        if app == "vault":
            (raw / "logs" / f"{tag}.log").write_text(f"t_window_ms:{int(window * 0.6)}\nt_first_frame_ms:{int(window * 0.8)}\n")
        return str(path)

    base = {"vault": (180, 260), "obsidian": (700, 1300)}
    for load in LOADS:
        for mode, count in (("warm", 10), ("cold", 5)):
            for i in range(1, count + 1):
                for app in APPS:
                    w = base[app][0] * (1.4 if mode == "cold" else 1) + rng.uniform(0, 40)
                    c = base[app][1] * (1.6 if mode == "cold" else 1) + rng.uniform(0, 80)
                    j = launch(f"startup-{mode}-{load}-{i}-{app}", app, round(w, 3), round(c, 3))
                    iso = "-"
                    if app == "obsidian":
                        iso = str(raw / "isolation" / f"startup-{mode}-{load}-{i}-{app}.json")
                        Path(iso).write_text(json.dumps({"problems": []}))
                    record.startup(ns(run=str(run), json=j, isolation=iso, app=app, load=load, mode=mode, index=str(i)))
        for i in range(1, 4):
            for app in APPS:
                tag = f"resources-{load}-{i}-{app}"
                j = launch(tag, app, 200, 300)
                fp = 90e6 if app == "vault" else 520e6
                samples = raw / "resources" / f"{tag}.samples.jsonl"
                lines = []
                for s in range(121):
                    lines.append(json.dumps({"sample": s, "t_s": float(s), "epoch": 1000.0 + 30 + s, "root_alive": True,
                                             "phys_footprint": int(fp + s * 1000), "resident": int(fp * 1.3),
                                             "process_count": 1 if app == "vault" else 4, "threads": 20 if app == "vault" else 90,
                                             "open_fds": 40 if app == "vault" else 160, "cpu_ns": s * 20_000_000,
                                             "idle_wakeups": s * 30, "interrupt_wakeups": s * 5, "billed_energy": s * 1000,
                                             "processes": [{"pid": 1000}]}))
                samples.write_text("\n".join(lines) + "\n")
                top = raw / "resources" / f"{tag}.top.txt"
                top.write_text("".join("PID    COMMAND  %CPU POWER IDLEW\n1000   App Main 1.0 2.5   10\n" for _ in range(3)))
                fp30 = raw / "resources" / f"{tag}.footprint30.json"
                fp30.write_text(json.dumps({"total footprint": int(fp), "processes": [{"pid": 1000}]}))
                record.resources(ns(run=str(run), launch=j, samples=str(samples), top=str(top), fp30=str(fp30),
                                    fp_end="-", isolation="-", app=app, load=load, index=str(i), launched_at="1000.0"))
    for mode in ("cold", "warm"):
        for app in APPS:
            tag = f"network-{mode}-light-1-{app}"
            j = launch(tag, app, 200, 300)
            polls = raw / "network" / f"{tag}.polls.jsonl"
            remotes = [] if app == "vault" else ["TCP:[2001:db8::1]:443"]
            polls.write_text("".join(json.dumps({"poll": p, "t_s": p * 0.5, "remotes": remotes}) + "\n" for p in range(121)))
            record.network(ns(run=str(run), launch=j, polls=str(polls), app=app, load="light", mode=mode, index="1"))
    for app in APPS:
        tag = f"growth-heavy-{app}"
        j = launch(tag, app, 200, 300)
        s = raw / "growth" / f"{tag}.samples.jsonl"
        s.write_text("".join(json.dumps({"t_s": m * 60.0, "phys_footprint": 100_000_000 + m * 10_000, "root_alive": True}) + "\n"
                             for m in range(61)))
        record.growth(ns(run=str(run), launch=j, samples=str(s), app=app, load="heavy"))
    for scen, loads, count in (("switcher", ["light", "heavy"], 10), ("search", ["light", "heavy"], 10),
                               ("typing", ["light", "big-50000"], 3), ("scroll", ["big-50000"], 3)):
        for load in loads:
            for app in APPS:
                for i in range(1, count + 1):
                    out = raw / "interact" / f"interact-{scen}-{load}-{app}-{i}.json"
                    d = {"schema": "krate.benchmark.interact.v1", "status": "accepted", "reason": "-",
                         "result_after_last_key_ms": rng.uniform(20, 120), "switcher_open_ms": rng.uniform(8, 40),
                         "open_note_ms": rng.uniform(30, 200), "result_after_first_key_ms": rng.uniform(200, 900),
                         "stats_ms": {"median": rng.uniform(8, 30), "p90": rng.uniform(20, 50), "max": 80},
                         "latencies_ms": [10.0] * 30, "missed": 0,
                         "frame_gap_ms": {"median": 8.3, "p95": 16.7, "p99": 25, "max": 40}, "frames_per_second": 110,
                         "actual_hz": 125}
                    out.write_text(json.dumps(d))
                    before = after = "-"
                    if scen == "scroll":
                        before, after = str(out) + ".before", str(out) + ".after"
                        Path(before).write_text(json.dumps({"content_on_screen": True, "last_line": 40 * i}))
                        Path(after).write_text(json.dumps({"content_on_screen": True, "last_line": 40 * i + 300}))
                    record.interact(ns(run=str(run), json=str(out), app=app, load=load, index=str(i), scenario=scen,
                                       before=before, after=after))


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    make(Path(sys.argv[1]))
    print(sys.argv[1])
    return 0


if __name__ == "__main__":
    sys.exit(main())
