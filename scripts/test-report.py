#!/usr/bin/env python3
"""Turn a cargo-test log into an honest, categorised report (IC-706 / IC-709).

    cargo test --workspace -- --nocapture 2>&1 | tee run.log
    scripts/test-report.py run.log
    scripts/test-report.py --self-test

Cargo counts a test that returns early as PASSED, so the number at the end of
a run reads as coverage when it is not (K-269: 44 of 121 CLI tests did nothing
on this machine, all green). The register asks for the opposite: "a clean
report that separates substantive passes, skips, ignored cases and setup
failures. Ordinary pass counts may contain neither."

This reads the log cargo already produces and splits its "passed" total into:

  ran           a real pass -- reached its assertions
  skipped       an OPTIONAL fixture was absent (env var not set, a
                platform the test does not apply to). Coverage we choose not
                to have here; fine to be absent.
  setup-failed  a MANDATORY fixture was absent (a CI-built component "not
                built", a service that "could not connect"). Coverage we
                MEANT to have and did not get -- distinct from a skip because
                it means the lane is not proving what it claims to.

A skip and a setup failure look identical in cargo's count; the difference is
in the skip line's own words, which the tests already print. The report does
not change any exit code -- it explains the number, it does not gate on it.
Whether a setup failure should FAIL a lane is the lane's decision (a
mandatory-fixture lane sets KRATE_REPORT_STRICT=1); by default this only tells
the truth about what ran.
"""
import re
import sys
from pathlib import Path

# A skip line whose reason marks it as an optional-coverage skip rather than a
# mandatory-fixture setup failure. Ordered: the first that matches wins.
# Every fixture a test can skip on, who provides it, and whether the lane
# that claims full coverage may run without it (1422: a skip names its
# fixture, its reason and its owner). The first column is matched against
# the skip line's text, case-insensitively. A skip that matches nothing
# here and none of the markers below is a setup failure with no owner --
# which is the report's way of saying "classify me".
FIXTURES = [
    # (text in the skip line, kind, owner -- who builds or provides it)
    #
    # The loopback rows come first: "krate-ts-curl fixture: runtime could not
    # connect to localhost" is the loopback's fault, not the TS builder's.
    ("localhost", "mandatory",
     "the loopback interface -- a sandbox or firewall that refuses bind, not a fixture"),
    ("bind is not permitted", "mandatory",
     "the loopback interface -- a sandbox or firewall that refuses bind, not a fixture"),
    # The language variants next: "krate-go-cat" must not be read as
    # the mandatory "krate-cat". They are built by a later CI step and
    # tested by scripts/test-phase2-language-variants.sh, so at phase-1
    # time their absence is expected (IC-707 keeps them visibly optional).
    ("krate-go-", "optional",
     "scripts/build-phase2-language-variant-fixtures.sh with KRATE_LANGUAGE_VARIANTS_MODE=go"),
    ("KRATE_GO_", "optional",
     "scripts/build-phase2-language-variant-fixtures.sh with KRATE_LANGUAGE_VARIANTS_MODE=go"),
    ("krate-ts-", "optional",
     "scripts/build-phase2-language-variant-fixtures.sh with KRATE_LANGUAGE_VARIANTS_MODE=ts"),
    ("KRATE_TS_", "optional",
     "scripts/build-phase2-language-variant-fixtures.sh with KRATE_LANGUAGE_VARIANTS_MODE=ts"),
    ("phase2 smoke", "mandatory",
     "scripts/build-phase2-smoke-component.sh -> KRATE_PHASE2_SMOKE_WASM (CI: hello-fixture job)"),
    ("KRATE_PHASE2_SMOKE_WASM", "mandatory",
     "scripts/build-phase2-smoke-component.sh -> KRATE_PHASE2_SMOKE_WASM (CI: hello-fixture job)"),
    ("krate-clock", "mandatory",
     "scripts/build-krate-clock-component.sh -> KRATE_CLOCK_WASM (CI: hello-fixture job)"),
    ("KRATE_CLOCK_WASM", "mandatory",
     "scripts/build-krate-clock-component.sh -> KRATE_CLOCK_WASM (CI: hello-fixture job)"),
    ("krate-cat", "mandatory",
     "scripts/build-krate-cat-component.sh -> KRATE_CAT_WASM (CI: hello-fixture job)"),
    ("krate-curl", "mandatory",
     "scripts/build-krate-curl-component.sh -> KRATE_CURL_WASM (CI: hello-fixture job)"),
    ("cargo-component", "mandatory",
     "cargo install cargo-component --locked --version 0.21.1 (CI: the test job's install step)"),
    ("Go fixture", "optional",
     "scripts/build-phase2-language-variant-fixtures.sh with KRATE_LANGUAGE_VARIANTS_MODE=go"),
    ("TypeScript fixture", "optional",
     "scripts/build-phase2-language-variant-fixtures.sh with KRATE_LANGUAGE_VARIANTS_MODE=ts"),
    ("KRATE_WINIT_NATIVE_SMOKE", "optional",
     "a real display; run by hand with KRATE_WINIT_NATIVE_SMOKE=1"),
    ("system fonts", "optional",
     "the host's font set; a headless runner has none"),
    ("GPU adapter", "optional",
     "a GPU the offscreen presenter can open; a headless runner has none"),
]

