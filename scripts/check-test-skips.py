#!/usr/bin/env python3
"""Reject silent early returns in test suites (IC-706 / IC-709, K-269).

    scripts/check-test-skips.py              # check the suites
    scripts/check-test-skips.py --self-test  # prove the checker bites

Cargo counts a test that returns early as PASSED. On a machine without the
optional fixtures, 44 of 121 CLI tests did exactly that -- 0.00 seconds each,
green, and the pass count read as coverage (K-269; the register's own audit
counted "at least 51", IC-706). The run's tally now announces skips, but a
tally can only count skips that SAY so.

This holds the other half: every early `return` in a test must be announced,
either by an `eprintln!("skipping ...")` beside it or by calling a helper
that announces on the caller's behalf. A silent early return is
indistinguishable from a pass in every report we produce, so it is rejected
here, at review time, where the pattern is visible in the source.

An early return is not itself the offence -- optional fixtures are a fact of
life, and the honest response to a missing one is a LOUD skip. The offence
is silence.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Test sources this discipline applies to. Grown deliberately, not globbed:
# each suite added here has had its early returns audited once by a person.
SUITES = [
    "crates/cli/tests/cli.rs",
]

# Helpers that announce the skip themselves, so a bare `return` after calling
# one is already loud. Each entry names where it announces, so a rename or a
# silenced helper is caught by --self-test's negative fixture and by review.
ANNOUNCING_HELPERS = [
    # eprintln!("skipping {label}: {env} is not set") inside the helper.
    "configured_hello_component",
    "configured_phase2_smoke_component",
    "configured_krate_clock_component",
    "configured_krate_cat_component",
    "configured_krate_curl_component",
    "configured_krate_go_clock_component",
    "configured_krate_go_cat_component",
    "configured_krate_go_curl_component",
    "configured_krate_ts_clock_component",
    "configured_krate_ts_cat_component",
    "configured_krate_ts_curl_component",
    "configured_component_from_env",
    "language_variant_components",
    # eprintln!("skipping {label}: localhost bind is blocked") inside.
    "bind_local_fixture_listener",
    "spawn_http_fixture",
]

# How far back an announcement may sit from its `return`. A skip message
# twelve lines from the return it explains is a skip message nobody will
# keep beside it through refactors.
WINDOW = 6


def find_silent_returns(source: str, path: str) -> list[str]:
    lines = source.split("\n")
    problems = []
    in_test = False
    fn_name = ""
    fn_indent = 0
    for index, line in enumerate(lines):
        header = re.match(r"(\s*)fn\s+(\w+)", line)
        if header:
            # A test is a fn directly preceded by #[test] (attributes and doc
            # lines may sit between).
            back = index - 1
            is_test = False
            while back >= 0 and (
                lines[back].strip().startswith("#[")
                or lines[back].strip().startswith("///")
                or lines[back].strip().startswith("//")
            ):
                if lines[back].strip() == "#[test]":
                    is_test = True
                back -= 1
            in_test = is_test
            fn_name = header.group(2)
            fn_indent = len(header.group(1))
            continue
        if not in_test:
            continue
        # Leaving the fn: a closing brace at the fn's own indent.
        if line.rstrip() == " " * fn_indent + "}":
            in_test = False
            continue
        if not re.match(r"\s+return;\s*$", line):
            continue
        window = "\n".join(lines[max(0, index - WINDOW) : index + 1])
        announced = "skipping" in window or "eprintln!" in window
        excused = any(helper in window for helper in ANNOUNCING_HELPERS)
        if not announced and not excused:
            problems.append(
                f"{path}:{index + 1}: silent early return in test `{fn_name}` "
                f"-- announce the skip (eprintln!(\"skipping ...\")) or call "
                f"an announcing helper, so the run's tally can count it"
            )
    return problems


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    problems = []
    for suite in SUITES:
        source_path = ROOT / suite
        if not source_path.is_file():
            # A suite this checker is FOR going missing is the claim-gate
            # defect all over again (K-277): refuse, never shrug.
            print(f"test suite MISSING: {suite}")
            return 1
        problems.extend(find_silent_returns(source_path.read_text(), suite))
    if problems:
        print(f"{len(problems)} silent early return(s) in test suites:\n")
        for problem in problems:
            print(f"  {problem}")
        print(
            "\nA test that returns early counts as PASSED, and a silent one "
            "is indistinguishable from a real pass in every report (K-269)."
        )
        return 1
    print(f"OK -- no silent early returns across {len(SUITES)} suite(s).")
    return 0


def self_test() -> int:
    failures = []
    silent = (
        "#[test]\nfn quiet() {\n    let Some(x) = maybe() else {\n"
        "        return;\n    };\n    assert!(x);\n}\n"
    )
    if not find_silent_returns(silent, "sample.rs"):
        failures.append("a silent early return was not rejected")

    loud = (
        "#[test]\nfn loud() {\n    let Some(x) = maybe() else {\n"
        '        eprintln!("skipping loud: fixture absent");\n'
        "        return;\n    };\n    assert!(x);\n}\n"
    )
    if find_silent_returns(loud, "sample.rs"):
        failures.append("an announced skip was wrongly rejected")

    helper = (
        "#[test]\nfn via_helper() {\n"
        "    let Some(x) = configured_hello_component() else {\n"
        "        return;\n    };\n    assert!(x.exists());\n}\n"
    )
    if find_silent_returns(helper, "sample.rs"):
        failures.append("a helper-announced skip was wrongly rejected")

    plain_fn = (
        "fn not_a_test() {\n    if bad() {\n        return;\n    }\n}\n"
    )
    if find_silent_returns(plain_fn, "sample.rs"):
        failures.append("a non-test fn was policed; only tests count as passes")

    if failures:
        print("check-test-skips self-test FAILED:\n")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print("OK -- the checker rejects silence, allows announced and helper skips,")
    print("and leaves non-test functions alone.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
