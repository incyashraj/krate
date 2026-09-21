#!/usr/bin/env python3
"""Every site test must run before the site deploys.

The pre-push gate globs `docs/landing/studio/*-test.mjs`, so a new test is
covered at the desk the moment it exists. The deploy does not: pages.yml
names each test on its own line, with a comment saying why that one
matters. That is good writing and a bad list -- a test added later guards
the desk and not the thing people actually load.

Found when money-seam-test.mjs was written: the gate picked it up at once,
the deploy would never have run it. The same shape as the jobs that sat
behind the Windows lane for weeks -- anything not named does not run, and
nothing says so.

So: the two lists must agree. Run with --self-test to check the checker.
"""

import argparse
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
TESTS = ROOT / "docs/landing/studio"
PAGES = ROOT / ".github/workflows/pages.yml"


def missing(test_names: list[str], workflow: str) -> list[str]:
    """Tests that exist on disk and are not named in the workflow."""
    return [name for name in test_names if name not in workflow]


def on_disk() -> list[str]:
    return sorted(p.name for p in TESTS.glob("*-test.mjs"))


def self_test() -> int:
    names = on_disk()
    if not names:
        print("self-test: no site tests found at all -- the glob is wrong")
        return 1

    # 1. The real files must agree today.
    real = missing(names, PAGES.read_text())
    if real:
        print("self-test: the real files disagree, which is the bug this checks for:")
        for name in real:
            print(f"  - {name}")
        return 1

    # 2. A workflow with one test dropped must be caught. The negative
    #    fixture is built from the real workflow rather than a stub, so it
    #    cannot pass by testing something that is not the shipped shape.
    text = PAGES.read_text()
    victim = names[0]
    dropped = text.replace(f"node docs/landing/studio/{victim}", "node true", 1)
    if dropped == text:
        print(f"self-test: could not build the fixture -- {victim} is not invoked as expected")
        return 1
    if victim not in missing(names, dropped):
        print(f"self-test: dropping {victim} from the deploy was NOT caught")
        return 1

    print(f"self-test: ok -- {len(names)} tests agree, and a dropped one is caught")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="check the checker")
    args = parser.parse_args()
    if args.self_test:
        return self_test()

    names = on_disk()
    gaps = missing(names, PAGES.read_text())
    if gaps:
        print("These site tests run at the desk but not before the site deploys:")
        for name in gaps:
            print(f"  - {name}")
        print(
            "\nAdd each to .github/workflows/pages.yml, beside the others, with a "
            "line saying what it protects. The pre-push gate globs this folder; "
            "the deploy does not."
        )
        return 1
    print(f"ok  all {len(names)} site tests run before the site deploys")
    return 0


if __name__ == "__main__":
    sys.exit(main())