# Reasons that are not a fixture at all: a platform the test does not cover,
# a build shape that is allowed. Fine anywhere.
OPTIONAL_MARKERS = [
    "is not set",          # an opt-in env var the developer did not set
    "on windows",          # a platform the test deliberately does not cover
    "on macos",
    "on linux",
    "without git",         # a build with no VCS -- a real, allowed state
]
SETUP_FAILURE_MARKERS = [
    "not built",           # a CI fixture that should have been built
    "could not connect",   # a fixture service that should have been up
    "is blocked",          # localhost bind refused -- environment, but wanted
]
NO_OWNER = "nobody -- add this fixture to FIXTURES in scripts/test-report.py"

TEST_RESULT = re.compile(
    r"test result:\s*(\w+)\.\s*(\d+)\s+passed;\s*(\d+)\s+failed;"
    r"\s*(\d+)\s+ignored"
)
SKIP_LINE = re.compile(r"^skipping\b(.*)", re.MULTILINE)
# Under --nocapture a skip line is several stderr writes, and another test
# thread's result line (stdout) can land between them:
#   skipping krate-curl component test: test some_other_test ... ok
#   KRATE_CURL_WASM is not set
# The "skipping" prefix still appears exactly once per skip, so the count is
# right; only the reason text is torn. The fragment is cut off, and a reason
# that is nothing but fragment is recorded as lost rather than passed off as
# a reason.
INTERLEAVED_RESULT = re.compile(r"\s*test \S+ \.\.\. (?:ok|FAILED|ignored)\s*$")
REASON_LOST = "(reason lost to interleaved output; run with --test-threads=1 to see it)"


def classify_skip(reason: str) -> tuple[str, str]:
    """(bucket, owner) for one skip line. bucket is "skipped" or "setup-failed"."""
    low = reason.lower()
    owner = NO_OWNER
    kind = None
    for text, fixture_kind, fixture_owner in FIXTURES:
        if text.lower() in low:
            kind, owner = fixture_kind, fixture_owner
            break
    # A named fixture that could not be reached is a setup failure whatever
    # its kind -- "krate-curl ... could not connect" is coverage the lane
    # wanted and did not get, even though krate-curl itself was built.
    for marker in SETUP_FAILURE_MARKERS:
        if marker in low:
            return "setup-failed", owner
    if kind == "optional":
        return "skipped", owner
    if kind == "mandatory":
        return "setup-failed", owner
    for marker in OPTIONAL_MARKERS:
        if marker in low:
            return "skipped", owner
    # An unrecognised skip is treated as a setup failure, not waved through:
    # a reason nobody classified is a reason nobody vouched for, and the safe
    # reading of "I don't know why this skipped" is "the coverage is missing".
    return "setup-failed", owner


