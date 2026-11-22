#!/usr/bin/env python3
"""Fail closed unless a run is complete, untouched and held to its
protocol (IC-999, PROTOCOL.md 7 and 10).

  audit.py RUN_DIR        PASS or FAIL with every blocking issue named
  audit.py --self-test    the rules on small made-up runs

It refuses: a protocol with uncommitted changes, committed after the run
began, or differing from the copy in the run; a kit edited since its
commit; a different machine or OS build between the prepared manifest, the
start and the end; inputs that changed during the run; a vault whose tree
digest is not the README's; a scheduled sample that is missing; an
accepted sample whose own raw instrument output does not show the note on
screen; more than 20 % of one app's samples in a leg rejected; Rosetta; an
Obsidian profile that leaked into the person's own; any file changed after
the seal; an analysis.md that is not what the raw files give. A synthetic
run (the self-test's) passes only with BENCH_SELF_TEST=1, and says so.
"""

from __future__ import annotations

import csv
import hashlib
import json
import os
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import analyze  # noqa: E402
import seal  # noqa: E402

APPS = ("vault", "obsidian")
PROTOCOL_LOADS = ("light", "medium", "heavy", "big-50000", "big-200000")
INTERACT_LOADS = ("light", "heavy")
MINIMUM = {"warm": 10, "cold": 5, "resources": 3, "interact": 10, "typing": 3, "scroll": 3}
CORE_LEGS = ("size", "startup", "resources", "network", "interaction", "growth")
TREES = {
    "light": "de15775a8f33f793618de644d4a9e505784f191b50f144ee60756ffe29dbff57",
    "medium": "89e0119e6f93311fb32e2cc925386bed47e2038e5c02239dce3b0779768ce97b",
    "heavy": "42209ae7fc0cd817d9ffdde36d0d97bf594a57c192ba340b3b32adf3d9402040",
    "big-1000": "656754a544229c5e9693e673cb4a2b4d6713aabb2c0d45df7b9bb933581e4e08",
    "big-50000": "08504f9b4ad1858d962a99574cbc3c7eed222716650475b4f399b906301f1610",
    "big-200000": "1e711196337de5c2c367bcee5d90a7086706281dee27886f5a739d1d0ef0de44",
}
FIXED_MACHINE = ("hw_model", "chip", "memory_bytes", "host_arch", "macos_version", "macos_build")
KIT_PATH = "evidence/benchmarks/obsidian-vs-krate"


def tsv(path: Path) -> list[dict[str, str]]:
    if not path.exists():
        return []
    with path.open(newline="") as fh:
        return list(csv.DictReader(fh, delimiter="\t"))


