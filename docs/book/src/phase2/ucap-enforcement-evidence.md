# UCap Enforcement Evidence

This page shows how we collect repeatable proof that Phase 2 permission checks
are really enforced at runtime boundaries.

## What This Covers

The current evidence run checks six deny paths:

1. Runtime deny matrix for non-default capabilities
2. Dispatcher deny-before-adapter matrix for every non-default Phase 2 boundary
3. `krate-cat` denies missing `fs.read` grant
4. `krate-curl` denies missing `net.connect` grant
5. Manifest-required capability deny path
6. Rust Go TypeScript curl missing-grant parity test

The second check is important. It proves the runtime returns permission denied
before the host adapter can open a file, list a directory, remove a path, create
a directory, rename a path, or start a network fetch.

## Record One Host Report

Run this on Linux, macOS, and Windows:

```bash
scripts/record-phase2-ucap-evidence.sh --strict
```

By default, the report is written to:

`target/phase2-ucap-evidence/ucap-enforcement-evidence.md`

You can choose a custom output path:

```bash
scripts/record-phase2-ucap-evidence.sh --strict --output /tmp/ucap-linux.md
```

## Compare Three Host Reports

After you have one report from each host:

```bash
scripts/compare-phase2-ucap-evidence.sh /tmp/ucap-linux.md /tmp/ucap-macos.md /tmp/ucap-windows.md
```

The comparator fails when:

- commit metadata does not match across reports
- host labels do not match the expected OS lane
- any required deny check failed on any host

## Hosted CI Evidence

Full hosted CI now records one UCap evidence report per OS lane and uploads:

- `ucap-enforcement-evidence-ubuntu-latest`
- `ucap-enforcement-evidence-macos-latest`
- `ucap-enforcement-evidence-windows-latest`

Then CI runs an evidence compare job:

- `UCap enforcement evidence compare`

This gives us one strict cross-host gate for `P2E-09`.