def report(log: str) -> dict:
    passed = failed = ignored = 0
    for _outcome, p, f, ig in TEST_RESULT.findall(log):
        passed += int(p)
        failed += int(f)
        ignored += int(ig)

    skips = {"skipped": [], "setup-failed": []}
    owners = {}
    for match in SKIP_LINE.finditer(log):
        reason = INTERLEAVED_RESULT.sub("", match.group(1)).strip().strip(":").strip()
        if not reason:
            reason = REASON_LOST
        bucket, owner = classify_skip(reason)
        skips[bucket].append(reason)
        owners[reason] = owner

    # A skip happens INSIDE a test cargo counts as passed, so the tests that
    # only ran to their skip line are a subset of `passed`. "ran" is what is
    # left once they are removed.
    skipped_n = len(skips["skipped"])
    setup_n = len(skips["setup-failed"])
    ran = passed - skipped_n - setup_n
    return {
        # No "test result:" line at all means cargo never reached the tests --
        # a compile error, the wrong toolchain, a killed run. That is not
        # "0 ran, 0 failed"; it is a run that has nothing to report, and the
        # report has to say so instead of printing a row of clean zeros
        # (which is exactly what a compile failure looked like on 2026-09-11).
        "suites": len(TEST_RESULT.findall(log)),
        "ran": ran,
        "skipped": skips["skipped"],
        "setup_failed": skips["setup-failed"],
        "owners": owners,
        "failed": failed,
        "ignored": ignored,
        "counted_passed": passed,
        # 1426: every function lands in exactly one bucket. Passed splits
        # into ran + skipped + setup-failed; failed and ignored are cargo's
        # own. If there are more skip lines than passing tests the log does
        # not reconcile -- a test printed two, or a skip came from something
        # that is not a test -- and the report must say so, not go negative.
        "total": passed + failed + ignored,
        "reconciles": ran >= 0,
    }


def render(r: dict) -> str:
    if r["suites"] == 0:
        return (
            "Test report: NO TEST RESULTS in this log.\n"
            "  cargo never reached the tests (a compile error, a killed run, or\n"
            "  the wrong toolchain). Nothing ran, so nothing here is proven."
        )
    if not r["reconciles"]:
        return (
            f"Test report: DOES NOT RECONCILE. cargo counted {r['counted_passed']} "
            f"passed but the log holds {len(r['skipped']) + len(r['setup_failed'])} "
            "skip lines -- more skips than passes.\n"
            "  A test printed two skip lines, or a skip line came from something\n"
            "  that is not a test. The buckets below would be wrong, so none are shown."
        )
    lines = [
        "Test report -- what the pass count actually contains:",
        f"  ran            {r['ran']:>4}   passed and printed no skip line",
        f"  skipped        {len(r['skipped']):>4}   optional fixture absent (fine here)",
        f"  setup-failed   {len(r['setup_failed']):>4}   MANDATORY fixture absent (coverage missing)",
        f"  failed         {r['failed']:>4}",
        f"  ignored        {r['ignored']:>4}",
        f"  = {r['total']} test functions, each counted once",
        # IC-709: a derived number must not be sold as a measured one. "ran"
        # is cargo's pass count minus the skip lines; nothing here observed
        # an assertion executing, and the report says so rather than let a
        # reader turn 843 into "843 assertions verified".
        "  ('ran' is cargo's pass count minus skip lines; assertion execution",
        "   is not measured, so it is not an assertion count)",
    ]
    if r["setup_failed"]:
        lines.append("")
        lines.append("  coverage that was meant to run and did not:")
        for reason in sorted(set(r["setup_failed"])):
            lines.append(f"    - {reason}")
            lines.append(f"        owner: {r['owners'][reason]}")
    return "\n".join(lines)


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if not args:
        print("usage: test-report.py <cargo-test-log> [--strict]", file=sys.stderr)
        return 2
    log = Path(args[0]).read_text(errors="replace")
    r = report(log)
    print(render(r))
    if r["suites"] == 0 or not r["reconciles"] or r["failed"]:
        return 1
    if "--strict" in sys.argv and r["setup_failed"]:
        print(
            "\nstrict: a mandatory fixture did not build, so this lane is not "
            "proving what it claims. Failing.",
            file=sys.stderr,
        )
        return 1
    return 0