def kv(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    return {r[0]: r[1] for r in csv.reader(path.open(), delimiter="\t") if len(r) >= 2}


def config(path: Path) -> dict[str, str]:
    out = {}
    if path.exists():
        for line in path.read_text().splitlines():
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                out[k.strip()] = v.strip().strip('"').strip("'")
    return out


def jload(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return {}


def git(*args: str) -> tuple[int, str]:
    try:
        r = subprocess.run(["git", "-C", str(HERE), *args], capture_output=True, timeout=30)
        return r.returncode, r.stdout.decode(errors="replace")
    except (OSError, subprocess.SubprocessError):
        return 1, ""


def audit(run: Path) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    warns: list[str] = []
    raw = run / "raw"
    for f in ("protocol.json", "PROTOCOL.md", "config.snapshot.env", "machine.prepared.tsv", "machine.start.tsv",
              "machine.end.tsv", "inputs.start.tsv", "inputs.end.tsv", "artifacts/vault.krate", "analysis.md"):
        if not (run / f).exists():
            errors.append(f"missing {f}")
    if not (run / "kit" / "scripts").is_dir():
        errors.append("missing kit/scripts (the kit that ran)")

    machine = kv(run / "machine.start.tsv")
    synthetic = machine.get("synthetic") == "yes"
    if synthetic:
        if os.environ.get("BENCH_SELF_TEST") == "1":
            warns.append("SYNTHETIC run made by the self-test: not a measurement")
        else:
            errors.append("synthetic run: made by the self-test, not a measurement")

    # The protocol, committed before the run, and the same text.
    proto = jload(run / "protocol.json")
    copy = (run / "PROTOCOL.md").read_bytes() if (run / "PROTOCOL.md").exists() else b""
    if proto.get("protocol_dirty") is not False:
        errors.append("PROTOCOL.md had uncommitted changes when the run began")
    if proto.get("protocol_sha256") != hashlib.sha256(copy).hexdigest():
        errors.append("the run's PROTOCOL.md copy is not the one recorded at the start")
    commit = proto.get("protocol_commit", "")
    if not commit:
        errors.append("no commit recorded for PROTOCOL.md")
    else:
        rc, text = git("show", f"{commit}:{KIT_PATH}/PROTOCOL.md")
        if rc != 0:
            warns.append(f"cannot read protocol commit {commit[:12]} from git here; the protocol text is unchecked")
        elif text.encode() != copy:
            errors.append(f"the run's PROTOCOL.md differs from the text committed in {commit[:12]}")
    started = machine.get("captured_utc", "")
    if proto.get("protocol_committed_utc") and started:
        import datetime
        c = datetime.datetime.fromisoformat(proto["protocol_committed_utc"])
        s = datetime.datetime.fromisoformat(started.replace("Z", "+00:00"))
        if c > s:
            errors.append("PROTOCOL.md was committed after the run began")
    kit = proto.get("kit_commit", "")
    if proto.get("kit_dirty") is not False:
        errors.append("the kit (scripts/, config.example.env) had uncommitted changes when the run began")
    if kit == "synthetic":
        warns.append("kit copy not compared with git (synthetic run)")
    elif kit and (run / "kit" / "scripts").is_dir():
        for f in sorted((run / "kit" / "scripts").rglob("*")):
            if f.is_file():
                relp = f.relative_to(run / "kit").as_posix()
                rc, text = git("show", f"{kit}:{KIT_PATH}/{relp}")
                if rc != 0:
                    errors.append(f"kit/{relp} is not in the kit commit {kit[:12]}")
                elif text.encode() != f.read_bytes():
                    errors.append(f"kit/{relp} differs from the kit commit {kit[:12]}")

    # The same machine and OS throughout.
    for name in ("machine.prepared.tsv", "machine.end.tsv"):
        other = kv(run / name)
        for k in FIXED_MACHINE:
            if other and machine and other.get(k) != machine.get(k):
                errors.append(f"{name} {k} is {other.get(k)!r}, the start was {machine.get(k)!r}")
    if machine.get("low_power_mode") not in ("0", "", None):
        errors.append(f"Low Power Mode was {machine.get('low_power_mode')} at the start")

    # Inputs unchanged, the bundle in the run is the one measured, vaults as published.
    start, end = tsv(run / "inputs.start.tsv"), tsv(run / "inputs.end.tsv")
    if start != end:
        errors.append("inputs changed during the run (inputs.start.tsv differs from inputs.end.tsv)")
    by = {(r["kind"], r["path"]): r for r in start}
    vk = [r for r in start if r["kind"] == "vault_krate"]
    if vk and (run / "artifacts" / "vault.krate").exists():
        if hashlib.sha256((run / "artifacts" / "vault.krate").read_bytes()).hexdigest() != vk[0]["sha256"]:
            errors.append("artifacts/vault.krate is not the bundle the run measured")
    cfg = config(run / "config.snapshot.env")
    ob = next((r for r in start if r["kind"] == "obsidian_binary"), None)
    if ob:
        if cfg.get("OBSIDIAN_VERSION", "") not in ob["version_or_arch"]:
            errors.append(f"Obsidian was {ob['version_or_arch']}, the config pins {cfg.get('OBSIDIAN_VERSION')}")
        if machine.get("host_arch") and machine.get("host_arch") not in ob["version_or_arch"].split():
            errors.append("Obsidian has no slice for this Mac's architecture")
    kb = next((r for r in start if r["kind"] == "krate_binary"), None)
    if kb and cfg.get("KRATE_VERSION", "") not in kb["version_or_arch"]:
        errors.append(f"Krate reported {kb['version_or_arch']!r}, the config pins {cfg.get('KRATE_VERSION')}")
    for load, digest in TREES.items():
        row = by.get(("vault_tree", load))
        if row and row["sha256"] != digest:
            errors.append(f"vault {load} tree_sha256 {row['sha256'][:16]} is not the README's {digest[:16]}")

    legs = cfg.get("BENCH_LEGS", "").split()
    for leg in CORE_LEGS:
        if leg not in legs:
            warns.append(f"leg {leg} was not run: make no claim about it from this run")
    n = lambda k, d: int(cfg.get(k, d) or d)  # noqa: E731
    leg_rows: dict[str, list[dict]] = {}

    def need(rows, key_fn, expected: dict, what: str, minimum: int, configured: int):
        if configured < minimum:
            errors.append(f"{what}: config schedules {configured} samples, the protocol needs {minimum}")
        got = Counter(key_fn(r) for r in rows)
        for key in expected:
            if got[key] != configured:
                errors.append(f"{what} {'/'.join(key)}: {got[key]} samples recorded, {configured} scheduled")

    def proof(r, launch_key="launch_json") -> bool:
        d = jload(run / r.get(launch_key, ""))
        return d.get("content_on_screen") is True and int(d.get("lines_read", 0)) >= 3 and int(d.get("min_phrases", 0)) >= 3

    if "startup" in legs:
        loads = cfg.get("BENCH_LOADS", "").split()
        for l in PROTOCOL_LOADS:
            if l not in loads:
                errors.append(f"startup/resources: load {l} not scheduled")
        st = tsv(raw / "startup.tsv")
        leg_rows["startup"] = st
        for mode, key in (("warm", "BENCH_WARM_RUNS"), ("cold", "BENCH_COLD_RUNS")):
            exp = {(l, mode, a) for l in loads for a in APPS}
            need([r for r in st if r["mode"] == mode], lambda r: (r["load"], r["mode"], r["app"]), exp,
                 "startup", MINIMUM[mode], n(key, MINIMUM[mode]))
        for r in st:
            if r["status"] == "accepted":
                d = jload(run / r["launch_json"])
                if not proof(r) or str(d.get("content_ms")) != r["content_ms"] or str(d.get("window_ms")) != r["window_ms"]:
                    errors.append(f"startup {r['load']}/{r['mode']}#{r['run_index']} {r['app']}: accepted, but its raw "
                                  f"probe output does not show the note on screen with these times")
    if "resources" in legs:
        res = tsv(raw / "resources.tsv")
        leg_rows["resources"] = res
        loads = cfg.get("BENCH_LOADS", "").split()
        need(res, lambda r: (r["load"], r["app"]), {(l, a) for l in loads for a in APPS}, "resources",
             MINIMUM["resources"], n("BENCH_RESOURCE_RUNS", 3))
        idle = n("BENCH_IDLE_SECONDS", 120)
        for r in res:
            if r["status"] == "accepted":
                lines = [x for x in (run / r["samples"]).read_text().splitlines() if x.startswith("{")] \
                    if (run / r["samples"]).exists() else []
                if not proof(r) or len(lines) < idle:
                    errors.append(f"resources {r['load']}#{r['run_index']} {r['app']}: accepted without the note on "
                                  f"screen or with {len(lines)} of {idle + 1} idle samples")
            if "rosetta" in r.get("reason", ""):
                errors.append(f"resources {r['load']} {r['app']}: Rosetta translation")
    if "network" in legs:
        net = tsv(raw / "network.tsv")
        leg_rows["network"] = net
        need(net, lambda r: (r["mode"], r["app"]), {(m, a) for m in ("cold", "warm") for a in APPS}, "network", 1, 1)
        for r in net:
            if r["status"] == "accepted" and not proof(r):
                errors.append(f"network {r['mode']} {r['app']}: accepted without the note on screen")
    if "interaction" in legs:
        it = tsv(raw / "interact.tsv")
        leg_rows["interact"] = it
        for scen, loads, key, mk in (("switcher", INTERACT_LOADS, "BENCH_INTERACT_RUNS", "interact"),
                                     ("search", INTERACT_LOADS, "BENCH_INTERACT_RUNS", "interact"),
                                     ("typing", ("light", "big-50000"), "BENCH_TYPING_RUNS", "typing"),
                                     ("scroll", ("big-50000",), "BENCH_SCROLL_RUNS", "scroll")):
            need([r for r in it if r["scenario"] == scen], lambda r: (r["scenario"], r["load"], r["app"]),
                 {(scen, l, a) for l in loads for a in APPS}, "interaction", MINIMUM[mk], n(key, MINIMUM[mk]))
        for r in it:
            if r["status"] == "accepted" and jload(run / r["json"]).get("status") != "accepted":
                errors.append(f"interaction {r['scenario']} {r['load']}#{r['run_index']} {r['app']}: accepted, but its raw output is not")
    if "growth" in legs:
        gr = tsv(raw / "growth.tsv")
        leg_rows["growth"] = gr
        need(gr, lambda r: (r["app"],), {(a,) for a in APPS}, "growth", 1, 1)
        for r in gr:
            if r["status"] == "accepted" and (not proof(r) or float(r.get("minutes") or 0) < n("BENCH_GROWTH_MINUTES", 60) - 1):
                errors.append(f"growth {r['app']}: accepted without the note on screen or shorter than scheduled")

    # 20 % rule, per leg per app (PROTOCOL.md 7).
    for leg, rs in leg_rows.items():
        for app in APPS:
            mine = [r for r in rs if r["app"] == app]
            bad = [r for r in mine if r["status"] != "accepted"]
            if mine and len(bad) * 5 > len(mine):
                errors.append(f"{leg}: {len(bad)} of {len(mine)} {app} samples rejected (over 20 %); the leg's numbers cannot be used")

    # Obsidian stayed in its own profile (PROTOCOL.md 4, assumption 1).
    for f in sorted((raw / "isolation").glob("*.json")) if (raw / "isolation").exists() else []:
        for p in jload(f).get("problems", []):
            if p.startswith("own_obsidian_folder_changed") or p.startswith("profile_unused"):
                errors.append(f"isolation {f.name}: {p} -- an assumption of the protocol is false; stop and revise it")

    starts = tsv(raw / "startup.tsv")
    if any(r.get("ocr_level") == "fast" for r in starts):
        warns.append("some captures were read with Vision's fast level (the accurate one was refused here)")
    for r in analyze.accepted(starts):
        try:
            if abs(float(r["window_w"]) - float(cfg.get("BENCH_WINDOW_W", 1280))) > 2 or \
               abs(float(r["window_h"]) - float(cfg.get("BENCH_WINDOW_H", 800))) > 40:
                warns.append(f"{r['app']} {r['load']}: window {r['window_w']}x{r['window_h']}, not the protocol's size")
                break
        except ValueError:
            pass

    problems = seal.verify(run)
    errors += [f"seal: {p}" for p in problems]
    if (run / "analysis.md").exists() and analyze.check(run):
        errors.append("analysis.md is not what analyze.py produces from raw/")
    warns.append("Vault is a replica with the gaps in parity.md: quote results as an equivalent-workload comparison")
    return errors, warns


def report(run: Path) -> int:
    errors, warns = audit(run)
    for w in dict.fromkeys(warns):
        print(f"WARN: {w}")
    for e in errors:
        print(f"FAIL: {e}")
    if errors:
        print(f"FAIL ({len(errors)} blocking issue(s))")
        return 1
    print("PASS: complete, sealed and held to its protocol, under the stated scope")
    return 0


def self_test() -> int:
    """Small structural checks; the full synthetic run is in self-test.sh."""
    import tempfile
    fails = []
    with tempfile.TemporaryDirectory() as tmp:
        run = Path(tmp) / "20261122T000000Z"
        (run / "raw").mkdir(parents=True)
        e, _ = audit(run)
        for needed in ("missing protocol.json", "missing artifacts/vault.krate", "seal: no seal"):
            if not any(needed in x for x in e):
                fails.append(f"an empty run names: {needed}")
    if fails:
        for f in fails:
            print("FAIL", f)
        return 1
    print("audit self-test OK -- an empty run fails, naming what is missing")
    return 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    return report(Path(sys.argv[1]))


if __name__ == "__main__":
    sys.exit(main())
