#!/usr/bin/env python3
"""The starter apps share their kit by copy: check the copies are the same.

Team Notes, Expenses and Stock List carry the first edition of the starter
kit between "===== The starter kit" and "===== End of the starter kit"; the
twelve record apps (Requests ... Shift Rota) carry the second edition. A fix
made in one copy and not the others is how two apps come to behave
differently for the same reason, so every copy of an edition must be the same
text.

    python3 scripts/check-starter-kit.py              check apps/
    python3 scripts/check-starter-kit.py --self-test  prove the check bites
"""
import pathlib
import sys
import tempfile

START = "// ===== The starter kit"
END = "// ===== End of the starter kit ====="


def kit_of(text):
    """The kit block of one lib.rs, its edition line first, or None."""
    a = text.find(START)
    if a < 0:
        return None
    b = text.find(END, a)
    if b < 0:
        return None
    return text[a:b]


def check(apps):
    """Problems, as lines; empty when every edition's copies agree."""
    editions = {}
    for lib in sorted(apps.glob("*/src/lib.rs")):
        kit = kit_of(lib.read_text())
        if kit is None:
            continue
        editions.setdefault(kit.splitlines()[0], []).append((lib.parent.parent.name, kit))
    problems = []
    if not editions:
        problems.append("no app carries a starter kit")
    for head, copies in editions.items():
        first_name, first = copies[0]
        for name, kit in copies[1:]:
            if kit != first:
                a, b = first.splitlines(), kit.splitlines()
                line = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
                problems.append(
                    f"{name}'s starter kit differs from {first_name}'s at kit line {line + 1}: "
                    "make the change in every copy"
                )
    return problems, editions


def self_test():
    with tempfile.TemporaryDirectory() as d:
        apps = pathlib.Path(d)
        body = START + " edition =====\nfn kit() {}\n" + END + "\nfn app() {}\n"
        for name in ("krate-a", "krate-b"):
            (apps / name / "src").mkdir(parents=True)
            (apps / name / "src/lib.rs").write_text(body)
        (apps / "krate-other/src").mkdir(parents=True)
        (apps / "krate-other/src/lib.rs").write_text("fn no_kit() {}\n")
        problems, editions = check(apps)
        assert not problems and len(editions) == 1, problems
        # One copy changed: named, with the line.
        (apps / "krate-b/src/lib.rs").write_text(body.replace("fn kit() {}", "fn kit() { 1; }"))
        problems, _ = check(apps)
        assert len(problems) == 1 and "krate-b" in problems[0] and "line 2" in problems[0], problems
        # The app's own code after the kit may differ freely.
        (apps / "krate-b/src/lib.rs").write_text(body.replace("fn app() {}", "fn app() { 2; }"))
        assert not check(apps)[0]
    print("self-test passed")


def main():
    if "--self-test" in sys.argv:
        self_test()
        return 0
    root = pathlib.Path(__file__).resolve().parent.parent
    problems, editions = check(root / "apps")
    for p in problems:
        print(p)
    if problems:
        return 1
    for head, copies in editions.items():
        print(f"{len(copies)} copies agree: {head.strip('/ =')}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