def self_test() -> int:
    failures = []

    # An optional skip and a setup failure look the same to cargo; the report
    # must tell them apart by the skip line's words.
    #
    # A mandatory fixture is filed as a setup failure even when the words
    # are the same "is not set" an optional flag uses: KRATE_CLOCK_WASM is
    # coverage the full lane provides, KRATE_WINIT_NATIVE_SMOKE is a display
    # nobody expects a runner to have.
    log = (
        "test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured\n"
        "skipping krate-clock component test: KRATE_CLOCK_WASM is not set\n"
        "skipping phase2 smoke fixture: not built\n"
        "skipping the tool check on Windows: a .exe cannot be faked\n"
        "skipping: KRATE_WINIT_NATIVE_SMOKE not set\n"
    )
    r = report(log)
    if len(r["skipped"]) != 2:  # "on Windows" + the native-smoke opt-in
        failures.append(f"expected 2 optional skips, got {len(r['skipped'])}: {r['skipped']}")
    if len(r["setup_failed"]) != 2:  # "not built" + the clock fixture
        failures.append(
            f"expected 2 setup failures, got {len(r['setup_failed'])}: {r['setup_failed']}"
        )
    if r["ran"] != 5 - 4:
        failures.append(f"ran should be passed minus all skips: got {r['ran']}")
    if r["ignored"] != 1:
        failures.append(f"ignored not carried through: {r['ignored']}")

    # An unrecognised skip reason is a setup failure, never a silent pass.
    r2 = report("test result: ok. 1 passed; 0 failed; 0 ignored\nskipping mystery: who knows\n")
    if r2["setup_failed"] != ["mystery: who knows"]:
        failures.append(f"an unclassified skip must be a setup failure: {r2}")

    # A real failure count survives.
    r3 = report("test result: FAILED. 3 passed; 2 failed; 0 ignored\n")
    if r3["failed"] != 2:
        failures.append(f"failures must be counted: {r3['failed']}")

    # A log with no test results is not a clean run of zero tests. A compile
    # error produced exactly this shape and the first version of the report
    # printed five tidy zeros for it.
    r4 = report("error: rustc 1.91.1 is not supported by the following packages:\n")
    if r4["suites"] != 0 or "NO TEST RESULTS" not in render(r4):
        failures.append(f"a log without test results must be called out: {render(r4)!r}")
    if r["suites"] != 1:
        failures.append(f"suite count must be carried through: {r['suites']}")

    # 1422: a mandatory fixture's absence names who provides it; an optional
    # one is filed as a skip whatever words the test chose.
    r5 = report(
        "test result: ok. 4 passed; 0 failed; 0 ignored\n"
        "skipping krate-curl component test: KRATE_CURL_WASM is not set\n"
        "skipping krate-curl success fixture: runtime could not connect to localhost\n"
        "skipping language variant cat parity: Go fixture is unavailable\n"
        "skipping: cargo-component is not installed\n"
    )
    if sorted(r5["setup_failed"]) != [
        "cargo-component is not installed",
        "krate-curl component test: KRATE_CURL_WASM is not set",
        "krate-curl success fixture: runtime could not connect to localhost",
    ]:
        failures.append(f"mandatory fixtures must land in setup-failed: {r5['setup_failed']}")
    if r5["skipped"] != ["language variant cat parity: Go fixture is unavailable"]:
        failures.append(f"an optional language fixture is a skip: {r5['skipped']}")
    # "krate-go-cat" is the optional Go variant, not the mandatory krate-cat.
    r5b = report(
        "test result: ok. 1 passed; 0 failed; 0 ignored\n"
        "skipping krate-go-cat component test: KRATE_GO_CAT_WASM is not set\n"
    )
    if r5b["setup_failed"] or "MODE=go" not in r5b["owners"][r5b["skipped"][0]]:
        failures.append(f"a Go variant must be an optional skip with its builder: {r5b}")
    if "build-krate-curl-component.sh" not in \
            r5["owners"]["krate-curl component test: KRATE_CURL_WASM is not set"]:
        failures.append(f"the curl fixture must name its builder: {r5['owners']}")
    if "loopback" not in \
            r5["owners"]["krate-curl success fixture: runtime could not connect to localhost"]:
        failures.append(f"a curl fixture that could not connect is the loopback's: {r5['owners']}")
    if "cargo install cargo-component" not in render(r5):
        failures.append("the rendered report must show each setup failure's owner")
    if r2["owners"]["mystery: who knows"] != NO_OWNER:
        failures.append(f"an unclassified skip must say it has no owner: {r2['owners']}")

    # Interleaved --nocapture output: the count holds, the torn reason is
    # cut back to what the test actually said, and a reason that was lost
    # entirely says so instead of reading as a passing test's name.
    r7 = report(
        "test result: ok. 3 passed; 0 failed; 0 ignored\n"
        "skipping krate-curl component test: test some_other_test ... ok\n"
        "KRATE_CURL_WASM is not set\n"
        "skipping test another_test ... ok\n"
        "skipping krate-ts-curl fixture: runtime could not connect to localhost fixture\n"
    )
    if len(r7["setup_failed"]) != 3 or r7["ran"] != 0:
        failures.append(f"an interleaved skip still counts once: {r7}")
    if "krate-curl component test" not in r7["setup_failed"] or \
       "some_other_test" in "".join(r7["setup_failed"]):
        failures.append(f"the other thread's result line must be cut off: {r7['setup_failed']}")
    if REASON_LOST not in r7["setup_failed"]:
        failures.append(f"a wholly torn reason must be recorded as lost: {r7['setup_failed']}")
    if "loopback" not in r7["owners"]["krate-ts-curl fixture: runtime could not connect to localhost fixture"]:
        failures.append(f"could-not-connect is the loopback's, not the TS builder's: {r7['owners']}")

    # 1426: the buckets add up to the function count, once each -- and a log
    # where they cannot is refused rather than reported with a negative.
    if r["total"] != 5 + 0 + 1 or not r["reconciles"]:
        failures.append(f"total must be passed + failed + ignored: {r['total']}")
    if "= 6 test functions, each counted once" not in render(r):
        failures.append("the report must print the reconciliation line")
    if "assertion execution" not in render(r) or "reached their assertions" in render(r):
        failures.append("the report must not present a derived count as a measured one")
    r6 = report("test result: ok. 1 passed; 0 failed; 0 ignored\nskipping: a\nskipping: b\n")
    if r6["reconciles"] or "DOES NOT RECONCILE" not in render(r6):
        failures.append(f"more skips than passes must be refused: {render(r6)!r}")

    if failures:
        print("test-report self-test FAILED:\n")
        for f in failures:
            print(f"  {f}")
        return 1
    print("OK -- the report separates ran, optional skips, setup failures,")
    print("ignored and failed; names the owner of every missing mandatory")
    print("fixture; treats an unclassified skip as an ownerless setup failure;")
    print("reconciles the buckets to the function count; and refuses a log")
    print("with no test results, or more skips than passes, as a clean run.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
