#!/usr/bin/env python3
"""Seal a run directory's raw output so "untouched" is checkable (IC-829,
test 1809).

A run directory holds what the instruments wrote (raw/), what was fed in
(artifacts/, inputs.tsv, config.snapshot.env) and where it ran
(machine.tsv). analysis.md and audit.txt are DERIVED from those and are not
sealed: they are regenerated and compared instead (test 1810, analyze.py
--check). The seal is a sha256 per file over everything else, in sorted
order, plus a header naming when it was sealed and the commit of the kit
that sealed it. Verification fails on a changed, missing or added file.

This is a seal, not a signature. It proves the files are the ones that
were sealed; it does not prove who sealed them. A signature over the seal
file arrives with publisher keys (CP1 signing), and the header says so
rather than implying it.

  python3 seal.py RUN_DIR             # write RUN_DIR/seal.sha256
  python3 seal.py --verify RUN_DIR    # exit 1 on any difference
  python3 seal.py --self-test
"""

import datetime
import hashlib
import subprocess
import sys
from pathlib import Path

SEAL = "seal.sha256"
# Derived files are compared against their inputs, not sealed.
DERIVED = {"analysis.md", "audit.txt", SEAL}
HEADER = "# krate.benchmark-run.seal.v1"


def sealed_files(run_dir):
    out = []
    for path in sorted(run_dir.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(run_dir).as_posix()
        if rel in DERIVED or rel.startswith("profiles/.") or "/__pycache__/" in f"/{rel}":
            continue
        out.append(rel)
    return out


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def render(run_dir, sealed_at=None, commit=None):
    lines = [HEADER]
    lines.append(f"# sealed_at: {sealed_at or datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}")
    lines.append(f"# kit_commit: {commit or _head()}")
    lines.append("# unsigned: a seal proves these files are the ones sealed, not who sealed them")
    for rel in sealed_files(run_dir):
        lines.append(f"{digest(run_dir / rel)}  {rel}")
    return "\n".join(lines) + "\n"


def _head():
    try:
        out = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, timeout=20)
        return out.stdout.strip() if out.returncode == 0 else "unknown"
    except (OSError, subprocess.SubprocessError):
        return "unknown"


def write(run_dir):
    text = render(run_dir)
    (run_dir / SEAL).write_text(text)
    return sum(1 for l in text.splitlines() if l and not l.startswith("#"))


def verify(run_dir):
    """A list of problems. Empty means every sealed file is as it was."""
    seal_path = run_dir / SEAL
    if not seal_path.is_file():
        return ["no seal: raw output is not sealed, so 'untouched' cannot be checked (1809)"]
    lines = seal_path.read_text().splitlines()
    if not lines or lines[0] != HEADER:
        return [f"{SEAL} does not start with {HEADER}"]
    recorded = {}
    for line in lines[1:]:
        if not line or line.startswith("#"):
            continue
        parts = line.split("  ", 1)
        if len(parts) != 2 or len(parts[0]) != 64:
            return [f"{SEAL} has an unreadable line: {line[:60]!r}"]
        recorded[parts[1]] = parts[0]
    problems = []
    present = sealed_files(run_dir)
    for rel in present:
        if rel not in recorded:
            problems.append(f"{rel}: added after sealing")
        elif digest(run_dir / rel) != recorded[rel]:
            problems.append(f"{rel}: changed after sealing")
    for rel in recorded:
        if rel not in present:
            problems.append(f"{rel}: sealed but now missing")
    return problems


def self_test():
    import tempfile
    failures = []

    def check(name, cond, detail=""):
        if not cond:
            failures.append(f"{name}: {detail}" if detail else name)

    with tempfile.TemporaryDirectory() as tmp:
        run = Path(tmp) / "run"
        (run / "raw").mkdir(parents=True)
        (run / "artifacts").mkdir()
        (run / "raw" / "startup.tsv").write_text("app\tms\nkrate\t200\n")
        (run / "artifacts" / "app.krate").write_bytes(b"PK\x03\x04bytes")
        (run / "machine.tsv").write_text("host_arch\tarm64\n")
        (run / "analysis.md").write_text("derived\n")
        n = write(run)
        check("three files sealed, the derived one left out", n == 3, f"{n}")
        check("a fresh seal verifies", verify(run) == [], f"{verify(run)}")

        (run / "analysis.md").write_text("regenerated differently\n")
        check("a changed DERIVED file does not break the seal", verify(run) == [], f"{verify(run)}")

        (run / "raw" / "startup.tsv").write_text("app\tms\nkrate\t150\n")
        p = verify(run)
        check("a changed raw sample is caught and named", any("raw/startup.tsv: changed" in x for x in p), f"{p}")
        (run / "raw" / "startup.tsv").write_text("app\tms\nkrate\t200\n")
        check("and restoring the bytes restores the seal", verify(run) == [])

        (run / "raw" / "extra.tsv").write_text("x\n")
        p = verify(run)
        check("a file added after sealing is caught", any("extra.tsv: added" in x for x in p), f"{p}")
        (run / "raw" / "extra.tsv").unlink()

        (run / "artifacts" / "app.krate").unlink()
        p = verify(run)
        check("a sealed file gone missing is caught", any("app.krate: sealed but now missing" in x for x in p), f"{p}")
        (run / "artifacts" / "app.krate").write_bytes(b"PK\x03\x04bytes")

        (run / SEAL).unlink()
        p = verify(run)
        check("no seal is a problem, not a pass", any("no seal" in x for x in p), f"{p}")

        write(run)
        text = (run / SEAL).read_text()
        check("the seal says it is unsigned", "unsigned" in text)
        (run / SEAL).write_text(text.replace("a" * 0, "", 1)[:-1] + "0\n")  # corrupt the last digest character
        check("a tampered seal line is caught", verify(run) != [])

    if failures:
        print("seal self-test FAILED:\n")
        for f in failures:
            print(f"  - {f}")
        return 1
    print("seal self-test OK -- raw and input files are sealed, derived files are not, and a changed, added, missing or unsealed file is a named problem")
    return 0


def main(argv):
    if "--self-test" in argv:
        return self_test()
    if "--verify" in argv:
        args = [a for a in argv if a != "--verify"]
        if len(args) != 1:
            print("usage: seal.py --verify RUN_DIR", file=sys.stderr)
            return 2
        problems = verify(Path(args[0]))
        for p in problems:
            print(f"FAIL: {p}")
        if problems:
            return 1
        print("OK -- every sealed file is as it was")
        return 0
    if len(argv) != 1:
        print("usage: seal.py RUN_DIR | --verify RUN_DIR | --self-test", file=sys.stderr)
        return 2
    n = write(Path(argv[0]))
    print(f"sealed {n} files into {Path(argv[0]) / SEAL}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
