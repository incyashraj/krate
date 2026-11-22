#!/usr/bin/env python3
"""Every remote address held by a set of processes, polled (IC-999).

  net-poll.py --seconds 60 --interval 0.5 --exe-prefix PATH [--root-file F]

Polls `lsof -i -n -P` (no DNS lookups) and keeps the connections of every
process whose executable path starts with PATH, and of the process tree
of the pid written in F once it appears. One JSON line per poll. A
connection that opens and closes between two polls is not seen.
"""

import argparse
import json
import subprocess
import sys
import time


def exe_paths() -> tuple[dict[int, str], dict[int, int]]:
    out = subprocess.run(["/bin/ps", "-axo", "pid=,ppid=,comm="], capture_output=True, text=True).stdout
    paths, parents = {}, {}
    for line in out.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) == 3 and parts[0].isdigit():
            paths[int(parts[0])] = parts[2]
            parents[int(parts[0])] = int(parts[1])
    return paths, parents


def tree(root: int, parents: dict[int, int]) -> set[int]:
    out = {root}
    grew = True
    while grew:
        grew = False
        for pid, ppid in parents.items():
            if ppid in out and pid not in out:
                out.add(pid)
                grew = True
    return out


def connections(pids: set[int]) -> list[tuple[int, str, str]]:
    """Internet sockets of these pids only (-a -p), which keeps one poll short."""
    if not pids:
        return []
    cmd = ["/usr/sbin/lsof", "-a", "-p", ",".join(map(str, sorted(pids))), "-i", "-n", "-P", "-F", "pcnP"]
    out = subprocess.run(cmd, capture_output=True, text=True).stdout
    rows, pid, cmd, proto = [], 0, "", ""
    for line in out.splitlines():
        if not line:
            continue
        tag, val = line[0], line[1:]
        if tag == "p":
            pid = int(val)
        elif tag == "c":
            cmd = val
        elif tag == "P":
            proto = val
        elif tag == "n" and "->" in val:
            rows.append((pid, cmd, f"{proto}:{val.split('->', 1)[1]}"))
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, required=True)
    ap.add_argument("--interval", type=float, default=0.5)
    ap.add_argument("--exe-prefix", action="append", default=[])
    ap.add_argument("--root-file")
    a = ap.parse_args()
    start = time.time()
    i = 0
    while True:
        t = time.time() - start
        if t > a.seconds:
            break
        paths, parents = exe_paths()
        members = {p for p, path in paths.items() if any(path.startswith(x) for x in a.exe_prefix)}
        if a.root_file:
            try:
                root = int(open(a.root_file).read().strip())
                members |= tree(root, parents)
            except (OSError, ValueError):
                pass
        held = [(p, c, r) for p, c, r in connections(members) if p in members]
        print(json.dumps({"poll": i, "t_s": round(t, 3), "members": sorted(members),
                          "remotes": sorted({r for _, _, r in held}),
                          "by_process": sorted({f"{p}:{c}:{r}" for p, c, r in held})}), flush=True)
        i += 1
        time.sleep(max(0.0, start + i * a.interval - time.time()))
    return 0


if __name__ == "__main__":
    sys.exit(main())
