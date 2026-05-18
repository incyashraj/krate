# Hosted Full CI Evidence

Normal push CI is intentionally small. It checks the fast Linux path, docs, UAPI
freshness, the Rust SDK package shape, formatting, and linting.

Phase 2 exit also needs the heavier hosted full CI path. That path runs the
Linux, macOS, and Windows lanes and records the cross-host evidence artifacts.

Use this recorder to prove that a recent hosted CI run was a full run, not only
the fast push run:

```bash
scripts/record-phase2-hosted-full-ci-evidence.sh
```

Default output:

```text
target/phase2-hosted-full-ci-evidence/hosted-full-ci-evidence.md
```

For final review, require a completed full run with every required full job
green:

```bash
scripts/record-phase2-hosted-full-ci-evidence.sh --require-success
```

If a full run is cancelled or fails, the report can still record it for triage,
but `--require-success` will reject it. The selected-run summary shows the
workflow conclusion separately from the required job table, so a cancelled run
cannot be mistaken for Phase 2 proof.

To limit the report to the final review window:

```bash
scripts/record-phase2-hosted-full-ci-evidence.sh \
  --created '>=2026-05-18' \
  --require-success
```

You can include it in the exit bundle:

```bash
scripts/record-phase2-exit-bundle.sh --strict --include-hosted-full-ci
```

The final review shortcut includes it too:

```bash
scripts/record-phase2-exit-bundle.sh --final-review
```

## Required Full Jobs

The recorder checks these hosted CI jobs:

- Phase 2 bindings
- Build shared component fixtures
- Full test on Linux
- Full test on macOS
- Full test on Windows
- Language variant evidence compare
- UCap enforcement evidence compare
- Adapter evidence compare
- Sample evidence compare
- Phase 2 benchmark check
- Dependency audit

If those jobs are missing or skipped, the run is not counted as hosted full CI
proof.

## How To Run Hosted Full CI

Use one of these paths:

```bash
gh workflow run CI --ref main -f full=true -f language_variants_mode=ts
```

or include `[full-ci]` in a commit message.

The manual workflow path is cleaner for final review because it does not require
a documentation-only commit just to trigger the heavy matrix.

## Shared Fixtures

Hosted full CI builds the Rust component fixtures once on Linux, uploads them,
then downloads the same files into each full-test lane.

The full-test lanes also copy those downloaded files into the app target paths
named by the sample manifests:

```text
apps/layer36-clock/target/wasm32-wasip1/release/layer36_clock.wasm
apps/layer36-cat/target/wasm32-wasip1/release/layer36_cat.wasm
apps/layer36-curl/target/wasm32-wasip1/release/layer36_curl.wasm
```

That keeps two checks true at the same time: each host runs the same shared
fixture bytes, and the sample manifest tests still use the exact entry paths
shown in the example apps.

## Windows Command-Line Limit

One guard test sends more than 64 KiB of app arguments to prove that Layer36
rejects the payload before the runtime starts. Linux and macOS can launch that
test command and Layer36 rejects it.

Windows has a lower process command-line limit for this shape of argument. The
OS rejects the process before Layer36 can run, so the Windows lane records this
case as a host-limit skip. The related count-limit, empty-argument, newline, and
NUL checks still run on Windows.

## Local HTTP Fixtures

Some curl tests use a tiny local HTTP server inside the test process. The
response-limit test asks Layer36 to stop after a very small number of response
bytes. On Windows, that early close can surface in the fixture thread as a
connection-aborted write. The fixture treats that as an accepted connection and
lets the test check the real Layer36 result: exit code `21` and a
`response too large` message.

## What This Does Not Prove

This is hosted full CI proof only.

It does not replace:

- normal hosted CI and Pages stability history
- self-hosted macOS ARM64 full-gate proof
- long fuzz soak proof
- the outside Rust walkthrough
- the final UAPI freeze decision

Each track answers a different question, so the final Phase 2 packet should
keep them separate.
